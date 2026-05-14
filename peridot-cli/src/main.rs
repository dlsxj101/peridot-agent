//! Peridot command-line entrypoint.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use clap::{Parser, Subcommand, ValueEnum};
use peridot_common::{ExecutionMode, PeridotConfig, PermissionMode, ToolCall};
use peridot_context::ContextManager;
use peridot_core::{AgentState, HarnessAgent};
use peridot_llm::parse_action;
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
    /// Print version information.
    Version,
}

/// Config subcommands.
#[derive(Debug, Subcommand)]
enum ConfigCommand {
    /// Print the effective config.
    Show,
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
        Some(Command::Config {
            command: ConfigCommand::Show,
        }) => {
            print_json_or_text(&config, cli.output)?;
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
    let agent = HarnessAgent::new(state, ContextManager::new(), registry);
    let call = task_to_tool_call(&task);

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
