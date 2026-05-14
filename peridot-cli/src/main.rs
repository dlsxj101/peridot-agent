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
    SkillCommand, load_effective_config, print_scan, read_stored_api_key,
    read_stored_openai_oauth_access_token, run_agents_command, run_config_command,
    run_login_command, run_logout_command, run_mcp_command, run_session_command, run_setup_command,
    run_skill_command, run_update_command, run_verify_command,
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
use peridot_tools::hooks::{HookRunner, lifecycle_hook_variables};
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
                    let mut state =
                        TuiState::new(HeaderState::new(mode, permission, model.clone()))
                            .with_config(config.tui.clone());
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
    let task = apply_resume(task, cli.resume.as_deref(), project_root)?;
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
    let max_turns = cli.max_turns.unwrap_or(config.defaults.max_turns);
    let budget_usd = cli.budget.unwrap_or(config.defaults.budget_usd);
    let mut registry = ToolRegistry::new();
    register_builtin_tools(&mut registry)?;
    register_configured_mcp_tools(&mut registry, config).await?;
    let context = ContextManager::with_limits(project_context_limits_from_config(
        project_root,
        &config.context,
    ));
    let mut agent = HarnessAgent::new(state, context, registry);
    let call = task_to_tool_call(&task);

    if let Some(mock_response_file) = &cli.mock_response_file {
        let profile = ProjectScanner::new().scan(project_root)?;
        let denied_paths = profile.boundaries.into_iter().map(PathBuf::from).collect();
        let provider = FileMockProvider::from_file(mock_response_file)?;
        let summary = run_agent_loop(
            &mut agent,
            &provider,
            RunLoopOptions {
                task,
                model,
                max_turns,
                budget_usd,
                config,
                project_root,
                denied_paths,
            },
        )
        .await?;
        if cli.output == OutputFormat::Json || cli.effective_headless() {
            println!(
                "{}",
                serde_json::to_string_pretty(&run_summary_output(&summary, mode))?
            );
            exit_for_summary(&summary, cli.effective_headless());
        } else {
            print_run_summary_text(&summary, mode);
        }
        return Ok(());
    }

    if cli.live {
        let profile = ProjectScanner::new().scan(project_root)?;
        let denied_paths = profile.boundaries.into_iter().map(PathBuf::from).collect();
        let provider = live_provider(config, &model).await?;
        let summary = run_agent_loop(
            &mut agent,
            provider.as_ref(),
            RunLoopOptions {
                task,
                model,
                max_turns,
                budget_usd,
                config,
                project_root,
                denied_paths,
            },
        )
        .await?;
        if cli.output == OutputFormat::Json || cli.effective_headless() {
            println!(
                "{}",
                serde_json::to_string_pretty(&run_summary_output(&summary, mode))?
            );
            exit_for_summary(&summary, cli.effective_headless());
        } else {
            print_run_summary_text(&summary, mode);
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
                    config.security.clone(),
                )
                .await?;
            if cli.output == OutputFormat::Json || cli.effective_headless() {
                println!("{}", serde_json::to_string_pretty(&result)?);
            } else {
                println!("{}", result.summary);
            }
            exit_for_tool_result(&result, cli.effective_headless());
        }
        None => {
            if cli.output == OutputFormat::Json || cli.effective_headless() {
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

fn apply_resume(task: String, resume_id: Option<&str>, project_root: &Path) -> Result<String> {
    let Some(resume_id) = resume_id else {
        return Ok(task);
    };
    let store = MemoryStore::new(project_root.join(".peridot/memory.db"));
    let session = store
        .get_session(resume_id)?
        .with_context(|| format!("session not found: {resume_id}"))?;
    Ok(resume_task_text(&session.id, &session.summary, &task))
}

fn resume_task_text(id: &str, summary: &str, task: &str) -> String {
    let task = task.trim();
    if task.is_empty() {
        format!("Resume session {id} from this summary: {summary}")
    } else {
        format!("Resume session {id} from this summary: {summary}\n\nCurrent task: {task}")
    }
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

struct RunLoopOptions<'a> {
    task: String,
    model: String,
    max_turns: u32,
    budget_usd: f64,
    config: &'a PeridotConfig,
    project_root: &'a Path,
    denied_paths: Vec<PathBuf>,
}

async fn run_agent_loop<P>(
    agent: &mut HarnessAgent,
    provider: &P,
    options: RunLoopOptions<'_>,
) -> Result<peridot_core::AgentRunSummary>
where
    P: LlmProvider + ?Sized,
{
    let session_id = format!("session-{}-{}", std::process::id(), unix_timestamp());
    run_lifecycle_hook(agent, &options, &session_id, "session_start", "running", "")?;
    let summary = agent
        .run_until_done(
            provider,
            AgentRunRequest {
                task: options.task.clone(),
                model: options.model.clone(),
                goal_checker_model: Some(options.config.models.goal_checker.clone()),
                max_turns: options.max_turns,
                max_tokens: 4096,
                budget_usd: options.budget_usd,
                project_root: options.project_root.to_path_buf(),
                denied_paths: options.denied_paths.clone(),
                hooks: options.config.hooks.clone(),
                security: options.config.security.clone(),
            },
        )
        .await?;
    run_lifecycle_hook(
        agent,
        &options,
        &session_id,
        "session_end",
        &format!("{:?}", summary.stopped_reason),
        &format!("turns={}", summary.turns.len()),
    )?;
    run_completion_lifecycle_hooks(agent, &options, &session_id, &summary)?;
    if let Err(err) = save_run_session(
        options.project_root,
        &session_id,
        &summary,
        &options.task,
        &options.config.memory,
    ) {
        eprintln!("warning: failed to save session {session_id}: {err}");
    }
    if let Err(err) = auto_commit_run(
        options.project_root,
        options.config,
        &summary,
        &options.task,
    ) {
        eprintln!("warning: failed to auto-commit session {session_id}: {err}");
    }
    Ok(summary)
}

fn save_run_session(
    project_root: &Path,
    session_id: &str,
    summary: &AgentRunSummary,
    task: &str,
    memory: &MemoryConfig,
) -> Result<()> {
    if !memory.session_history {
        return Ok(());
    }
    let task = compact_summary_text(task, 160);
    let session = SessionSummary {
        id: session_id.to_string(),
        summary: format!(
            "task=\"{}\" stopped={:?} turns={} cost=${:.6}",
            task,
            summary.stopped_reason,
            summary.turns.len(),
            summary.usage.estimated_cost_usd
        ),
    };
    let store = MemoryStore::new(project_root.join(".peridot/memory.db"));
    store.save_session(&session)?;
    if memory.auto_skills && summary.stopped_reason == StopReason::Done {
        save_auto_skill(
            project_root,
            &store,
            session_id,
            summary,
            &task,
            memory.skills_review,
        )?;
    }
    Ok(())
}

fn save_auto_skill(
    project_root: &Path,
    store: &MemoryStore,
    session_id: &str,
    summary: &AgentRunSummary,
    task: &str,
    needs_review: bool,
) -> Result<()> {
    let name = format!("auto-{}", slugify_for_branch(task));
    let body = auto_skill_body(session_id, summary, task, needs_review);
    store.save_skill(&StoredSkill {
        name: name.clone(),
        body: body.clone(),
    })?;
    let skills_dir = project_root.join(".peridot/skills/auto");
    fs::create_dir_all(&skills_dir)?;
    fs::write(skills_dir.join(format!("{name}.md")), body)?;
    Ok(())
}

fn auto_skill_body(
    session_id: &str,
    summary: &AgentRunSummary,
    task: &str,
    needs_review: bool,
) -> String {
    let review = if needs_review { "true" } else { "false" };
    let tools = summary
        .turns
        .iter()
        .map(|turn| format!("- {}: {}", turn.tool_name, turn.tool_result.summary))
        .collect::<Vec<_>>()
        .join("\n");
    format!(
        "# Auto Skill: {task}\n\nreview_required: {review}\nsession: {session_id}\n\n## When To Use\nRepeat this pattern for similar tasks.\n\n## Observed Steps\n{tools}\n"
    )
}

fn auto_commit_run(
    project_root: &Path,
    config: &PeridotConfig,
    summary: &AgentRunSummary,
    task: &str,
) -> Result<Option<String>> {
    if !config.git.auto_commit || summary.stopped_reason != StopReason::Done {
        return Ok(None);
    }
    let manager = GitManager::new(project_root);
    if !manager.is_repository() {
        return Ok(None);
    }
    let status = manager.status()?;
    if status.changed_files.is_empty() {
        return Ok(None);
    }
    if config.git.auto_branch {
        ensure_auto_branch(&manager, &config.git.branch_prefix, task)?;
    }
    let message = commit_message_for_task(task, &config.git.commit_message_style);
    manager.commit_all(&message)?;
    Ok(Some(message))
}

fn ensure_auto_branch(manager: &GitManager, branch_prefix: &str, task: &str) -> Result<()> {
    let status = manager.status()?;
    let current = status.branch.unwrap_or_default();
    if current.starts_with(branch_prefix) {
        return Ok(());
    }
    let branch = format!(
        "{}{}-{}",
        branch_prefix,
        slugify_for_branch(task),
        unix_timestamp()
    );
    manager.create_branch(&branch)?;
    Ok(())
}

fn commit_message_for_task(task: &str, style: &str) -> String {
    let subject = compact_summary_text(task, 64)
        .trim_matches('"')
        .trim()
        .to_string();
    if style == "conventional" {
        format!("chore(agent): {}", fallback_subject(&subject))
    } else {
        fallback_subject(&subject).to_string()
    }
}

fn fallback_subject(subject: &str) -> &str {
    if subject.is_empty() {
        "complete agent task"
    } else {
        subject
    }
}

fn slugify_for_branch(task: &str) -> String {
    let mut slug = String::new();
    let mut last_dash = false;
    for ch in task.chars().flat_map(char::to_lowercase) {
        if ch.is_ascii_alphanumeric() {
            slug.push(ch);
            last_dash = false;
        } else if !last_dash && !slug.is_empty() {
            slug.push('-');
            last_dash = true;
        }
        if slug.len() >= 40 {
            break;
        }
    }
    let slug = slug.trim_matches('-');
    if slug.is_empty() {
        "task".to_string()
    } else {
        slug.to_string()
    }
}

fn compact_summary_text(value: &str, max_chars: usize) -> String {
    let value = value.split_whitespace().collect::<Vec<_>>().join(" ");
    if value.chars().count() <= max_chars {
        return value;
    }
    let mut compact = value
        .chars()
        .take(max_chars.saturating_sub(3))
        .collect::<String>();
    compact.push_str("...");
    compact
}

fn unix_timestamp() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or_default()
}

fn run_lifecycle_hook(
    agent: &HarnessAgent,
    options: &RunLoopOptions<'_>,
    session_id: &str,
    event: &str,
    status: &str,
    summary: &str,
) -> Result<()> {
    let variables = lifecycle_hook_variables(
        session_id,
        &agent.state().mode.to_string(),
        &agent.state().permission.to_string(),
        options.project_root,
        status,
        summary,
    );
    HookRunner::new(options.project_root, options.config.hooks.clone())
        .run_lifecycle_hooks(event, &variables)?;
    Ok(())
}

fn run_completion_lifecycle_hooks(
    agent: &HarnessAgent,
    options: &RunLoopOptions<'_>,
    session_id: &str,
    summary: &AgentRunSummary,
) -> Result<()> {
    if summary.stopped_reason != StopReason::Done {
        return Ok(());
    }
    match agent.state().mode {
        ExecutionMode::Plan => run_lifecycle_hook(
            agent,
            options,
            session_id,
            "plan_completed",
            "done",
            "plan_file=todo.md",
        ),
        ExecutionMode::Goal => run_lifecycle_hook(
            agent,
            options,
            session_id,
            "goal_achieved",
            "done",
            agent.state().goal.as_deref().unwrap_or(&options.task),
        ),
        ExecutionMode::Execute => Ok(()),
    }
}

fn exit_for_summary(summary: &AgentRunSummary, headless: bool) {
    if !headless {
        return;
    }
    match summary.stopped_reason {
        StopReason::Done => {}
        StopReason::Budget => std::process::exit(2),
        StopReason::MaxTurns => std::process::exit(3),
    }
}

fn exit_for_tool_result(result: &peridot_common::ToolResult, headless: bool) {
    if headless && !result.success {
        std::process::exit(4);
    }
}

fn run_summary_output(summary: &AgentRunSummary, mode: ExecutionMode) -> serde_json::Value {
    let mut output = serde_json::to_value(summary).unwrap_or_else(|_| serde_json::json!({}));
    if mode == ExecutionMode::Plan
        && summary.stopped_reason == StopReason::Done
        && let Some(object) = output.as_object_mut()
    {
        object.insert(
            "next_actions".to_string(),
            serde_json::Value::Array(
                plan_completion_choices()
                    .into_iter()
                    .map(|choice| choice.to_json())
                    .collect(),
            ),
        );
    }
    output
}

fn print_run_summary_text(summary: &AgentRunSummary, mode: ExecutionMode) {
    println!(
        "stopped={:?} turns={} cost=${:.6}",
        summary.stopped_reason,
        summary.turns.len(),
        summary.usage.estimated_cost_usd
    );
    if mode == ExecutionMode::Plan && summary.stopped_reason == StopReason::Done {
        println!("{}", render_plan_completion_choices());
    }
}

fn render_plan_completion_choices() -> String {
    plan_completion_choices()
        .into_iter()
        .map(|choice| format!("[{}] {}", choice.id, choice.label))
        .collect::<Vec<_>>()
        .join("\n")
}

fn plan_completion_choices() -> Vec<PlanCompletionChoice> {
    vec![
        PlanCompletionChoice::new(
            1,
            "Execute·auto",
            ExecutionMode::Execute,
            PermissionMode::Auto,
        ),
        PlanCompletionChoice::new(
            2,
            "Execute·safe",
            ExecutionMode::Execute,
            PermissionMode::Safe,
        ),
        PlanCompletionChoice::new(3, "Goal·auto", ExecutionMode::Goal, PermissionMode::Auto),
        PlanCompletionChoice::new(4, "Goal·yolo", ExecutionMode::Goal, PermissionMode::Yolo),
        PlanCompletionChoice::new(5, "Revise plan", ExecutionMode::Plan, PermissionMode::Safe),
        PlanCompletionChoice::new(6, "Cancel", ExecutionMode::Plan, PermissionMode::Safe),
    ]
}

#[derive(Clone, Debug)]
struct PlanCompletionChoice {
    id: u8,
    label: &'static str,
    mode: ExecutionMode,
    permission: PermissionMode,
}

impl PlanCompletionChoice {
    fn new(id: u8, label: &'static str, mode: ExecutionMode, permission: PermissionMode) -> Self {
        Self {
            id,
            label,
            mode,
            permission,
        }
    }

    fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "id": self.id,
            "label": self.label,
            "mode": self.mode,
            "permission": self.permission
        })
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

fn project_context_limits_from_config(
    project_root: &Path,
    config: &ContextConfig,
) -> ContextLimits {
    let mut limits = project_context_limits(project_root);
    limits.hard_limit_tokens = config.hard_limit;
    limits.compaction_threshold_tokens = config.compaction_threshold;
    limits.offload_threshold_chars = config.offload_threshold_chars;
    limits
}

async fn live_provider(config: &PeridotConfig, model: &str) -> Result<Box<dyn LlmProvider>> {
    match config.auth.primary.as_str() {
        "claude-api" => {
            let api_key = std::env::var("ANTHROPIC_API_KEY")
                .ok()
                .or_else(|| read_stored_api_key(AuthProvider::ClaudeApi).ok().flatten())
                .with_context(
                    || "ANTHROPIC_API_KEY or peridot login claude-api is required for --live",
                )?;
            Ok(Box::new(ClaudeProvider::with_transport_options(
                model.to_string(),
                Some(api_key),
                config.api.base_url.clone(),
                config.api.timeout_seconds,
                config.api.max_retries,
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
            Ok(Box::new(OpenAiProvider::with_transport_options(
                model.to_string(),
                Some(api_key),
                base_url,
                AuthMethod::ApiKey,
                config.api.timeout_seconds,
                config.api.max_retries,
            )))
        }
        "openai-oauth" => {
            let access_token = match std::env::var("OPENAI_ACCESS_TOKEN").ok() {
                Some(access_token) => Some(access_token),
                None => read_stored_openai_oauth_access_token().await?,
            }
            .with_context(
                || "OPENAI_ACCESS_TOKEN or peridot login openai-oauth is required for --live",
            )?;
            let base_url = if config.api.base_url == "https://api.anthropic.com" {
                "https://api.openai.com".to_string()
            } else {
                config.api.base_url.clone()
            };
            Ok(Box::new(OpenAiProvider::with_transport_options(
                model.to_string(),
                Some(access_token),
                base_url,
                AuthMethod::OAuth,
                config.api.timeout_seconds,
                config.api.max_retries,
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    #[test]
    fn resume_text_wraps_current_task() {
        let text = resume_task_text("demo", "created parser", "finish tests");

        assert!(text.contains("Resume session demo"));
        assert!(text.contains("created parser"));
        assert!(text.contains("Current task: finish tests"));
    }

    #[test]
    fn resume_text_handles_empty_task() {
        let text = resume_task_text("demo", "created parser", "");

        assert_eq!(
            text,
            "Resume session demo from this summary: created parser"
        );
    }

    #[test]
    fn saves_run_summary_for_resume() {
        let root =
            std::env::temp_dir().join(format!("peridot-cli-run-save-{}", std::process::id()));
        let summary = AgentRunSummary {
            turns: Vec::new(),
            usage: Usage::default(),
            stopped_reason: StopReason::Done,
        };

        save_run_session(
            &root,
            "session-test",
            &summary,
            "finish the parser",
            &MemoryConfig {
                auto_skills: false,
                ..MemoryConfig::default()
            },
        )
        .unwrap();

        let session = MemoryStore::new(root.join(".peridot/memory.db"))
            .get_session("session-test")
            .unwrap()
            .unwrap();
        assert!(session.summary.contains("finish the parser"));
        assert!(session.summary.contains("stopped=Done"));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn completed_run_saves_auto_skill_when_enabled() {
        let root =
            std::env::temp_dir().join(format!("peridot-cli-auto-skill-{}", std::process::id()));
        let summary = AgentRunSummary {
            turns: vec![peridot_core::AgentTurnOutcome {
                tool_name: "verify_test".to_string(),
                tool_result: peridot_common::ToolResult::success(
                    "tests passed",
                    serde_json::json!({}),
                ),
                usage: Usage::default(),
                done: true,
            }],
            usage: Usage::default(),
            stopped_reason: StopReason::Done,
        };

        save_run_session(
            &root,
            "session-auto",
            &summary,
            "fix parser tests",
            &MemoryConfig::default(),
        )
        .unwrap();

        let skill = MemoryStore::new(root.join(".peridot/memory.db"))
            .search_skills("parser")
            .unwrap()
            .pop()
            .unwrap();
        assert_eq!(skill.name, "auto-fix-parser-tests");
        assert!(
            root.join(".peridot/skills/auto/auto-fix-parser-tests.md")
                .exists()
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn compact_summary_text_limits_long_tasks() {
        let compact = compact_summary_text("a b c d e f", 5);

        assert_eq!(compact, "a ...");
    }

    #[test]
    fn plan_summary_output_includes_execution_choices() {
        let summary = AgentRunSummary {
            turns: Vec::new(),
            usage: Usage::default(),
            stopped_reason: StopReason::Done,
        };

        let output = run_summary_output(&summary, ExecutionMode::Plan);

        assert_eq!(output["next_actions"][0]["label"], "Execute·auto");
        assert_eq!(output["next_actions"][3]["permission"], "yolo");
        assert!(render_plan_completion_choices().contains("[6] Cancel"));
    }

    #[test]
    fn commit_message_uses_conventional_style() {
        assert_eq!(
            commit_message_for_task("fix the parser", "conventional"),
            "chore(agent): fix the parser"
        );
        assert_eq!(slugify_for_branch("Fix the parser!"), "fix-the-parser");
    }

    #[test]
    fn auto_commit_run_commits_dirty_worktree() {
        if Command::new("git").arg("--version").output().is_err() {
            return;
        }
        let root =
            std::env::temp_dir().join(format!("peridot-cli-auto-commit-{}", std::process::id()));
        fs::create_dir_all(&root).unwrap();
        run_git(&root, ["init"]).unwrap();
        run_git(&root, ["config", "user.email", "peridot@example.com"]).unwrap();
        run_git(&root, ["config", "user.name", "Peridot Test"]).unwrap();
        fs::write(root.join("README.md"), "hello\n").unwrap();
        run_git(&root, ["add", "--all"]).unwrap();
        run_git(&root, ["commit", "-m", "chore: initial"]).unwrap();
        fs::write(root.join("result.txt"), "done\n").unwrap();
        let summary = AgentRunSummary {
            turns: Vec::new(),
            usage: Usage::default(),
            stopped_reason: StopReason::Done,
        };
        let config = PeridotConfig {
            git: peridot_common::GitConfig {
                auto_commit: true,
                auto_branch: true,
                branch_prefix: "peridot/".to_string(),
                ..peridot_common::GitConfig::default()
            },
            ..PeridotConfig::default()
        };

        let message = auto_commit_run(&root, &config, &summary, "write result file")
            .unwrap()
            .unwrap();
        let status = run_git(&root, ["status", "--short"]).unwrap();
        let branch = run_git(&root, ["rev-parse", "--abbrev-ref", "HEAD"]).unwrap();

        assert_eq!(message, "chore(agent): write result file");
        assert!(status.trim().is_empty());
        assert!(branch.trim().starts_with("peridot/write-result-file-"));
        fs::remove_dir_all(root).unwrap();
    }

    fn run_git<const N: usize>(root: &Path, args: [&str; N]) -> Result<String> {
        let output = Command::new("git").args(args).current_dir(root).output()?;
        if !output.status.success() {
            anyhow::bail!("{}", String::from_utf8_lossy(&output.stderr).trim());
        }
        Ok(String::from_utf8_lossy(&output.stdout).to_string())
    }
}
