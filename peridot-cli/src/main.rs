//! Peridot command-line entrypoint.

use std::fs;
use std::io::{IsTerminal, Read};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use async_trait::async_trait;
use clap::{Parser, Subcommand, ValueEnum};
use commands::{
    AgentsCommand, AuthProvider, ConfigCommand, McpCommand, OutputFormat, SessionCommand,
    SkillCommand, load_project_config, print_scan, read_stored_api_key,
    read_stored_openai_oauth_access_token, run_agents_command, run_config_command,
    run_login_command, run_logout_command, run_mcp_command, run_session_command, run_setup_command,
    run_skill_command, run_update_command, run_verify_command,
};
use peridot_common::{
    ExecutionMode, PeriError, PeriResult, PeridotConfig, PermissionMode, ToolCall,
};
use peridot_context::{ContextManager, project_context_limits};
use peridot_core::{AgentRunRequest, AgentState, HarnessAgent};
use peridot_llm::{
    AuthMethod, ClaudeProvider, CompletionRequest, CompletionResponse, LlmProvider, OpenAiProvider,
    PricingTable, Usage, parse_action,
};
use peridot_mcp::McpClient;
use peridot_project::ProjectScanner;
use peridot_tools::{ToolRegistry, register_builtin_tools, register_mcp_tools};
use peridot_tui::{HeaderState, TuiState, run_interactive};

mod commands;

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

    /// Optional task to start immediately.
    task: Option<String>,

    /// Subcommand to run.
    #[command(subcommand)]
    command: Option<Command>,
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
    let config = load_project_config(&project_root)?;

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
            run_verify_command(&project_root, cli.output)?;
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
            run_skill_command(command, &project_root, cli.output)?;
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
        Some(Command::Update { check }) => {
            run_update_command(*check, cli.output).await?;
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
            } else {
                let model = cli
                    .model
                    .clone()
                    .unwrap_or_else(|| config.models.main.clone());
                if cli.headless || cli.output == OutputFormat::Json {
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
                    let mut state =
                        TuiState::new(HeaderState::new(mode, permission, model.clone()));
                    state.push_transcript("Peridot ready. Type a task, /plan, /execute, /goal <objective>, /safe, /auto, /yolo, or Esc.");
                    if let Some(task) = run_interactive(state)?.submitted {
                        run_task(task, mode, &cli, &config, &project_root).await?;
                    }
                }
            }
        }
    }

    Ok(())
}

async fn run_task(
    task: String,
    mode: ExecutionMode,
    cli: &Cli,
    config: &PeridotConfig,
    project_root: &Path,
) -> Result<()> {
    let permission = cli
        .permission
        .map(PermissionMode::from)
        .unwrap_or(config.defaults.permission);
    let model = cli
        .model
        .clone()
        .unwrap_or_else(|| config.models.main.clone());
    let state = if mode == ExecutionMode::Goal {
        AgentState::new(mode, permission).with_goal(task.clone())
    } else {
        AgentState::new(mode, permission)
    };
    let mut registry = ToolRegistry::new();
    register_builtin_tools(&mut registry)?;
    register_configured_mcp_tools(&mut registry, config).await?;
    let context = ContextManager::with_limits(project_context_limits(project_root));
    let mut agent = HarnessAgent::new(state, context, registry);
    let call = task_to_tool_call(&task);

    if let Some(mock_response_file) = &cli.mock_response_file {
        let profile = ProjectScanner::new().scan(project_root)?;
        let denied_paths = profile.boundaries.into_iter().map(PathBuf::from).collect();
        let provider = FileMockProvider::from_file(mock_response_file)?;
        let summary = run_agent_loop(
            &mut agent,
            &provider,
            task,
            model,
            config,
            project_root,
            denied_paths,
        )
        .await?;
        if cli.output == OutputFormat::Json || cli.headless {
            println!("{}", serde_json::to_string_pretty(&summary)?);
        } else {
            println!(
                "stopped={:?} turns={}",
                summary.stopped_reason,
                summary.turns.len()
            );
        }
        return Ok(());
    }

    if cli.live {
        let profile = ProjectScanner::new().scan(project_root)?;
        let denied_paths = profile.boundaries.into_iter().map(PathBuf::from).collect();
        let provider = live_provider(config, &model)?;
        let summary = run_agent_loop(
            &mut agent,
            provider.as_ref(),
            task,
            model,
            config,
            project_root,
            denied_paths,
        )
        .await?;
        if cli.output == OutputFormat::Json || cli.headless {
            println!("{}", serde_json::to_string_pretty(&summary)?);
        } else {
            println!(
                "stopped={:?} turns={} cost=${:.6}",
                summary.stopped_reason,
                summary.turns.len(),
                summary.usage.estimated_cost_usd
            );
        }
        return Ok(());
    }

    match call {
        Some(call) => {
            let profile = ProjectScanner::new().scan(project_root)?;
            let denied_paths = profile.boundaries.into_iter().map(PathBuf::from).collect();
            let result = agent
                .execute_tool_call_with_runtime(
                    call,
                    project_root,
                    denied_paths,
                    config.hooks.clone(),
                )
                .await?;
            if cli.output == OutputFormat::Json || cli.headless {
                println!("{}", serde_json::to_string_pretty(&result)?);
            } else {
                println!("{}", result.summary);
            }
        }
        None => {
            if cli.output == OutputFormat::Json || cli.headless {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&serde_json::json!({
                        "status": "needs_llm",
                        "mode": agent.state().mode,
                        "permission": agent.state().permission,
                        "model": model,
                        "task": task
                    }))?
                );
            } else {
                println!(
                    "Hello Peridot: mode={} permission={} model={} task={}",
                    agent.state().mode,
                    agent.state().permission,
                    model,
                    task
                );
                println!(
                    "Use --live for a model run, --mock-response-file for deterministic replay, or enter a JSON tool action."
                );
            }
        }
    }
    Ok(())
}

async fn register_configured_mcp_tools(
    registry: &mut ToolRegistry,
    config: &PeridotConfig,
) -> Result<()> {
    for server in &config.mcp {
        let tools = McpClient::new(server.clone()).list_tools().await?;
        register_mcp_tools(registry, server.clone(), tools)?;
    }
    Ok(())
}

async fn run_agent_loop<P>(
    agent: &mut HarnessAgent,
    provider: &P,
    task: String,
    model: String,
    config: &PeridotConfig,
    project_root: &Path,
    denied_paths: Vec<PathBuf>,
) -> Result<peridot_core::AgentRunSummary>
where
    P: LlmProvider + ?Sized,
{
    Ok(agent
        .run_until_done(
            provider,
            AgentRunRequest {
                task,
                model,
                max_turns: config.defaults.max_turns,
                max_tokens: 4096,
                project_root: project_root.to_path_buf(),
                denied_paths,
                hooks: config.hooks.clone(),
            },
        )
        .await?)
}

fn live_provider(config: &PeridotConfig, model: &str) -> Result<Box<dyn LlmProvider>> {
    match config.auth.primary.as_str() {
        "claude-api" => {
            let api_key = std::env::var("ANTHROPIC_API_KEY")
                .ok()
                .or_else(|| read_stored_api_key(AuthProvider::ClaudeApi).ok().flatten())
                .with_context(
                    || "ANTHROPIC_API_KEY or peridot login claude-api is required for --live",
                )?;
            Ok(Box::new(ClaudeProvider::with_options(
                model.to_string(),
                Some(api_key),
                config.api.base_url.clone(),
            )))
        }
        "openai-api" => {
            let api_key = std::env::var("OPENAI_API_KEY")
                .ok()
                .or_else(|| read_stored_api_key(AuthProvider::OpenaiApi).ok().flatten())
                .with_context(
                    || "OPENAI_API_KEY or peridot login openai-api is required for --live",
                )?;
            let base_url = if config.api.base_url == "https://api.anthropic.com" {
                "https://api.openai.com".to_string()
            } else {
                config.api.base_url.clone()
            };
            Ok(Box::new(OpenAiProvider::with_options(
                model.to_string(),
                Some(api_key),
                base_url,
                AuthMethod::ApiKey,
            )))
        }
        "openai-oauth" => {
            let access_token = std::env::var("OPENAI_ACCESS_TOKEN")
                .ok()
                .or_else(|| read_stored_openai_oauth_access_token().ok().flatten())
                .with_context(
                    || "OPENAI_ACCESS_TOKEN or peridot login openai-oauth is required for --live",
                )?;
            let base_url = if config.api.base_url == "https://api.anthropic.com" {
                "https://api.openai.com".to_string()
            } else {
                config.api.base_url.clone()
            };
            Ok(Box::new(OpenAiProvider::with_options(
                model.to_string(),
                Some(access_token),
                base_url,
                AuthMethod::OAuth,
            )))
        }
        provider => anyhow::bail!(
            "live provider {provider} is not implemented yet; use claude-api, openai-api, openai-oauth, or --mock-response-file"
        ),
    }
}

struct FileMockProvider {
    responses: std::sync::Mutex<Vec<String>>,
}

impl FileMockProvider {
    fn from_file(path: &Path) -> Result<Self> {
        let content = fs::read_to_string(path)
            .with_context(|| format!("failed to read {}", path.display()))?;
        let responses = content
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .map(str::to_string)
            .rev()
            .collect();
        Ok(Self {
            responses: std::sync::Mutex::new(responses),
        })
    }
}

#[async_trait]
impl LlmProvider for FileMockProvider {
    async fn complete(&self, _request: CompletionRequest) -> PeriResult<CompletionResponse> {
        let text = self
            .responses
            .lock()
            .unwrap()
            .pop()
            .ok_or_else(|| PeriError::Provider("mock response file exhausted".to_string()))?;
        Ok(CompletionResponse {
            text,
            usage: Usage::default(),
        })
    }

    fn supports_cache(&self) -> bool {
        false
    }

    fn supports_prefill(&self) -> bool {
        false
    }

    fn supports_thinking(&self) -> bool {
        false
    }

    fn pricing(&self) -> PricingTable {
        PricingTable::default()
    }

    fn auth_method(&self) -> AuthMethod {
        AuthMethod::NotConfigured
    }
}

fn task_to_tool_call(task: &str) -> Option<ToolCall> {
    if let Ok(parsed) = parse_action(task) {
        return Some(parsed.tool_call);
    }

    let lower = task.to_lowercase();
    if lower.contains("hello.py") && lower.contains("hello world") {
        return Some(ToolCall::new(
            "file_write",
            serde_json::json!({
                "path": "hello.py",
                "content": "print(\"Hello World\")\n"
            }),
        ));
    }

    None
}

fn read_piped_task() -> Result<Option<String>> {
    let stdin = std::io::stdin();
    if stdin.is_terminal() {
        return Ok(None);
    }
    let mut task = String::new();
    stdin.lock().read_to_string(&mut task)?;
    let task = task.trim().to_string();
    if task.is_empty() {
        Ok(None)
    } else {
        Ok(Some(task))
    }
}
