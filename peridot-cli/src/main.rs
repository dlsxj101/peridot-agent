//! Peridot command-line entrypoint.

use std::fs;
use std::io::{IsTerminal, Read};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use async_trait::async_trait;
use clap::{Parser, Subcommand, ValueEnum};
use peridot_common::{
    ExecutionMode, PeriError, PeriResult, PeridotConfig, PermissionMode, ToolCall,
};
use peridot_context::{ContextManager, project_context_limits};
use peridot_core::{AgentRunRequest, AgentState, HarnessAgent};
use peridot_llm::{
    AuthMethod, CompletionRequest, CompletionResponse, LlmProvider, PricingTable, Usage,
    parse_action,
};
use peridot_memory::{MemoryStore, SessionSummary};
use peridot_project::{ProjectProfile, ProjectScanner};
use peridot_tools::{ToolRegistry, register_builtin_tools};

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
    /// Print version information.
    Version,
}

/// Config subcommands.
#[derive(Debug, Subcommand)]
enum ConfigCommand {
    /// Initialize project-local Peridot config.
    Init,
    /// Print the effective config.
    Show,
}

/// Session subcommands.
#[derive(Debug, Subcommand)]
enum SessionCommand {
    /// List saved sessions.
    List,
    /// Save a session summary.
    Save {
        /// Session id.
        id: String,
        /// Summary text.
        summary: Vec<String>,
    },
    /// Show one session summary.
    Show {
        /// Session id.
        id: String,
    },
    /// Delete one session summary.
    Delete {
        /// Session id.
        id: String,
    },
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

/// Scriptable output format.
#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
enum OutputFormat {
    /// Human-readable text.
    Text,
    /// JSON.
    Json,
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
        Some(Command::Config { command }) => {
            run_config_command(command, &config, &project_root, cli.output)?;
            return Ok(());
        }
        Some(Command::Session { command }) => {
            run_session_command(command, &project_root, cli.output)?;
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
                println!(
                    "Hello Peridot: mode={} permission={} model={}",
                    mode, permission, model
                );
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
    let context = ContextManager::with_limits(project_context_limits(project_root));
    let mut agent = HarnessAgent::new(state, context, registry);
    let call = task_to_tool_call(&task);

    if let Some(mock_response_file) = &cli.mock_response_file {
        let profile = ProjectScanner::new().scan(project_root)?;
        let denied_paths = profile.boundaries.into_iter().map(PathBuf::from).collect();
        let provider = FileMockProvider::from_file(mock_response_file)?;
        let summary = agent
            .run_until_done(
                &provider,
                AgentRunRequest {
                    task,
                    model,
                    max_turns: config.defaults.max_turns,
                    max_tokens: 4096,
                    project_root: project_root.to_path_buf(),
                    denied_paths,
                },
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

    match call {
        Some(call) => {
            let profile = ProjectScanner::new().scan(project_root)?;
            let denied_paths = profile.boundaries.into_iter().map(PathBuf::from).collect();
            let result = agent
                .execute_tool_call_with_denied_paths(call, project_root, denied_paths)
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
                    "No live LLM provider is wired yet; try a JSON tool action or ask for hello.py."
                );
            }
        }
    }
    Ok(())
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

fn load_project_config(project_root: &Path) -> Result<PeridotConfig> {
    let path = project_root.join(".peridot/config.toml");
    if !path.exists() {
        return Ok(PeridotConfig::default());
    }
    let content =
        fs::read_to_string(&path).with_context(|| format!("failed to read {}", path.display()))?;
    let config = toml::from_str::<PeridotConfig>(&content)
        .with_context(|| format!("failed to parse {}", path.display()))?;
    Ok(config)
}

fn memory_store(project_root: &Path) -> MemoryStore {
    MemoryStore::new(project_root.join(".peridot/memory.db"))
}

fn run_config_command(
    command: &ConfigCommand,
    config: &PeridotConfig,
    project_root: &Path,
    output: OutputFormat,
) -> Result<()> {
    match command {
        ConfigCommand::Init => init_project_config(project_root, output),
        ConfigCommand::Show => print_json_or_text(config, output),
    }
}

fn init_project_config(project_root: &Path, output: OutputFormat) -> Result<()> {
    let peridot_dir = project_root.join(".peridot");
    fs::create_dir_all(peridot_dir.join("hooks"))?;
    fs::create_dir_all(peridot_dir.join("skills"))?;
    let config_path = peridot_dir.join("config.toml");
    let created_config = if config_path.exists() {
        false
    } else {
        let config = toml::to_string_pretty(&PeridotConfig::default())?;
        fs::write(&config_path, config)?;
        true
    };
    let gitignore_path = project_root.join(".gitignore");
    let managed_entries = [
        ".peridot/memory.db",
        ".peridot/mem/",
        ".peridot/sessions/",
        ".peridot/skills/auto/",
        ".peridot/logs/",
    ];
    let mut gitignore = fs::read_to_string(&gitignore_path).unwrap_or_default();
    let mut changed_gitignore = false;
    for entry in managed_entries {
        if !gitignore.lines().any(|line| line.trim() == entry) {
            if !gitignore.ends_with('\n') && !gitignore.is_empty() {
                gitignore.push('\n');
            }
            gitignore.push_str(entry);
            gitignore.push('\n');
            changed_gitignore = true;
        }
    }
    if changed_gitignore {
        fs::write(&gitignore_path, gitignore)?;
    }
    print_json_or_text_result(
        serde_json::json!({
            "config_path": config_path,
            "created_config": created_config,
            "updated_gitignore": changed_gitignore
        }),
        format!(
            "initialized {} (created_config={created_config}, updated_gitignore={changed_gitignore})",
            peridot_dir.display()
        ),
        output,
    )
}

fn run_session_command(
    command: &SessionCommand,
    project_root: &Path,
    output: OutputFormat,
) -> Result<()> {
    let store = memory_store(project_root);
    match command {
        SessionCommand::List => {
            let sessions = store.list_sessions()?;
            match output {
                OutputFormat::Json => println!("{}", serde_json::to_string_pretty(&sessions)?),
                OutputFormat::Text => {
                    for session in sessions {
                        println!("{}\t{}", session.id, session.summary);
                    }
                }
            }
        }
        SessionCommand::Save { id, summary } => {
            let session = SessionSummary {
                id: id.clone(),
                summary: summary.join(" "),
            };
            store.save_session(&session)?;
            print_json_or_text_result(
                serde_json::json!({"saved": true, "id": id}),
                format!("saved session {id}"),
                output,
            )?;
        }
        SessionCommand::Show { id } => {
            let session = store.get_session(id)?;
            match output {
                OutputFormat::Json => println!("{}", serde_json::to_string_pretty(&session)?),
                OutputFormat::Text => match session {
                    Some(session) => println!("{}\t{}", session.id, session.summary),
                    None => println!("session not found: {id}"),
                },
            }
        }
        SessionCommand::Delete { id } => {
            let deleted = store.delete_session(id)?;
            print_json_or_text_result(
                serde_json::json!({"deleted": deleted, "id": id}),
                format!("deleted session {id}: {deleted}"),
                output,
            )?;
        }
    }
    Ok(())
}

fn print_scan(profile: &ProjectProfile, output: OutputFormat) -> Result<()> {
    match output {
        OutputFormat::Json => println!("{}", serde_json::to_string_pretty(profile)?),
        OutputFormat::Text => {
            println!("Project: {}", profile.name);
            println!("Root: {}", profile.root.display());
            println!("Build system: {:?}", profile.build_system);
            if !profile.languages.is_empty() {
                let languages = profile
                    .languages
                    .iter()
                    .map(|language| language.name.as_str())
                    .collect::<Vec<_>>()
                    .join(", ");
                println!("Languages: {languages}");
            }
            if let Some(build) = &profile.commands.build {
                println!("Build: {build}");
            }
            if let Some(test) = &profile.commands.test {
                println!("Test: {test}");
            }
            println!("AGENTS.md: {}", profile.has_agents_md);
        }
    }
    Ok(())
}

fn print_json_or_text(config: &PeridotConfig, output: OutputFormat) -> Result<()> {
    match output {
        OutputFormat::Json => println!("{}", serde_json::to_string_pretty(config)?),
        OutputFormat::Text => {
            println!("auth.primary = {}", config.auth.primary);
            println!("models.main = {}", config.models.main);
            println!("defaults.mode = {}", config.defaults.mode);
            println!("defaults.permission = {}", config.defaults.permission);
            println!("defaults.max_turns = {}", config.defaults.max_turns);
            println!("defaults.budget_usd = {}", config.defaults.budget_usd);
        }
    }
    Ok(())
}

fn print_json_or_text_result(
    value: serde_json::Value,
    text: String,
    output: OutputFormat,
) -> Result<()> {
    match output {
        OutputFormat::Json => println!("{}", serde_json::to_string_pretty(&value)?),
        OutputFormat::Text => println!("{text}"),
    }
    Ok(())
}
