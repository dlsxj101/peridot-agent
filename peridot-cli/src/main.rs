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
use peridot_memory::{
    MemoryStore, SessionLifecycle, SessionRecord, SessionSummary, StoredSkill, save_session_blob,
};
use peridot_project::ProjectScanner;
use peridot_tools::hooks::{HookRunner, HookVariables, lifecycle_hook_variables};
use peridot_tools::{ToolRegistry, register_builtin_tools, register_mcp_tools};
use peridot_tui::{
    ApprovalDecision, HeaderState, SessionCommandEvent, SessionDirectoryItem, TuiRuntimeEvent,
    TuiState, run_interactive_with_events,
};

mod commands;
mod context_limits;
mod interactive_io;
mod providers;
mod run_loop;
mod run_output;
mod run_state;
mod session_router;
#[cfg(test)]
mod tests;

use context_limits::project_context_limits_from_config;
use interactive_io::{read_piped_task, run_tui_lifecycle_hooks};
use providers::{FileMockProvider, live_provider};
use run_loop::{agent_task_options, run_task, run_task_with_events};
use run_output::{exit_for_summary, print_run_summary_text, run_summary_output};
use run_state::{apply_resume, auto_commit_run, save_run_session, unix_timestamp};
use session_router::{SessionHandle, SessionRouter, WorkspaceIsolation};

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
    Version {
        /// Include build metadata (target triple, rustc fingerprint).
        #[arg(long)]
        detailed: bool,
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
        Some(Command::Version { detailed }) => {
            if *detailed {
                println!("peridot {}", env!("CARGO_PKG_VERSION"));
                println!("  target: {}", std::env::consts::OS);
                println!("  arch:   {}", std::env::consts::ARCH);
                if let Some(profile) = option_env!("PROFILE") {
                    println!("  profile: {profile}");
                }
            } else {
                println!("peridot {}", env!("CARGO_PKG_VERSION"));
            }
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
            } else if cli.resume.is_some()
                && (cli.effective_headless() || cli.output == OutputFormat::Json)
            {
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
                    let restored_state = cli
                        .resume
                        .as_deref()
                        .and_then(|id| restore_tui_state_from_disk(id, &project_root).ok());
                    let workspace_label = project_root
                        .file_name()
                        .and_then(|name| name.to_str())
                        .map(|name| name.to_string());
                    let mut state = match restored_state {
                        Some((id, restored)) => {
                            let mut state = restored.with_config(config.tui.clone());
                            state.header.workspace_label = workspace_label.clone();
                            state.committee_mode = config.committee.mode;
                            state.push_notice(format!("session: resumed {id} from disk"));
                            state
                        }
                        None => {
                            let mut header = HeaderState::new(mode, permission, model.clone());
                            header.workspace_label = workspace_label.clone();
                            let mut state = TuiState::new(header).with_config(config.tui.clone());
                            state.committee_mode = config.committee.mode;
                            state.push_transcript("Peridot ready. Type a task, /plan, /execute, /goal <objective>, /safe, /auto, /yolo, or Esc.");
                            state.push_transcript(
                                "Submitted tasks continue inside this TUI; tool activity and run status stream here.",
                            );
                            state
                        }
                    };
                    let suspended = scan_and_suspend_running_sessions(&project_root);
                    if !suspended.is_empty() {
                        state.push_notice(format!(
                            "found {} stale session(s) marked Suspended: {}. \
                             Resume with `peridot --resume <id>`.",
                            suspended.len(),
                            suspended.join(", ")
                        ));
                    }
                    let router: std::sync::Arc<std::sync::Mutex<SessionRouter>> =
                        std::sync::Arc::new(std::sync::Mutex::new(SessionRouter::new()));
                    let initial_session_id = if state.current_session_id.is_empty() {
                        let new_id = format!("session-{}-{}", std::process::id(), unix_timestamp());
                        state.current_session_id = new_id.clone();
                        state
                            .sessions
                            .push(SessionDirectoryItem::new(&new_id, "main"));
                        new_id
                    } else {
                        state.current_session_id.clone()
                    };
                    router.lock().unwrap().register(SessionHandle::new(
                        initial_session_id.clone(),
                        project_root.clone(),
                        WorkspaceIsolation::Shared,
                    ));
                    let (event_tx, event_rx) =
                        std::sync::mpsc::channel::<(String, TuiRuntimeEvent)>();
                    let handle = tokio::runtime::Handle::current();
                    let base_options = agent_task_options(&cli, &config);
                    let run_config = config.clone();
                    let run_project_root = project_root.clone();
                    let exit = run_interactive_with_events(
                        state,
                        event_rx,
                        {
                            let event_tx = event_tx.clone();
                            let handle = handle.clone();
                            let router = router.clone();
                            let options_template = base_options.clone();
                            let config_template = run_config.clone();
                            let project_template = run_project_root.clone();
                            move |task, state| {
                                let foreground = state.current_session_id.clone();
                                let mut options = options_template.clone();
                                options.permission = state.header.permission;
                                options.model = state.header.model.clone();
                                let token = peridot_core::CancelToken::new();
                                {
                                    let mut router = router.lock().unwrap();
                                    if let Some(handle) = router.get_mut(&foreground) {
                                        handle.cancel = token.clone();
                                    }
                                }
                                let effective_config = config_with_provider(
                                    &config_template,
                                    state.header.provider.as_deref(),
                                );
                                spawn_tui_agent_run(
                                    handle.clone(),
                                    event_tx.clone(),
                                    router.clone(),
                                    foreground,
                                    task,
                                    state.header.mode,
                                    options,
                                    effective_config,
                                    project_template.clone(),
                                    Some(token),
                                );
                            }
                        },
                        {
                            let event_tx = event_tx.clone();
                            let handle = handle.clone();
                            let router = router.clone();
                            let options_template = base_options.clone();
                            let config_template = run_config.clone();
                            let project_template = run_project_root.clone();
                            move |decision, scope, _tool_name, reason, state| {
                                if decision != ApprovalDecision::Approve {
                                    return;
                                }
                                let Some(task) = state.last_task.clone() else {
                                    state.push_transcript(
                                        "approval: no task is available to resume",
                                    );
                                    return;
                                };
                                let foreground = state.current_session_id.clone();
                                let mut options = options_template.clone();
                                options.permission = state.header.permission;
                                options.model = state.header.model.clone();
                                let mut config = config_with_provider(
                                    &config_template,
                                    state.header.provider.as_deref(),
                                );
                                relax_security_for_approval(&mut config, &reason);
                                if scope != peridot_tui::ApprovalScope::Once {
                                    state.push_transcript(format!(
                                        "approval: scope {scope:?} noted (persistence TBD)"
                                    ));
                                }
                                let token = peridot_core::CancelToken::new();
                                {
                                    let mut router = router.lock().unwrap();
                                    if let Some(handle) = router.get_mut(&foreground) {
                                        handle.cancel = token.clone();
                                    }
                                }
                                spawn_tui_agent_run(
                                    handle.clone(),
                                    event_tx.clone(),
                                    router.clone(),
                                    foreground,
                                    task,
                                    state.header.mode,
                                    options,
                                    config,
                                    project_template.clone(),
                                    Some(token),
                                );
                            }
                        },
                        {
                            let router = router.clone();
                            move |state| {
                                let foreground = state.current_session_id.clone();
                                let cancelled = {
                                    let router = router.lock().unwrap();
                                    router
                                        .get(&foreground)
                                        .map(|handle| {
                                            handle.cancel.cancel();
                                            true
                                        })
                                        .unwrap_or(false)
                                };
                                if cancelled {
                                    state.push_transcript("interrupting current run...");
                                } else {
                                    state.push_transcript("interrupt: no active run");
                                }
                            }
                        },
                        {
                            let router = router.clone();
                            let event_tx = event_tx.clone();
                            let handle = handle.clone();
                            let options_template = base_options.clone();
                            let config_template = run_config.clone();
                            let project_template = run_project_root.clone();
                            move |command, state| {
                                apply_session_command(
                                    command,
                                    state,
                                    &router,
                                    &handle,
                                    &event_tx,
                                    &options_template,
                                    &config_template,
                                    &project_template,
                                );
                            }
                        },
                        {
                            let project_template = run_project_root.clone();
                            let router = router.clone();
                            let mut last_persist_unix: u64 = 0;
                            let mut last_transcript_count: std::collections::HashMap<
                                String,
                                usize,
                            > = std::collections::HashMap::new();
                            move |state: &mut TuiState| {
                                append_new_transcript_entries(
                                    state,
                                    &mut last_transcript_count,
                                    &project_template,
                                );
                                flush_pending_notes(state, &project_template);
                                flush_pending_committee_events(state, &project_template);
                                let now = SystemTime::now()
                                    .duration_since(UNIX_EPOCH)
                                    .map(|d| d.as_secs())
                                    .unwrap_or_default();
                                if now.saturating_sub(last_persist_unix) < 1 {
                                    return;
                                }
                                last_persist_unix = now;
                                persist_session_snapshot(state, &router, &project_template);
                            }
                        },
                    )?;
                    persist_session_snapshot(&exit.state, &router, &run_project_root);
                    run_tui_lifecycle_hooks(&exit.state, &config, &project_root)?;
                }
            }
        }
    }

    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn spawn_tui_agent_run(
    handle: tokio::runtime::Handle,
    event_tx: std::sync::mpsc::Sender<(String, TuiRuntimeEvent)>,
    router: std::sync::Arc<std::sync::Mutex<SessionRouter>>,
    session_id: String,
    task: String,
    mode: ExecutionMode,
    options: run_loop::AgentTaskOptions,
    config: PeridotConfig,
    project_root: PathBuf,
    cancel: Option<peridot_core::CancelToken>,
) {
    let context_snapshot_path = Some(
        project_root
            .join(".peridot")
            .join("sessions")
            .join(&session_id)
            .join("context.bin"),
    );
    handle.spawn(async move {
        let event_sender = event_tx.clone();
        let session = session_id.clone();
        let router_for_events = router.clone();
        let result = run_task_with_events(
            task,
            mode,
            options,
            config,
            project_root,
            cancel,
            context_snapshot_path,
            move |event| {
                let evt = tui_runtime_event_from_agent(event);
                if let TuiRuntimeEvent::ApprovalRequested { reason, .. } = &evt {
                    let foreground = router_for_events
                        .lock()
                        .unwrap()
                        .foreground()
                        .map(|s| s.to_string());
                    if foreground.as_deref() != Some(session.as_str()) {
                        notify_os_attention(&session, reason);
                    }
                }
                let _ = event_sender.send((session.clone(), evt));
            },
        )
        .await;
        if let Err(err) = result {
            let _ = event_tx.send((
                session_id,
                TuiRuntimeEvent::Failed {
                    message: err.to_string(),
                },
            ));
        }
    });
}

#[cfg(feature = "os-notify")]
fn notify_os_attention(session_id: &str, reason: &str) {
    if let Err(err) = notify_rust::Notification::new()
        .summary("Peridot: session needs attention")
        .body(&format!("Session {session_id}: {reason}"))
        .show()
    {
        eprintln!("warning: notify-rust failed for session {session_id}: {err}");
    }
}

#[cfg(not(feature = "os-notify"))]
fn notify_os_attention(_session_id: &str, _reason: &str) {}

#[allow(clippy::too_many_arguments)]
fn apply_session_command(
    command: SessionCommandEvent,
    state: &mut TuiState,
    router: &std::sync::Arc<std::sync::Mutex<SessionRouter>>,
    handle: &tokio::runtime::Handle,
    event_tx: &std::sync::mpsc::Sender<(String, TuiRuntimeEvent)>,
    options_template: &run_loop::AgentTaskOptions,
    config_template: &PeridotConfig,
    project_template: &Path,
) {
    let effective_config = config_with_provider(config_template, state.header.provider.as_deref());
    let config_template = &effective_config;
    match command {
        SessionCommandEvent::SessionNew(task) => {
            let new_id = format!("session-{}-{}", std::process::id(), unix_timestamp());
            let title = task.clone().unwrap_or_else(|| "new session".to_string());
            router.lock().unwrap().register(SessionHandle::new(
                new_id.clone(),
                project_template.to_path_buf(),
                WorkspaceIsolation::Shared,
            ));
            state
                .sessions
                .push(SessionDirectoryItem::new(&new_id, &title));
            state.push_transcript(format!("session: registered {new_id}"));
            if let Some(task) = task {
                spawn_session_task(
                    handle,
                    event_tx,
                    router,
                    new_id,
                    task,
                    state.header.mode,
                    state.header.permission,
                    state.header.model.clone(),
                    options_template,
                    config_template,
                    project_template,
                );
            }
        }
        SessionCommandEvent::SessionSwitch(target) => {
            let resolved = resolve_session_id(state, &target);
            if let Some(id) = resolved {
                let switched = router.lock().unwrap().switch_to(&id);
                if switched {
                    state.current_session_id = id.clone();
                    if let Some(item) = state.sessions.iter_mut().find(|item| item.id == id) {
                        item.pending_attention = false;
                    }
                    state.push_transcript(format!("session: switched to {id}"));
                } else {
                    state.push_error(format!("session: router has no session {id}"));
                }
            } else {
                state.push_error(format!("session: no session matching '{target}'"));
            }
        }
        SessionCommandEvent::SessionClose(target) => {
            let resolved = resolve_session_id(state, &target);
            if let Some(id) = resolved {
                let (removed, worktree_cleanup) = {
                    let mut router = router.lock().unwrap();
                    let cleanup = router.get(&id).and_then(|handle| {
                        if matches!(handle.isolation, WorkspaceIsolation::Worktree { .. }) {
                            Some(handle.workspace_root.clone())
                        } else {
                            None
                        }
                    });
                    if let Some(handle) = router.get(&id) {
                        handle.cancel.cancel();
                    }
                    (router.close(&id), cleanup)
                };
                if let Some(worktree_path) = worktree_cleanup {
                    let git = GitManager::new(project_template);
                    if let Err(err) = git.remove_worktree(&worktree_path) {
                        state.push_error(format!(
                            "worktree cleanup failed for {}: {err}",
                            worktree_path.display()
                        ));
                    } else {
                        state.push_transcript(format!(
                            "worktree: removed {}",
                            worktree_path.display()
                        ));
                    }
                }
                if removed {
                    state.sessions.retain(|item| item.id != id);
                    if state.current_session_id == id {
                        state.current_session_id = router
                            .lock()
                            .unwrap()
                            .foreground()
                            .map(|s| s.to_string())
                            .unwrap_or_default();
                    }
                    state.push_transcript(format!("session: closed {id}"));
                } else {
                    state.push_error(format!("session: nothing to close for '{target}'"));
                }
            } else {
                state.push_error(format!("session: no session matching '{target}'"));
            }
        }
        SessionCommandEvent::Fork(task) => {
            let new_id = format!("fork-{}-{}", std::process::id(), unix_timestamp());
            let title = task.clone();
            let parent_id = state.current_session_id.clone();
            {
                let mut router = router.lock().unwrap();
                let mut new_handle = SessionHandle::new(
                    new_id.clone(),
                    project_template.to_path_buf(),
                    WorkspaceIsolation::Shared,
                );
                new_handle.parent_id = Some(parent_id.clone());
                router.register(new_handle);
            }
            state
                .sessions
                .push(SessionDirectoryItem::new(&new_id, &title).with_parent(&parent_id, "fork"));
            inherit_parent_context(&parent_id, &new_id, project_template);
            spawn_session_task(
                handle,
                event_tx,
                router,
                new_id.clone(),
                task,
                state.header.mode,
                state.header.permission,
                state.header.model.clone(),
                options_template,
                config_template,
                project_template,
            );
            state.push_transcript(format!("fork: registered {new_id}"));
        }
        SessionCommandEvent::Teammate(task) => {
            let new_id = format!("teammate-{}-{}", std::process::id(), unix_timestamp());
            let branch = format!("peridot/teammate-{new_id}");
            spawn_worktree_session(
                &new_id,
                &branch,
                "teammate",
                task,
                state,
                router,
                handle,
                event_tx,
                options_template,
                config_template,
                project_template,
            );
        }
        SessionCommandEvent::Worktree { branch, task } => {
            let new_id = format!("worktree-{}-{}", std::process::id(), unix_timestamp());
            spawn_worktree_session(
                &new_id,
                &branch,
                "worktree",
                task,
                state,
                router,
                handle,
                event_tx,
                options_template,
                config_template,
                project_template,
            );
        }
    }
    warn_on_shared_workspace_collisions(state, router, project_template);
}

#[allow(clippy::too_many_arguments)]
fn spawn_worktree_session(
    new_id: &str,
    branch: &str,
    kind: &str,
    task: String,
    state: &mut TuiState,
    router: &std::sync::Arc<std::sync::Mutex<SessionRouter>>,
    handle: &tokio::runtime::Handle,
    event_tx: &std::sync::mpsc::Sender<(String, TuiRuntimeEvent)>,
    options_template: &run_loop::AgentTaskOptions,
    config_template: &PeridotConfig,
    project_template: &Path,
) {
    let worktree_path = project_template
        .join(".peridot")
        .join("worktrees")
        .join(new_id);
    if let Some(parent) = worktree_path.parent()
        && let Err(err) = std::fs::create_dir_all(parent)
    {
        state.push_error(format!(
            "worktree: failed to create parent directory {}: {err}",
            parent.display()
        ));
        return;
    }
    let git = GitManager::new(project_template);
    match git.add_worktree(&worktree_path, branch) {
        Ok(_) => {}
        Err(err) => {
            state.push_error(format!(
                "worktree: failed to create branch {branch} at {}: {err}",
                worktree_path.display()
            ));
            return;
        }
    }
    let title = task.clone();
    let parent_id = state.current_session_id.clone();
    {
        let mut router = router.lock().unwrap();
        let mut new_handle = SessionHandle::new(
            new_id.to_string(),
            worktree_path.clone(),
            WorkspaceIsolation::Worktree {
                branch: branch.to_string(),
            },
        );
        new_handle.parent_id = Some(parent_id.clone());
        router.register(new_handle);
    }
    state
        .sessions
        .push(SessionDirectoryItem::new(new_id, &title).with_parent(&parent_id, kind));
    state.push_transcript(format!(
        "worktree: registered {new_id} on branch {branch} at {}",
        worktree_path.display()
    ));
    inherit_parent_context(&parent_id, new_id, project_template);
    spawn_session_task(
        handle,
        event_tx,
        router,
        new_id.to_string(),
        task,
        state.header.mode,
        state.header.permission,
        state.header.model.clone(),
        options_template,
        config_template,
        &worktree_path,
    );
}

fn warn_on_shared_workspace_collisions(
    state: &mut TuiState,
    router: &std::sync::Arc<std::sync::Mutex<SessionRouter>>,
    project_template: &Path,
) {
    let active_shared = router
        .lock()
        .unwrap()
        .iter()
        .filter(|handle| {
            matches!(handle.isolation, WorkspaceIsolation::Shared)
                && handle.workspace_root == project_template
        })
        .count();
    if active_shared > 1 {
        state.push_error(format!(
            "warning: {active_shared} sessions share {} — concurrent file writes may collide. \
             Use /teammate or /worktree for isolated runs.",
            project_template.display()
        ));
    }
}

fn persist_session_snapshot(
    state: &TuiState,
    router: &std::sync::Arc<std::sync::Mutex<SessionRouter>>,
    project_root: &Path,
) {
    if state.current_session_id.is_empty() {
        return;
    }
    let sessions_root = project_root.join(".peridot").join("sessions");
    let id = state.current_session_id.as_str();
    if let Ok(bytes) = serde_json::to_vec(state) {
        let _ = save_session_blob(&sessions_root, id, "tui_state.json", &bytes);
    }
    let lifecycle = lifecycle_from_status(&state.agent_run_status);
    let (workspace_root, worktree_branch, started_at_unix) = {
        let mut router = router.lock().unwrap();
        let Some(handle) = router.get_mut(id) else {
            return;
        };
        handle.lifecycle = lifecycle;
        (
            handle.workspace_root.clone(),
            handle.worktree_branch.clone(),
            handle.started_at_unix,
        )
    };
    let record = SessionRecord {
        id: id.to_string(),
        summary: state.last_task.clone().unwrap_or_default(),
        status: lifecycle,
        created_at_unix: started_at_unix,
        updated_at_unix: unix_timestamp(),
        workspace_root,
        worktree_branch,
        last_task: state.last_task.clone(),
        total_tokens: state.header.total_tokens,
        total_cost_usd: state.header.cost_usd,
        turns_used: state.current_turn,
    };
    let memory = MemoryStore::new(project_root.join(".peridot/memory.db"));
    let _ = memory.save_session_record(&record);
}

/// Copies the parent session's `context.bin` to the child session's directory
/// so the spawned agent loop starts with the same conversation history. The
/// agent loop's restore_entries on the first turn picks the file up. Silently
/// returns when the parent has no snapshot yet (a freshly opened parent with
/// zero completed turns).
fn inherit_parent_context(parent_id: &str, child_id: &str, project_root: &Path) {
    let sessions = project_root.join(".peridot").join("sessions");
    let parent = sessions.join(parent_id).join("context.bin");
    if !parent.exists() {
        return;
    }
    let child_dir = sessions.join(child_id);
    if std::fs::create_dir_all(&child_dir).is_err() {
        return;
    }
    let _ = std::fs::copy(&parent, child_dir.join("context.bin"));
}

/// Returns a clone of `template` with `auth.primary` replaced by `provider`
/// when one is set. Used to thread per-session `/provider` selections through
/// to `live_provider` without mutating the project-wide config.
/// Drains the foreground session's queued committee events and appends one
/// JSON line per event to `<sessions>/<id>/committee.ndjson`. Mirrors
/// `flush_pending_notes`. Errors are silent so it can never block the UI.
fn flush_pending_committee_events(state: &mut TuiState, project_root: &Path) {
    if state.current_session_id.is_empty() {
        return;
    }
    let pending = state.drain_pending_committee_events();
    if pending.is_empty() {
        return;
    }
    let session_dir = project_root
        .join(".peridot")
        .join("sessions")
        .join(&state.current_session_id);
    if std::fs::create_dir_all(&session_dir).is_err() {
        return;
    }
    let path = session_dir.join("committee.ndjson");
    let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
    else {
        return;
    };
    use std::io::Write;
    for event in pending {
        let Ok(line) = serde_json::to_string(&event) else {
            continue;
        };
        if writeln!(file, "{line}").is_err() {
            break;
        }
    }
}

/// Drains the foreground session's queued `/note` slash commands and appends
/// one `{ "ts", "text" }` line per note to `<sessions>/<id>/notes.ndjson`.
/// Errors are silent: this runs from the UI thread and must never block.
fn flush_pending_notes(state: &mut TuiState, project_root: &Path) {
    if state.current_session_id.is_empty() {
        return;
    }
    let pending = state.drain_pending_notes();
    if pending.is_empty() {
        return;
    }
    let session_dir = project_root
        .join(".peridot")
        .join("sessions")
        .join(&state.current_session_id);
    if std::fs::create_dir_all(&session_dir).is_err() {
        return;
    }
    let path = session_dir.join("notes.ndjson");
    let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
    else {
        return;
    };
    use std::io::Write;
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or_default();
    for body in pending {
        let line = serde_json::json!({"ts": ts, "text": body});
        let Ok(serialised) = serde_json::to_string(&line) else {
            continue;
        };
        if writeln!(file, "{serialised}").is_err() {
            break;
        }
    }
}

/// Appends any transcript entries past `last_count` for the foreground
/// session to `<sessions>/<id>/transcript.ndjson`. Each entry is one JSON line
/// (newline-delimited). The append is best-effort: if the directory or file
/// is unavailable, the call is a no-op so it can never block the UI thread.
fn append_new_transcript_entries(
    state: &TuiState,
    last_counts: &mut std::collections::HashMap<String, usize>,
    project_root: &Path,
) {
    if state.current_session_id.is_empty() {
        return;
    }
    let id = state.current_session_id.clone();
    let last = *last_counts.get(&id).unwrap_or(&0);
    if state.transcript.len() <= last {
        return;
    }
    let session_dir = project_root.join(".peridot").join("sessions").join(&id);
    if std::fs::create_dir_all(&session_dir).is_err() {
        return;
    }
    let path = session_dir.join("transcript.ndjson");
    let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
    else {
        return;
    };
    use std::io::Write;
    let mut written = last;
    for entry in &state.transcript[last..] {
        let Ok(line) = serde_json::to_string(entry) else {
            continue;
        };
        if writeln!(file, "{line}").is_err() {
            break;
        }
        written += 1;
    }
    last_counts.insert(id, written);
}

fn config_with_provider(template: &PeridotConfig, provider: Option<&str>) -> PeridotConfig {
    let mut cfg = template.clone();
    if let Some(value) = provider
        && !value.is_empty()
    {
        cfg.auth.primary = value.to_string();
    }
    cfg
}

fn lifecycle_from_status(status: &peridot_tui::AgentRunStatus) -> SessionLifecycle {
    use peridot_tui::AgentRunStatus;
    match status {
        AgentRunStatus::Running | AgentRunStatus::WaitingApproval => SessionLifecycle::Running,
        AgentRunStatus::Succeeded => SessionLifecycle::Done,
        AgentRunStatus::Failed => SessionLifecycle::Failed,
        AgentRunStatus::Interrupted => SessionLifecycle::Suspended,
        AgentRunStatus::Idle => SessionLifecycle::Idle,
    }
}

/// Restores a previously persisted `TuiState` from
/// `<project_root>/.peridot/sessions/<id>/tui_state.json`. Returns the
/// session id alongside the deserialized state so the caller can prime its
/// `current_session_id`.
fn restore_tui_state_from_disk(
    id: &str,
    project_root: &Path,
) -> anyhow::Result<(String, TuiState)> {
    let sessions_root = project_root.join(".peridot").join("sessions");
    let bytes = peridot_memory::load_session_blob(&sessions_root, id, "tui_state.json")?
        .ok_or_else(|| anyhow::anyhow!("no persisted tui_state.json for session {id}"))?;
    let state: TuiState = serde_json::from_slice(&bytes)
        .with_context(|| format!("failed to parse persisted tui_state.json for session {id}"))?;
    Ok((id.to_string(), state))
}

/// Downgrades any session record still marked `Running` to `Suspended` on
/// startup. Returns the ids that were transitioned.
fn scan_and_suspend_running_sessions(project_root: &Path) -> Vec<String> {
    let memory = MemoryStore::new(project_root.join(".peridot/memory.db"));
    let Ok(records) = memory.list_session_records() else {
        return Vec::new();
    };
    let mut suspended = Vec::new();
    for record in records {
        if record.status != SessionLifecycle::Running {
            continue;
        }
        let mut updated = record;
        updated.status = SessionLifecycle::Suspended;
        if memory.save_session_record(&updated).is_ok() {
            suspended.push(updated.id);
        }
    }
    suspended
}

#[allow(clippy::too_many_arguments)]
fn spawn_session_task(
    handle: &tokio::runtime::Handle,
    event_tx: &std::sync::mpsc::Sender<(String, TuiRuntimeEvent)>,
    router: &std::sync::Arc<std::sync::Mutex<SessionRouter>>,
    session_id: String,
    task: String,
    mode: ExecutionMode,
    permission: PermissionMode,
    model: String,
    options_template: &run_loop::AgentTaskOptions,
    config_template: &PeridotConfig,
    project_template: &Path,
) {
    let mut options = options_template.clone();
    options.permission = permission;
    options.model = model;
    let token = peridot_core::CancelToken::new();
    if let Some(session_handle) = router.lock().unwrap().get_mut(&session_id) {
        session_handle.cancel = token.clone();
    }
    spawn_tui_agent_run(
        handle.clone(),
        event_tx.clone(),
        router.clone(),
        session_id,
        task,
        mode,
        options,
        config_template.clone(),
        project_template.to_path_buf(),
        Some(token),
    );
}

fn resolve_session_id(state: &TuiState, target: &str) -> Option<String> {
    state
        .sessions
        .iter()
        .find(|item| item.id == target || item.title == target)
        .map(|item| item.id.clone())
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
        AgentRunEvent::ApprovalRequested {
            tool_name,
            reason,
            parameters,
        } => TuiRuntimeEvent::ApprovalRequested {
            tool_name,
            reason,
            parameters,
        },
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
            duration_ms: summary.duration_ms,
        },
        AgentRunEvent::SessionSaved { session_id } => TuiRuntimeEvent::SessionSaved { session_id },
        AgentRunEvent::SessionSaveFailed {
            session_id,
            message,
        } => TuiRuntimeEvent::SessionSaveFailed {
            session_id,
            message,
        },
        AgentRunEvent::TurnEnded {
            turn_index,
            success,
        } => TuiRuntimeEvent::TurnEnded {
            turn_index,
            success,
        },
        AgentRunEvent::PlanUpdated { steps, current } => TuiRuntimeEvent::PlanUpdated {
            steps: steps
                .into_iter()
                .map(|step| peridot_tui::PlanStepUpdate {
                    label: step.label,
                    done: step.done,
                })
                .collect(),
            current,
        },
        AgentRunEvent::BudgetUpdated {
            cost_used,
            cost_limit,
            turns_used,
            turns_limit,
        } => TuiRuntimeEvent::BudgetUpdated {
            cost_used,
            cost_limit,
            turns_used,
            turns_limit,
        },
        AgentRunEvent::ContextUtilizationChanged {
            tokens_used,
            threshold,
        } => TuiRuntimeEvent::ContextUtilizationChanged {
            tokens_used,
            threshold,
        },
        AgentRunEvent::McpStatusChanged { servers } => TuiRuntimeEvent::McpStatusChanged {
            servers: servers
                .into_iter()
                .map(|server| peridot_tui::McpServerSummary {
                    name: server.name,
                    tool_count: server.tool_count,
                    connected: server.connected,
                })
                .collect(),
        },
        AgentRunEvent::AgentsMdLoaded { rule_count, paths } => {
            TuiRuntimeEvent::AgentsMdLoaded { rule_count, paths }
        }
        AgentRunEvent::HookFired {
            name,
            category,
            outcome,
        } => TuiRuntimeEvent::HookFired {
            name,
            category,
            outcome,
        },
        AgentRunEvent::Interrupted { stage } => TuiRuntimeEvent::Interrupted { stage },
        AgentRunEvent::PlannerPlanReady { plan_text } => {
            TuiRuntimeEvent::PlannerPlanReady { plan_text }
        }
        AgentRunEvent::ReviewerVerdict {
            turn_index,
            verdict,
        } => {
            let (label, comments) = match verdict {
                peridot_core::ReviewerVerdict::Approve => ("approve".to_string(), String::new()),
                peridot_core::ReviewerVerdict::RequestChanges { comments } => {
                    ("request_changes".to_string(), comments)
                }
                peridot_core::ReviewerVerdict::Block { reason } => ("block".to_string(), reason),
            };
            TuiRuntimeEvent::ReviewerVerdict {
                turn_index,
                verdict: label,
                comments,
            }
        }
        AgentRunEvent::CommitteeRoleUsage {
            role,
            cost_usd,
            tokens,
        } => TuiRuntimeEvent::CommitteeRoleUsage {
            role,
            cost_usd,
            tokens,
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
