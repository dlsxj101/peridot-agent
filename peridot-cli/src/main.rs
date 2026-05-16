//! Peridot command-line entrypoint.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use async_trait::async_trait;
use clap::{Parser, Subcommand, ValueEnum};
use commands::{
    AgentsCommand, AuthProvider, ConfigCommand, EnvCommand, McpCommand, OutputFormat,
    SessionCommand, SkillCommand, load_effective_config, maybe_print_update_notice,
    maybe_run_first_launch_wizard, print_scan, read_stored_api_key,
    read_stored_openai_oauth_access_token, run_agents_command, run_config_command, run_env_command,
    run_login_command, run_logout_command, run_mcp_command, run_session_command, run_setup_command,
    run_skill_command, run_update_command, run_verify_command,
};
use peridot_common::{
    ContextConfig, ExecutionMode, MemoryConfig, PeriError, PeriResult, PeridotConfig,
    PermissionMode,
};
use peridot_context::{ContextLimits, ContextManager, project_context_limits};
use peridot_core::{
    AgentRunEvent, AgentRunRequest, AgentRunSummary, AgentState, HarnessAgent, StopReason,
};
use peridot_git::GitManager;
use peridot_llm::{
    AuthMethod, ClaudeProvider, CodexAppServerProvider, CompletionRequest, CompletionResponse,
    LlmProvider, OpenAiProvider, PricingTable, Usage,
};
use peridot_mcp::McpClient;
use peridot_memory::{MemoryStore, SessionSummary, StoredSkill};
use peridot_project::ProjectScanner;
use peridot_tools::hooks::{HookRunner, HookVariables, lifecycle_hook_variables};
use peridot_tools::{ToolRegistry, register_builtin_tools, register_mcp_tools};
use peridot_tui::{
    ApprovalDecision, HeaderState, TuiRuntimeEvent, TuiState, run_interactive_with_events,
};

mod commands;
mod context_limits;
mod interactive_io;
mod providers;
mod run_loop;
mod run_output;
mod run_state;
#[cfg(test)]
mod tests;

use context_limits::project_context_limits_from_config;
use interactive_io::{read_piped_task, run_tui_lifecycle_hooks};
use providers::{FileMockProvider, live_provider};
use run_loop::{agent_task_options, run_task, run_task_with_events};
use run_output::{exit_for_summary, print_run_summary_text, run_summary_output};
use run_state::{apply_resume, auto_commit_run, save_run_session, unix_timestamp};

/// Peridot autonomous coding agent.
#[derive(Debug, Parser)]
#[command(name = "peridot", version, about = "Autonomous coding agent CLI/TUI")]
struct Cli {
    /// Model to use for the main agent.
    #[arg(long, global = true)]
    model: Option<String>,

    /// Execution mode.
    #[arg(long, value_enum, global = true)]
    mode: Option<CliMode>,

    /// Permission mode.
    #[arg(long, value_enum, global = true)]
    permission: Option<CliPermission>,

    /// Project root.
    #[arg(long, global = true)]
    project: Option<PathBuf>,

    /// Override config.toml path.
    #[arg(long, global = true)]
    config: Option<PathBuf>,

    /// TUI-less, script-friendly output.
    #[arg(long, global = true)]
    headless: bool,

    /// Output format for scriptable commands.
    #[arg(long, value_enum, default_value_t = OutputFormat::Text, global = true)]
    output: OutputFormat,

    /// Read deterministic assistant JSON responses from a file, one response per line.
    #[arg(long, global = true)]
    mock_response_file: Option<PathBuf>,

    /// Use the configured live provider. This is now the default unless --mock-response-file is set.
    #[arg(long, global = true)]
    live: bool,

    /// Maximum number of model/tool turns before stopping.
    #[arg(long, global = true)]
    max_turns: Option<u32>,

    /// Maximum estimated run cost in USD.
    #[arg(long, global = true)]
    budget: Option<f64>,

    /// Resume a saved session summary before running the task.
    #[arg(long, global = true)]
    resume: Option<String>,

    /// Optional task to start immediately.
    task: Option<String>,

    /// Subcommand to run.
    #[command(subcommand)]
    command: Option<Command>,
}

impl Cli {
    fn effective_headless(&self) -> bool {
        self.headless || env_truthy("PERIDOT_HEADLESS")
    }

    fn starts_agent_session(&self) -> bool {
        matches!(
            &self.command,
            None | Some(Command::Run { .. })
                | Some(Command::Plan { .. })
                | Some(Command::Goal { .. })
        )
    }
}

/// Top-level subcommands.
#[derive(Debug, Subcommand)]
enum Command {
    /// Run a task in execute mode.
    Run {
        /// Task text.
        task: Vec<String>,
    },
    /// Analyze and plan without modifying files.
    Plan {
        /// Task text.
        task: Vec<String>,
    },
    /// Run a durable goal.
    Goal {
        /// Goal text.
        task: Vec<String>,
    },
    /// Print a project scan.
    Scan,
    /// Run deterministic verification.
    Verify,
    /// Initialize project-local Peridot files.
    Setup,
    /// Configuration commands.
    Config {
        /// Config subcommand.
        #[command(subcommand)]
        command: ConfigCommand,
    },
    /// Session persistence commands.
    Session {
        /// Session subcommand.
        #[command(subcommand)]
        command: SessionCommand,
    },
    /// AGENTS.md commands.
    Agents {
        /// Agents subcommand.
        #[command(subcommand)]
        command: AgentsCommand,
    },
    /// Skill library commands.
    Skill {
        /// Skill subcommand.
        #[command(subcommand)]
        command: SkillCommand,
    },
    /// MCP server commands.
    Mcp {
        /// MCP subcommand.
        #[command(subcommand)]
        command: McpCommand,
    },
    /// Manage Peridot's user-local environment variable store.
    Env {
        /// Environment subcommand.
        #[command(subcommand)]
        command: EnvCommand,
    },
    /// Store provider credentials from environment.
    Login {
        /// Provider to configure.
        #[arg(value_enum)]
        provider: AuthProvider,
    },
    /// Remove stored provider credentials.
    Logout {
        /// Provider to remove.
        #[arg(value_enum)]
        provider: AuthProvider,
    },
    /// Check for or apply Peridot updates.
    Update {
        /// Check for an update without installing.
        #[arg(long)]
        check: bool,
        /// Install the latest release even when the current version matches.
        #[arg(long)]
        force: bool,
    },
    /// Print version information.
    Version,
}

/// Clap representation of execution modes.
#[derive(Clone, Copy, Debug, ValueEnum)]
enum CliMode {
    /// Read-only planning mode.
    Plan,
    /// Interactive execution mode.
    Execute,
    /// Goal mode.
    Goal,
}

impl From<CliMode> for ExecutionMode {
    fn from(value: CliMode) -> Self {
        match value {
            CliMode::Plan => Self::Plan,
            CliMode::Execute => Self::Execute,
            CliMode::Goal => Self::Goal,
        }
    }
}

/// Clap representation of permission modes.
#[derive(Clone, Copy, Debug, ValueEnum)]
enum CliPermission {
    /// Confirm every write/shell/git operation.
    Safe,
    /// Confirm risky operations.
    Auto,
    /// Run without confirmations except hard security blocks.
    Yolo,
}

impl From<CliPermission> for PermissionMode {
    fn from(value: CliPermission) -> Self {
        match value {
            CliPermission::Safe => Self::Safe,
            CliPermission::Auto => Self::Auto,
            CliPermission::Yolo => Self::Yolo,
        }
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let project_root = cli.project.clone().unwrap_or(std::env::current_dir()?);
    if cli.starts_agent_session() {
        maybe_run_first_launch_wizard(
            &project_root,
            cli.config.as_deref(),
            cli.effective_headless(),
            cli.output,
        )?;
    }
    let config = load_effective_config(&project_root, cli.config.as_deref())?;

    match &cli.command {
        Some(Command::Version) => {
            println!("peridot {}", env!("CARGO_PKG_VERSION"));
            return Ok(());
        }
        Some(Command::Scan) => {
            let profile = ProjectScanner::new().scan(&project_root)?;
            print_scan(&profile, cli.output)?;
            return Ok(());
        }
        Some(Command::Verify) => {
            run_verify_command(&project_root, &config, cli.output)?;
            return Ok(());
        }
        Some(Command::Setup) => {
            run_setup_command(&project_root, cli.output)?;
            return Ok(());
        }
        Some(Command::Config { command }) => {
            run_config_command(command, &config, &project_root, cli.output)?;
            return Ok(());
        }
        Some(Command::Session { command }) => {
            run_session_command(command, &project_root, cli.output)?;
            return Ok(());
        }
        Some(Command::Agents { command }) => {
            run_agents_command(command, &project_root, cli.output)?;
            return Ok(());
        }
        Some(Command::Skill { command }) => {
            run_skill_command(command, &project_root, cli.output).await?;
            return Ok(());
        }
        Some(Command::Mcp { command }) => {
            run_mcp_command(command, &config, cli.output).await?;
            return Ok(());
        }
        Some(Command::Env { command }) => {
            run_env_command(command, cli.output)?;
            return Ok(());
        }
        Some(Command::Login { provider }) => {
            run_login_command(*provider, cli.output).await?;
            return Ok(());
        }
        Some(Command::Logout { provider }) => {
            run_logout_command(*provider, cli.output)?;
            return Ok(());
        }
        Some(Command::Update { check, force }) => {
            run_update_command(*check, *force, cli.output).await?;
            return Ok(());
        }
        Some(Command::Run { task }) => {
            return run_task(
                task.join(" "),
                ExecutionMode::Execute,
                &cli,
                &config,
                &project_root,
            )
            .await;
        }
        Some(Command::Plan { task }) => {
            return run_task(
                task.join(" "),
                ExecutionMode::Plan,
                &cli,
                &config,
                &project_root,
            )
            .await;
        }
        Some(Command::Goal { task }) => {
            return run_task(
                task.join(" "),
                ExecutionMode::Goal,
                &cli,
                &config,
                &project_root,
            )
            .await;
        }
        None => {}
    }

    let mode = cli
        .mode
        .map(ExecutionMode::from)
        .unwrap_or(config.defaults.mode);
    let permission = cli
        .permission
        .map(PermissionMode::from)
        .unwrap_or(config.defaults.permission);

    match cli.task.as_ref() {
        Some(task) => {
            run_task(task.clone(), mode, &cli, &config, &project_root).await?;
        }
        None => {
            if let Some(task) = read_piped_task()? {
                run_task(task, mode, &cli, &config, &project_root).await?;
            } else if cli.resume.is_some() {
                run_task(
                    "Continue the resumed session.".to_string(),
                    mode,
                    &cli,
                    &config,
                    &project_root,
                )
                .await?;
            } else {
                let model = cli
                    .model
                    .clone()
                    .unwrap_or_else(|| config.models.main.clone());
                if cli.effective_headless() || cli.output == OutputFormat::Json {
                    match cli.output {
                        OutputFormat::Json => println!(
                            "{}",
                            serde_json::to_string_pretty(&serde_json::json!({
                                "status": "idle",
                                "mode": mode,
                                "permission": permission,
                                "model": model
                            }))?
                        ),
                        OutputFormat::Text => println!(
                            "Hello Peridot: mode={} permission={} model={}",
                            mode, permission, model
                        ),
                    }
                } else {
                    maybe_print_update_notice(&config, cli.effective_headless(), cli.output).await;
                    let mut state =
                        TuiState::new(HeaderState::new(mode, permission, model.clone()))
                            .with_config(config.tui.clone());
                    state.push_transcript("Peridot ready. Type a task, /plan, /execute, /goal <objective>, /safe, /auto, /yolo, or Esc.");
                    state.push_transcript(
                        "Submitted tasks continue inside this TUI; tool activity and run status stream here.",
                    );
                    let (event_tx, event_rx) = std::sync::mpsc::channel();
                    let handle = tokio::runtime::Handle::current();
                    let base_options = agent_task_options(&cli, &config);
                    let run_config = config.clone();
                    let run_project_root = project_root.clone();
                    let submit_options = base_options.clone();
                    let approve_options = base_options.clone();
                    let submit_config = run_config.clone();
                    let approve_config = run_config.clone();
                    let submit_project_root = run_project_root.clone();
                    let approve_project_root = run_project_root.clone();
                    let exit = run_interactive_with_events(
                        state,
                        event_rx,
                        {
                            let event_tx = event_tx.clone();
                            let handle = handle.clone();
                            move |task, state| {
                                let mut options = submit_options.clone();
                                options.permission = state.header.permission;
                                options.model = state.header.model.clone();
                                spawn_tui_agent_run(
                                    handle.clone(),
                                    event_tx.clone(),
                                    task,
                                    state.header.mode,
                                    options,
                                    submit_config.clone(),
                                    submit_project_root.clone(),
                                );
                            }
                        },
                        {
                            let event_tx = event_tx.clone();
                            let handle = handle.clone();
                            move |decision, _tool_name, reason, state| {
                                if decision != ApprovalDecision::Approve {
                                    return;
                                }
                                let Some(task) = state.last_task.clone() else {
                                    state.push_transcript(
                                        "approval: no task is available to resume",
                                    );
                                    return;
                                };
                                let mut options = approve_options.clone();
                                options.permission = state.header.permission;
                                options.model = state.header.model.clone();
                                let mut config = approve_config.clone();
                                relax_security_for_approval(&mut config, &reason);
                                spawn_tui_agent_run(
                                    handle.clone(),
                                    event_tx.clone(),
                                    task,
                                    state.header.mode,
                                    options,
                                    config,
                                    approve_project_root.clone(),
                                );
                            }
                        },
                    )?;
                    run_tui_lifecycle_hooks(&exit.state, &config, &project_root)?;
                }
            }
        }
    }

    Ok(())
}

fn spawn_tui_agent_run(
    handle: tokio::runtime::Handle,
    event_tx: std::sync::mpsc::Sender<TuiRuntimeEvent>,
    task: String,
    mode: ExecutionMode,
    options: run_loop::AgentTaskOptions,
    config: PeridotConfig,
    project_root: PathBuf,
) {
    handle.spawn(async move {
        let event_sender = event_tx.clone();
        let result =
            run_task_with_events(task, mode, options, config, project_root, move |event| {
                let _ = event_sender.send(tui_runtime_event_from_agent(event));
            })
            .await;
        if let Err(err) = result {
            let _ = event_tx.send(TuiRuntimeEvent::Failed {
                message: err.to_string(),
            });
        }
    });
}

fn relax_security_for_approval(config: &mut PeridotConfig, reason: &str) {
    if reason.contains("dependency installation") {
        config.security.ask_before_install = false;
    }
    if reason.contains("destructive shell command") {
        config.security.ask_before_delete = false;
    }
}

fn tui_runtime_event_from_agent(event: AgentRunEvent) -> TuiRuntimeEvent {
    match event {
        AgentRunEvent::RunStarted { task } => TuiRuntimeEvent::RunStarted { task },
        AgentRunEvent::TurnStarted { turn_index } => TuiRuntimeEvent::TurnStarted { turn_index },
        AgentRunEvent::AssistantStarted { label } => TuiRuntimeEvent::AssistantStarted { label },
        AgentRunEvent::AssistantDelta { delta } => TuiRuntimeEvent::AssistantDelta { delta },
        AgentRunEvent::AssistantFinished { .. } => TuiRuntimeEvent::AssistantFinished,
        AgentRunEvent::Thinking { text } => TuiRuntimeEvent::Thinking { text },
        AgentRunEvent::ToolStarted { name, parameters } => {
            TuiRuntimeEvent::ToolStarted { name, parameters }
        }
        AgentRunEvent::ToolFinished { name, result } => TuiRuntimeEvent::ToolFinished {
            name,
            success: result.success,
            summary: result.summary,
            output: result.output,
        },
        AgentRunEvent::ApprovalRequested { tool_name, reason } => {
            TuiRuntimeEvent::ApprovalRequested { tool_name, reason }
        }
        AgentRunEvent::UsageUpdated { usage } => {
            let prompt_tokens =
                usage.input_tokens + usage.cache_read_tokens + usage.cache_creation_tokens;
            let cache_hit_rate = if prompt_tokens == 0 {
                0.0
            } else {
                usage.cache_read_tokens as f64 / prompt_tokens as f64
            };
            TuiRuntimeEvent::UsageUpdated {
                total_tokens: usage.input_tokens
                    + usage.output_tokens
                    + usage.cache_read_tokens
                    + usage.cache_creation_tokens
                    + usage.reasoning_output_tokens,
                cache_hit_rate,
                cost_usd: usage.estimated_cost_usd,
            }
        }
        AgentRunEvent::Recovery { message } => TuiRuntimeEvent::Recovery { message },
        AgentRunEvent::Finished { summary } => TuiRuntimeEvent::Finished {
            success: summary.stopped_reason == StopReason::Done,
            stop_reason: format!("{:?}", summary.stopped_reason),
            turns: summary.turns.len(),
        },
        AgentRunEvent::SessionSaved { session_id } => TuiRuntimeEvent::SessionSaved { session_id },
        AgentRunEvent::SessionSaveFailed {
            session_id,
            message,
        } => TuiRuntimeEvent::SessionSaveFailed {
            session_id,
            message,
        },
    }
}

fn env_truthy(name: &str) -> bool {
    std::env::var(name)
        .ok()
        .map(|value| {
            matches!(
                value.trim().to_ascii_lowercase().as_str(),
                "1" | "true" | "yes" | "on"
            )
        })
        .unwrap_or(false)
}
