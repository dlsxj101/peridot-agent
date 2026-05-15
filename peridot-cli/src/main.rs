//! Peridot command-line entrypoint.

use std::fs;
use std::io::{IsTerminal, Read};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use async_trait::async_trait;
use clap::{Parser, Subcommand, ValueEnum};
use commands::{
    AgentsCommand, AuthProvider, ConfigCommand, McpCommand, OutputFormat, SessionCommand,
    SkillCommand, load_effective_config, maybe_print_update_notice, print_scan,
    read_stored_api_key, read_stored_openai_oauth_access_token, run_agents_command,
    run_config_command, run_login_command, run_logout_command, run_mcp_command,
    run_session_command, run_setup_command, run_skill_command, run_update_command,
    run_verify_command,
};
use peridot_common::{
    ContextConfig, ExecutionMode, MemoryConfig, PeriError, PeriResult, PeridotConfig,
    PermissionMode, ToolCall,
};
use peridot_context::{ContextLimits, ContextManager, project_context_limits};
use peridot_core::{AgentRunRequest, AgentRunSummary, AgentState, HarnessAgent, StopReason};
use peridot_git::GitManager;
use peridot_llm::{
    AuthMethod, ClaudeProvider, CompletionRequest, CompletionResponse, LlmProvider, OpenAiProvider,
    PricingTable, Usage, parse_action,
};
use peridot_mcp::McpClient;
use peridot_memory::{MemoryStore, SessionSummary, StoredSkill};
use peridot_project::ProjectScanner;
use peridot_tools::hooks::{HookRunner, HookVariables, lifecycle_hook_variables};
use peridot_tools::{ToolRegistry, register_builtin_tools, register_mcp_tools};
use peridot_tui::{HeaderState, TuiState, run_interactive};

mod commands;
mod context_limits;
mod direct_tools;
mod interactive_io;
mod providers;
mod run_loop;
mod run_output;
mod run_state;
#[cfg(test)]
mod tests;

use context_limits::project_context_limits_from_config;
use direct_tools::task_to_tool_call;
use interactive_io::{read_piped_task, run_tui_lifecycle_hooks};
use providers::{FileMockProvider, live_provider};
use run_loop::run_task;
use run_output::{
    exit_for_summary, exit_for_tool_result, print_run_summary_text, run_summary_output,
};
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

    /// Use the configured live provider instead of deterministic JSON/tool-only behavior.
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
                    let exit = run_interactive(state)?;
                    run_tui_lifecycle_hooks(&exit.state, &config, &project_root)?;
                    if let Some(task) = exit.submitted {
                        run_task(task, mode, &cli, &config, &project_root).await?;
                    }
                }
            }
        }
    }

    Ok(())
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
