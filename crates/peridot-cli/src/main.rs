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
    maybe_run_first_launch_wizard, move_auto_skill_to_archive, print_scan, read_stored_api_key,
    read_stored_openai_oauth_credentials, run_agents_command, run_config_command,
    run_doctor_command, run_env_command, run_login_command, run_logout_command, run_mcp_command,
    run_session_command, run_setting_command, run_setup_command, run_ship_command,
    run_skill_command, run_update_command, run_verify_command,
};
use peridot_common::{
    AskUserAnswer, AskUserRequest, ContextConfig, ExecutionMode, MemoryConfig, PeriError,
    PeriResult, PeridotConfig, PermissionMode,
};
use peridot_context::{
    ContextEntry, ContextLimits, ContextManager, ContextSource, project_context_limits,
};
use peridot_core::{
    AgentRunEvent, AgentRunRequest, AgentRunSummary, AgentState, HarnessAgent, StopReason,
};
use peridot_git::GitManager;
use peridot_llm::{
    AuthMethod, ClaudeProvider, CompletionRequest, CompletionResponse, LlmMessage, LlmProvider,
    MessageRole, OpenAiCodexProvider, OpenAiProvider, PricingTable, Usage,
};
use peridot_mcp::McpClient;
use peridot_memory::{
    MemoryStore, SessionLifecycle, SessionRecord, SessionSummary, StoredSkill, save_session_blob,
};
use peridot_project::ProjectScanner;
use peridot_tools::hooks::{HookRunner, HookVariables, lifecycle_hook_variables};
use peridot_tools::{AskUserPort, ToolRegistry, register_builtin_tools, register_mcp_tools};
use peridot_tui::{
    ApprovalDecision, ApprovalGrant, ApprovalScope, HeaderState, SessionCommandEvent,
    SessionDirectoryItem, SkillSlashSuggestion, TuiRuntimeEvent, TuiState,
    run_interactive_with_events,
};

mod checkpoints;
mod commands;
mod context_limits;
mod curator;
mod harness_learn;
mod interactive_io;
mod providers;
mod run_loop;
mod run_output;
mod run_state;
mod session_router;
#[cfg(test)]
mod tests;

use checkpoints::restore_latest_checkpoint;
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

    /// Emit one JSON line per `AgentRunEvent` on stderr (newline-
    /// delimited JSON). Useful for CI pipelines and automated QA — see
    /// `--headless`, which implicitly enables this so the script user
    /// gets *some* observability instead of a silent run. Stdout
    /// remains reserved for the final summary so existing pipes keep
    /// working.
    #[arg(long, global = true)]
    ndjson_events: bool,

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

/// Returns a suggested real subcommand when the operator typed a
/// well-known typo as the freeform task argument. The map is
/// intentionally small — only commands that are *commonly confused*
/// with what people type out of habit from other CLIs. False positives
/// here mean blocking a legit task, so we err toward "only catch the
/// obvious ones."
///
/// Caller passes `(has_subcommand, task_text)` so this stays a pure
/// function and unit tests don't need to fabricate a `Cli` (which
/// would require clap parsing). Returns `None` whenever the hint
/// should NOT fire — caller treats `None` as "carry on, valid input."
fn unknown_subcommand_hint(cli: &Cli) -> Option<&'static str> {
    suggest_subcommand_for_typo(cli.command.is_some(), cli.task.as_deref())
}

fn suggest_subcommand_for_typo(has_subcommand: bool, task: Option<&str>) -> Option<&'static str> {
    if has_subcommand {
        return None;
    }
    let raw = task?;
    let trimmed = raw.trim();
    if trimmed.is_empty() || trimmed.contains(char::is_whitespace) {
        return None;
    }
    match trimmed.to_ascii_lowercase().as_str() {
        "init" | "initialize" | "initialise" => Some("setup"),
        "status" => Some("doctor"),
        "start" => Some("run"),
        "config" | "configure" | "settings" => Some("setting"),
        _ => None,
    }
}

#[cfg(test)]
mod subcommand_hint_tests {
    use super::suggest_subcommand_for_typo as suggest;

    #[test]
    fn maps_common_typos_to_real_subcommands() {
        assert_eq!(suggest(false, Some("init")), Some("setup"));
        assert_eq!(suggest(false, Some("INIT")), Some("setup"));
        assert_eq!(suggest(false, Some("initialize")), Some("setup"));
        assert_eq!(suggest(false, Some("status")), Some("doctor"));
        assert_eq!(suggest(false, Some("start")), Some("run"));
        assert_eq!(suggest(false, Some("config")), Some("setting"));
        assert_eq!(suggest(false, Some("settings")), Some("setting"));
    }

    #[test]
    fn no_suggestion_when_subcommand_present() {
        // `peridot setup` shouldn't trigger the hint just because the
        // task arg also happened to be empty.
        assert_eq!(suggest(true, None), None);
        assert_eq!(suggest(true, Some("init")), None);
    }

    #[test]
    fn no_suggestion_for_multiword_task() {
        // `peridot "init the database for users"` is a legitimate task
        // — don't hijack it.
        assert_eq!(suggest(false, Some("init the database")), None);
        assert_eq!(suggest(false, Some("status of the system")), None);
    }

    #[test]
    fn no_suggestion_for_unknown_tokens() {
        // Random user task — must run as a task, no hint.
        assert_eq!(suggest(false, Some("refactor")), None);
        assert_eq!(suggest(false, Some("fix")), None);
        assert_eq!(suggest(false, None), None);
    }

    #[test]
    fn ignores_whitespace_padding() {
        // Shell quoting may leave trailing space — still match.
        assert_eq!(suggest(false, Some("  init  ")), Some("setup"));
    }
}

impl Cli {
    fn effective_headless(&self) -> bool {
        self.headless || env_truthy("PERIDOT_HEADLESS")
    }

    /// Decide whether to stream JSONL events to stderr. Explicit
    /// `--ndjson-events`, the `PERIDOT_NDJSON_EVENTS=1` env var, or
    /// `--headless` (which would otherwise be silent) all turn it on.
    /// `--headless` defaulting to ndjson means existing automation
    /// scripts that already pass `--headless` get observability for
    /// free — no flag-day breakage.
    fn effective_ndjson_events(&self) -> bool {
        self.ndjson_events || env_truthy("PERIDOT_NDJSON_EVENTS") || self.effective_headless()
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

/// Args struct for `peridot verify`. Extracted from the inline enum
/// variant because rustc 1.95 hit an ICE in `report_arg_errors` when an
/// inline struct variant with optional boolean flags was used inside
/// `main`'s match.
#[derive(Debug, clap::Args)]
struct VerifyArgs {
    /// Also invoke the LLM grader after deterministic stages pass.
    /// Uses the same primary provider as the agent loop.
    #[arg(long)]
    with_grader: bool,
    /// Task description handed to the grader. Required when
    /// `--with-grader` is set; ignored otherwise.
    #[arg(long, value_name = "TEXT")]
    grader_task: Option<String>,
}

/// Args struct for `peridot ship`. Bundles branch / commit / push / PR
/// settings into a single high-level invocation.
#[derive(Debug, clap::Args)]
struct ShipArgs {
    /// Target branch (created if missing). Defaults to
    /// `peridot/ship-<unix-seconds>` so two consecutive ships never
    /// collide silently.
    #[arg(long)]
    branch: Option<String>,
    /// Commit message. Defaults to "ship: N file(s) via peridot".
    #[arg(long, short = 'm')]
    message: Option<String>,
    /// Pull request title. Defaults to the first line of the commit
    /// message.
    #[arg(long)]
    pr_title: Option<String>,
    /// Pull request body. Defaults to a short notice mentioning peridot.
    #[arg(long)]
    pr_body: Option<String>,
    /// Base branch for the PR. Defaults to whatever the remote considers
    /// the default branch.
    #[arg(long)]
    base: Option<String>,
    /// Open the PR as a draft.
    #[arg(long)]
    draft: bool,
    /// Skip the `gh pr create` step entirely. Useful when the PR is
    /// raised manually after `ship` lands the commit on the remote.
    #[arg(long)]
    no_pr: bool,
    /// Allow shipping directly onto main / master / trunk. Off by
    /// default — `ship` refuses to push to a protected branch unless
    /// the operator opts in explicitly.
    #[arg(long)]
    allow_protected_branch: bool,
    /// Preview the branch / commit / push / PR steps without mutating
    /// the worktree or contacting GitHub.
    #[arg(long)]
    dry_run: bool,
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
    Verify(VerifyArgs),
    /// Initialize project-local Peridot files.
    Setup,
    /// JSON-RPC over stdio server. Editor extensions (VS Code, desktop
    /// app) spawn `peridot daemon` and speak line-delimited JSON-RPC
    /// 2.0 to drive sessions bidirectionally. v0.0.1 surface is
    /// `peridot.version` / `peridot.echo` / `shutdown` — real
    /// `session.start` arrives once the extension WebView is ready.
    Daemon,
    /// End-to-end health check: validates config, provider auth, MCP
    /// servers, AGENTS metadata, and permissions. Exit code 0 when
    /// everything passes, non-zero when any check fails.
    Doctor,
    /// One-shot publish flow: branch → commit → push → PR. Wraps the
    /// per-step `git_*` and `gh_pr_*` tools so an operator can move
    /// from "I have local changes" to "PR is open" in a single call.
    Ship(ShipArgs),
    /// Configuration commands.
    Config {
        /// Config subcommand.
        #[command(subcommand)]
        command: ConfigCommand,
    },
    /// Open the interactive settings screen.
    ///
    /// Lists every toggleable / cycleable option in a single TUI
    /// screen. Saves to `.peridot/config.toml` on `s`, discards on
    /// `q` / `Esc`. Use this instead of editing the config file by
    /// hand.
    Setting,
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

    // Guard against a common onboarding paper cut: typing `peridot init`
    // (or `status`, `start`) when the actual subcommand is `setup` /
    // `doctor` / `run`. Without this check, clap accepts the typo as
    // the freeform task argument, peridot fires up the LLM to "do" a
    // task literally named "init", and burns turns until the model
    // gives up. Catching it here means a near-zero-cost error instead.
    if let Some(suggestion) = unknown_subcommand_hint(&cli) {
        eprintln!(
            "error: `{task}` looks like a misspelled subcommand. \
             Did you mean `peridot {suggestion}`?\n\
             \n\
             If you really wanted to run that text as a task, use:\n  \
                 peridot run \"{task}\"",
            task = cli.task.as_deref().unwrap_or(""),
            suggestion = suggestion,
        );
        std::process::exit(2);
    }

    let project_root = cli.project.clone().unwrap_or(std::env::current_dir()?);
    if cli.starts_agent_session() {
        maybe_run_first_launch_wizard(
            &project_root,
            cli.config.as_deref(),
            cli.effective_headless(),
            cli.output,
        )
        .await?;
    }
    let config = load_effective_config(&project_root, cli.config.as_deref())?;

    // Hermes-style 7-day idle Curator. Cheap when not due (one SQLite
    // SELECT), otherwise refines `scope='auto'` skills before the user's
    // command continues. We run inline rather than spawning so the rare
    // 7-day fire isn't lost to a fast-exit command like `peridot version`.
    maybe_run_idle_curator(&config, &project_root).await;

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
        Some(Command::Verify(args)) => {
            run_verify_command(
                &project_root,
                &config,
                cli.output,
                args.with_grader,
                args.grader_task.clone(),
            )
            .await?;
            return Ok(());
        }
        Some(Command::Setup) => {
            run_setup_command(&project_root, cli.output)?;
            return Ok(());
        }
        Some(Command::Daemon) => {
            let template = agent_task_options(&cli, &config);
            commands::run_daemon_command(&project_root, &config, template).await?;
            return Ok(());
        }
        Some(Command::Doctor) => {
            run_doctor_command(&project_root, &config, cli.output).await?;
            return Ok(());
        }
        Some(Command::Ship(args)) => {
            let options = commands::ShipOptions {
                branch: args.branch.clone(),
                commit_message: args.message.clone(),
                pr_title: args.pr_title.clone(),
                pr_body: args.pr_body.clone(),
                base: args.base.clone(),
                draft: args.draft,
                no_pr: args.no_pr,
                allow_protected_branch: args.allow_protected_branch,
                dry_run: args.dry_run,
            };
            run_ship_command(&project_root, &config, options, cli.output).await?;
            return Ok(());
        }
        Some(Command::Config { command }) => {
            run_config_command(command, &config, &project_root, cli.output).await?;
            return Ok(());
        }
        Some(Command::Setting) => {
            run_setting_command(&project_root, cli.output)?;
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
            run_skill_command(command, &project_root, cli.output, Some(&config)).await?;
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
                    let suspended = scan_and_suspend_running_sessions(&project_root);
                    let restored_state = cli
                        .resume
                        .as_deref()
                        .and_then(|id| restore_tui_state_from_disk(id, &project_root).ok())
                        .or_else(|| restore_latest_tui_state_from_disk(&project_root).ok());
                    let workspace_label = project_root
                        .file_name()
                        .and_then(|name| name.to_str())
                        .map(|name| name.to_string());
                    let mut state = match restored_state {
                        Some((id, restored)) => {
                            let mut state = restored.with_config(config.tui.clone());
                            state.header.workspace_label = workspace_label.clone();
                            state.committee_mode = config.committee.mode;
                            if state.service_tier.is_none() {
                                state.service_tier = config.models.service_tier.clone();
                            }
                            state.set_skill_suggestions(load_auto_skill_suggestions(&project_root));
                            state.push_notice(format!("session: restored {id} from disk"));
                            state
                        }
                        None => {
                            let mut header = HeaderState::new(mode, permission, model.clone());
                            header.workspace_label = workspace_label.clone();
                            let mut state = TuiState::new(header).with_config(config.tui.clone());
                            state.committee_mode = config.committee.mode;
                            state.service_tier = config.models.service_tier.clone();
                            // Warm the `@file` picker index up-front so the
                            // first `@` keystroke gets an instant suggestion
                            // list instead of having to walk the project
                            // tree under the keystroke event.
                            state.ensure_at_picker_index(&project_root);
                            state.set_skill_suggestions(load_auto_skill_suggestions(&project_root));
                            state.push_transcript("Peridot ready. Type a task, /plan, /execute, /goal <objective>, /safe, /auto, /yolo, or Esc.");
                            state.push_transcript(
                                "Submitted tasks continue inside this TUI; tool activity and run status stream here.",
                            );
                            state
                        }
                    };
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
                    hydrate_persisted_sessions(&mut state, &router, &project_root);
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
                    if router.lock().unwrap().get(&initial_session_id).is_none() {
                        router.lock().unwrap().register(SessionHandle::new(
                            initial_session_id.clone(),
                            project_root.clone(),
                            WorkspaceIsolation::Shared,
                        ));
                    }
                    if state.sessions.len() > 1 {
                        let mut lines = String::from("sessions:");
                        for item in &state.sessions {
                            let marker = if item.id == state.current_session_id {
                                ">"
                            } else {
                                " "
                            };
                            let status = format!("{:?}", item.status).to_ascii_lowercase();
                            lines.push_str(&format!("\n {marker} {} ({status})", item.title,));
                        }
                        state.push_notice(lines);
                    }
                    let (event_tx, event_rx) =
                        std::sync::mpsc::channel::<(String, TuiRuntimeEvent)>();
                    let handle = tokio::runtime::Handle::current();
                    let base_options = agent_task_options(&cli, &config);
                    let run_config = config.clone();
                    let run_project_root = project_root.clone();
                    let ask_user_pending: AskUserPending = std::sync::Arc::new(
                        std::sync::Mutex::new(std::collections::HashMap::new()),
                    );
                    let ask_user_next_id: std::sync::Arc<std::sync::atomic::AtomicU64> =
                        std::sync::Arc::new(std::sync::atomic::AtomicU64::new(1));
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
                            let ask_user_pending = ask_user_pending.clone();
                            let ask_user_next_id = ask_user_next_id.clone();
                            move |task, state| {
                                let foreground = state.current_session_id.clone();
                                let mut options = options_template.clone();
                                options.permission = state.header.permission;
                                options.model = state.header.model.clone();
                                options.reasoning_effort = state.reasoning_effort;
                                options.service_tier = state.service_tier.clone();
                                let token = peridot_core::CancelToken::new();
                                let compact_flag = {
                                    let mut router = router.lock().unwrap();
                                    if let Some(handle) = router.get_mut(&foreground) {
                                        handle.cancel = token.clone();
                                        Some(handle.compact_request.clone())
                                    } else {
                                        None
                                    }
                                };
                                let effective_config = config_for_state(&config_template, state);
                                let needs_title = state
                                    .sessions
                                    .iter()
                                    .find(|s| s.id == foreground)
                                    .is_some_and(|s| !s.title_generated);
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
                                    compact_flag,
                                    ask_user_pending.clone(),
                                    ask_user_next_id.clone(),
                                    needs_title,
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
                            let ask_user_pending = ask_user_pending.clone();
                            let ask_user_next_id = ask_user_next_id.clone();
                            move |decision,
                                  scope,
                                  tool_name,
                                  reason,
                                  parameters,
                                  synthesised_parameters,
                                  state| {
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
                                options.reasoning_effort = state.reasoning_effort;
                                options.service_tier = state.service_tier.clone();
                                let grant = approval_grant_from_event(
                                    tool_name.clone(),
                                    reason.clone(),
                                    scope,
                                    &parameters,
                                );
                                let mut config = config_for_state(&config_template, state);
                                apply_approval_grant_to_config(&mut config, &grant);
                                if scope != ApprovalScope::Once
                                    && !state.approval_grants.contains(&grant)
                                {
                                    state.approval_grants.push(grant.clone());
                                }
                                if let Some(ref synth) = synthesised_parameters {
                                    state.push_transcript(
                                        "approval: partial-hunk patch staged \u{2014} re-running with selected hunks only",
                                    );
                                    let resume_path = project_template
                                        .join(".peridot/sessions")
                                        .join(&foreground)
                                        .join("pending_resume.bin");
                                    if resume_path.exists()
                                        && let Ok(bytes) = std::fs::read(&resume_path)
                                        && let Ok(mut call) =
                                            serde_json::from_slice::<peridot_common::ToolCall>(
                                                &bytes,
                                            )
                                    {
                                        call.parameters = synth.clone();
                                        let _ = serde_json::to_vec(&call)
                                            .map(|b| std::fs::write(&resume_path, b));
                                    }
                                }
                                if scope != ApprovalScope::Once {
                                    state.push_transcript(format!(
                                        "approval: scope {scope:?} remembered for this session"
                                    ));
                                }
                                let token = peridot_core::CancelToken::new();
                                let compact_flag = {
                                    let mut router = router.lock().unwrap();
                                    if let Some(handle) = router.get_mut(&foreground) {
                                        handle.cancel = token.clone();
                                        Some(handle.compact_request.clone())
                                    } else {
                                        None
                                    }
                                };
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
                                    compact_flag,
                                    ask_user_pending.clone(),
                                    ask_user_next_id.clone(),
                                    false,
                                );
                            }
                        },
                        {
                            let ask_user_pending = ask_user_pending.clone();
                            move |request_id, answer, _state| {
                                resolve_ask_user(&ask_user_pending, request_id, answer);
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
                                    let queued = state.input_queue.len();
                                    if queued > 0 {
                                        state.input_queue.clear();
                                        state.push_transcript(format!(
                                            "interrupt: cleared {queued} queued input(s); re-submit manually when ready"
                                        ));
                                    }
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
                            let ask_user_pending = ask_user_pending.clone();
                            let ask_user_next_id = ask_user_next_id.clone();
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
                                    &ask_user_pending,
                                    &ask_user_next_id,
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

/// Shared registry of in-flight `agent_ask_user` requests. The
/// `TuiAskUserPort` inserts a oneshot sender keyed by request id when
/// it dispatches a question; the TUI resolution callback removes the
/// matching entry and fulfils the channel when the operator confirms or
/// cancels the panel. Wrapped in a plain `std::sync::Mutex` because the
/// critical sections are O(1) HashMap ops with no `.await` inside.
type AskUserPending = std::sync::Arc<
    std::sync::Mutex<
        std::collections::HashMap<String, tokio::sync::oneshot::Sender<AskUserAnswer>>,
    >,
>;

/// `AskUserPort` implementation that ferries questions through the TUI
/// event channel and awaits a structured answer from the panel.
struct TuiAskUserPort {
    session_id: String,
    event_tx: std::sync::mpsc::Sender<(String, TuiRuntimeEvent)>,
    next_id: std::sync::Arc<std::sync::atomic::AtomicU64>,
    pending: AskUserPending,
}

#[async_trait]
impl AskUserPort for TuiAskUserPort {
    async fn ask(&self, request: AskUserRequest) -> AskUserAnswer {
        let request_id = self
            .next_id
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let request_id = format!("{}:ask-user:{request_id}", self.session_id);
        let (tx, rx) = tokio::sync::oneshot::channel();
        {
            let mut pending = self.pending.lock().unwrap();
            pending.insert(request_id.clone(), tx);
        }
        if self
            .event_tx
            .send((
                self.session_id.clone(),
                TuiRuntimeEvent::AskUserRequested {
                    request_id: request_id.clone(),
                    request,
                },
            ))
            .is_err()
        {
            // TUI channel closed before the question reached the panel:
            // drop the pending sender and fall back to the synthesised
            // default so the agent loop does not deadlock.
            self.pending.lock().unwrap().remove(&request_id);
            return AskUserAnswer::Cancelled;
        }
        rx.await.unwrap_or(AskUserAnswer::Cancelled)
    }
}

/// Resolves the pending `agent_ask_user` request matching `request_id`
/// by sending `answer` through its registered oneshot. No-ops when the
/// id is unknown (e.g., the agent already cancelled the run).
fn resolve_ask_user(pending: &AskUserPending, request_id: String, answer: AskUserAnswer) {
    let sender = pending.lock().unwrap().remove(&request_id);
    if let Some(sender) = sender {
        let _ = sender.send(answer);
    }
}

/// Generate a short LLM-authored title for a coding session.
///
/// Uses the configured main model with reasoning disabled, so this is a single
/// cheap completion call — no thinking, no tools. Returns `None` if the
/// provider fails or returns an empty string; callers (TUI, daemon RPC) treat
/// `None` as "fall back to a default placeholder."
pub(crate) async fn generate_session_title(
    config: &PeridotConfig,
    project_root: &Path,
    task: &str,
) -> Option<String> {
    let provider = live_provider(config, &config.models.main, project_root)
        .await
        .ok()?;
    let request = CompletionRequest {
        model: config.models.main.clone(),
        system: Some(
            "Generate a concise title (3-8 words) for this coding session. \
             Reply with ONLY the title text, no quotes or extra punctuation."
                .to_string(),
        ),
        messages: vec![LlmMessage::new(MessageRole::User, task)],
        max_tokens: Some(30),
        thinking: false,
        reasoning_effort: peridot_common::ReasoningEffort::Off,
        service_tier: None,
        tools: Vec::new(),
        tool_choice: Default::default(),
    };
    let response = provider.complete(request).await.ok()?;
    let title = response.text.trim().to_string();
    if title.is_empty() { None } else { Some(title) }
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
    compact_request: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
    ask_user_pending: AskUserPending,
    ask_user_next_id: std::sync::Arc<std::sync::atomic::AtomicU64>,
    generate_title: bool,
) {
    let context_snapshot_path = Some(
        project_root
            .join(".peridot")
            .join("sessions")
            .join(&session_id)
            .join("context.bin"),
    );
    let ask_user_port: std::sync::Arc<dyn AskUserPort> = std::sync::Arc::new(TuiAskUserPort {
        session_id: session_id.clone(),
        event_tx: event_tx.clone(),
        next_id: ask_user_next_id,
        pending: ask_user_pending,
    });
    // Wire the SessionRouter-backed message bus so this session's
    // `agent_message` calls route to its registered parent/children,
    // and so its inbox is drained at the start of every turn.
    let message_bus: run_loop::MessageBusHookup = {
        let bus = std::sync::Arc::new(
            session_router::RouterMessageBus::new(router.clone())
                .with_current_session(session_id.clone()),
        );
        Some((
            bus as std::sync::Arc<dyn peridot_tools::AgentMessageBus>,
            session_id.clone(),
        ))
    };
    handle.spawn(async move {
        let event_sender = event_tx.clone();
        let session = session_id.clone();
        let router_for_events = router.clone();
        let title_task = if generate_title {
            Some(task.clone())
        } else {
            None
        };
        let title_config = if generate_title {
            Some(config.clone())
        } else {
            None
        };
        let title_project_root = if generate_title {
            Some(project_root.clone())
        } else {
            None
        };
        let skill_project_root = project_root.clone();
        let result = run_task_with_events(
            task,
            mode,
            options,
            config,
            project_root,
            cancel,
            compact_request,
            context_snapshot_path,
            Some(ask_user_port),
            message_bus,
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
        if let Err(err) = &result {
            let _ = event_tx.send((
                session_id.clone(),
                TuiRuntimeEvent::Failed {
                    message: err.to_string(),
                },
            ));
        }
        let _ = event_tx.send((
            session_id.clone(),
            TuiRuntimeEvent::SkillSuggestionsUpdated {
                skills: load_auto_skill_suggestions(&skill_project_root),
            },
        ));
        if result.is_ok()
            && let (Some(task_text), Some(cfg), Some(root)) =
                (title_task, title_config, title_project_root)
            && let Some(title) = generate_session_title(&cfg, &root, &task_text).await
        {
            let _ = event_tx.send((
                session_id.clone(),
                TuiRuntimeEvent::SessionTitleUpdated { session_id, title },
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
    ask_user_pending: &AskUserPending,
    ask_user_next_id: &std::sync::Arc<std::sync::atomic::AtomicU64>,
) {
    let effective_config = config_for_state(config_template, state);
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
                    state.reasoning_effort,
                    state.service_tier.clone(),
                    options_template,
                    config_template,
                    project_template,
                    ask_user_pending,
                    ask_user_next_id,
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
        SessionCommandEvent::SessionClose(target) | SessionCommandEvent::SessionDelete(target) => {
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
                    delete_persisted_session(project_template, &id);
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
        SessionCommandEvent::SessionRename { target, title } => {
            let title = title.trim().to_string();
            if title.is_empty() {
                state.push_error("session: title cannot be empty".to_string());
                return;
            }
            let resolved = resolve_session_id(state, &target);
            if let Some(id) = resolved {
                if let Some(item) = state.sessions.iter_mut().find(|item| item.id == id) {
                    item.title = title.clone();
                    item.title_generated = true;
                }
                rename_persisted_session(project_template, &id, &title);
                state.push_transcript(format!("session: renamed {id} to {title}"));
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
                state.reasoning_effort,
                state.service_tier.clone(),
                options_template,
                config_template,
                project_template,
                ask_user_pending,
                ask_user_next_id,
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
                ask_user_pending,
                ask_user_next_id,
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
                ask_user_pending,
                ask_user_next_id,
            );
        }
        SessionCommandEvent::McpList => {
            handle_mcp_list(state, project_template);
        }
        SessionCommandEvent::McpAdd {
            name,
            transport,
            target,
        } => {
            handle_mcp_add(state, project_template, &name, &transport, &target);
        }
        SessionCommandEvent::McpRemove(name) => {
            handle_mcp_remove(state, project_template, &name);
        }
        SessionCommandEvent::McpTest(name) => {
            handle_mcp_test(handle, state, project_template, &name);
        }
        SessionCommandEvent::ScanTodos => {
            handle_scan_todos(state, project_template);
        }
        SessionCommandEvent::CodeMap => {
            handle_code_map(state, project_template, false);
        }
        SessionCommandEvent::CodeMapRefresh => {
            handle_code_map(state, project_template, true);
        }
        SessionCommandEvent::CodeMapFind(query) => {
            handle_code_map_find(state, project_template, &query);
        }
        SessionCommandEvent::Attach(path) => {
            handle_attach(state, project_template, &path);
        }
        SessionCommandEvent::Attachments => {
            handle_attachments(state, project_template);
        }
        SessionCommandEvent::Detach(path) => {
            handle_detach(state, project_template, &path);
        }
        SessionCommandEvent::BranchSave(name) => {
            handle_branch_save(state, project_template, &name);
        }
        SessionCommandEvent::BranchRestore(name) => {
            handle_branch_restore(state, project_template, &name);
        }
        SessionCommandEvent::BranchList => {
            handle_branch_list(state, project_template);
        }
        SessionCommandEvent::BranchTurn(turn_id) => {
            handle_branch_turn(state, project_template, turn_id);
        }
        SessionCommandEvent::BranchTree => {
            handle_branch_tree(state, project_template);
        }
        SessionCommandEvent::BranchSwitch(index) => {
            handle_branch_switch(state, project_template, index);
        }
        SessionCommandEvent::BranchPickerOpen => {
            handle_branch_picker_open(state, project_template, event_tx);
        }
        SessionCommandEvent::CompactContext => {
            handle_compact_context(state, router);
        }
        SessionCommandEvent::Skill { name, args } => {
            handle_skill_load(state, project_template, &name, &args);
        }
        SessionCommandEvent::ContextTop => {
            handle_context_top(state, project_template);
        }
        SessionCommandEvent::UndoLastCheckpoint => {
            handle_undo_last_checkpoint(state, project_template);
        }
        SessionCommandEvent::ClearAndRestart => {
            // 1. Cancel anything running on the active session so the
            //    LLM call returns immediately (the streaming helper
            //    races this cancel via tokio::select!).
            let old_id = state.current_session_id.clone();
            if !old_id.is_empty() {
                {
                    let router = router.lock().unwrap();
                    if let Some(handle) = router.get(&old_id) {
                        handle.cancel.cancel();
                    }
                }
                // 2. Close the old session and drop its persisted
                //    context snapshot so the next session starts
                //    from a truly empty memory.
                {
                    let mut router = router.lock().unwrap();
                    router.close(&old_id);
                }
                delete_persisted_session(project_template, &old_id);
                state.sessions.retain(|item| item.id != old_id);
            }
            // 3. Register a fresh session in the same workspace
            //    template. The id format mirrors `SessionNew`.
            let new_id = format!("session-{}-{}", std::process::id(), unix_timestamp());
            router.lock().unwrap().register(SessionHandle::new(
                new_id.clone(),
                project_template.to_path_buf(),
                WorkspaceIsolation::Shared,
            ));
            state
                .sessions
                .push(SessionDirectoryItem::new(&new_id, "new session"));
            state.current_session_id = new_id.clone();
            state.push_transcript(format!("session: opened {new_id} (cleared)"));
        }
    }
    warn_on_shared_workspace_collisions(state, router, project_template);
}

fn context_snapshot_path(project_root: &Path, session_id: &str) -> PathBuf {
    project_root
        .join(".peridot/sessions")
        .join(session_id)
        .join("context.bin")
}

fn branch_journal_path(project_root: &Path, session_id: &str) -> PathBuf {
    project_root
        .join(".peridot/sessions")
        .join(session_id)
        .join("branches.json")
}

fn read_context_snapshot(
    project_root: &Path,
    session_id: &str,
) -> Result<Vec<peridot_context::ContextEntry>, String> {
    let snapshot_path = context_snapshot_path(project_root, session_id);
    if !snapshot_path.exists() {
        return Err("no context snapshot has been written for this session yet".to_string());
    }
    let bytes = std::fs::read(&snapshot_path)
        .map_err(|err| format!("failed to read {}: {err}", snapshot_path.display()))?;
    serde_json::from_slice(&bytes)
        .map_err(|err| format!("failed to parse {}: {err}", snapshot_path.display()))
}

fn write_context_snapshot(
    project_root: &Path,
    session_id: &str,
    entries: &[ContextEntry],
) -> Result<(), String> {
    let snapshot_path = context_snapshot_path(project_root, session_id);
    if let Some(parent) = snapshot_path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|err| format!("failed to create {}: {err}", parent.display()))?;
    }
    let bytes = serde_json::to_vec(entries)
        .map_err(|err| format!("failed to serialize context snapshot: {err}"))?;
    std::fs::write(&snapshot_path, bytes)
        .map_err(|err| format!("failed to write {}: {err}", snapshot_path.display()))
}

fn append_plan_reminder_to_context(
    project_root: &Path,
    session_id: &str,
    content: String,
) -> Result<(), String> {
    let mut entries = read_context_snapshot(project_root, session_id).unwrap_or_default();
    entries.push(ContextEntry::trusted(ContextSource::PlanReminder, content));
    write_context_snapshot(project_root, session_id, &entries)
}

fn load_auto_skill_suggestions(project_root: &Path) -> Vec<SkillSlashSuggestion> {
    let store = MemoryStore::new(project_root.join(".peridot/memory.db"));
    let Ok(skills) = store.list_skills() else {
        return Vec::new();
    };
    skills
        .into_iter()
        .filter(|skill| skill.scope == "auto")
        .map(|skill| SkillSlashSuggestion {
            description: skill_description(&skill),
            name: skill.name,
        })
        .collect()
}

fn skill_description(skill: &StoredSkill) -> String {
    if !skill.description.trim().is_empty() {
        return skill.description.trim().to_string();
    }
    skill
        .body
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty() && !line.starts_with('#'))
        .unwrap_or("stored auto-skill")
        .chars()
        .take(120)
        .collect()
}

fn skill_plan_reminder(skill: &StoredSkill, args: &str) -> String {
    let trimmed_args = args.trim();
    if trimmed_args.is_empty() {
        format!("[skill:{}]\n{}", skill.name, skill.body)
    } else {
        format!(
            "[skill:{}]\nOperator passed args: {}\n\n{}",
            skill.name, trimmed_args, skill.body
        )
    }
}

fn estimate_context_tokens(text: &str) -> usize {
    text.len().div_ceil(4)
}

fn context_top_report(
    entries: &[peridot_context::ContextEntry],
    status_tokens: usize,
    status_window: usize,
    limit: usize,
) -> String {
    if entries.is_empty() {
        return "context top: <empty>".to_string();
    }

    let mut source_totals: std::collections::BTreeMap<&'static str, usize> =
        std::collections::BTreeMap::new();
    let mut rows: Vec<(&peridot_context::ContextEntry, usize)> = entries
        .iter()
        .map(|entry| {
            let tokens = estimate_context_tokens(&entry.content);
            *source_totals
                .entry(source_label(&entry.source))
                .or_default() += tokens;
            (entry, tokens)
        })
        .collect();
    rows.sort_by_key(|row| std::cmp::Reverse(row.1));

    let estimated_total: usize = rows.iter().map(|(_, tokens)| *tokens).sum();
    let status = if status_window > 0 {
        format!("status {} / {}", status_tokens, status_window)
    } else {
        "status <unknown>".to_string()
    };
    let mut report = format!(
        "context top: {} entries · estimated {} tok · {status}\nby source:",
        entries.len(),
        estimated_total
    );
    for (source, tokens) in source_totals {
        report.push_str(&format!("\n  {source}: {tokens} tok"));
    }
    report.push_str("\nlargest entries:");
    for (index, (entry, tokens)) in rows.into_iter().take(limit.max(1)).enumerate() {
        let marker = if entry.untrusted { " untrusted" } else { "" };
        let tool = entry
            .tool_call_id
            .as_deref()
            .map(|id| format!(" · call {id}"))
            .unwrap_or_default();
        report.push_str(&format!(
            "\n  {}. {} turn {} · {} tok{}{} · {}",
            index + 1,
            source_label(&entry.source),
            entry.turn_id,
            tokens,
            marker,
            tool,
            preview_line(&entry.content, 120)
        ));
    }
    report
}

fn handle_context_top(state: &mut TuiState, project_root: &Path) {
    let session_id = state.current_session_id.clone();
    if session_id.is_empty() {
        state.push_error("context top: no active session".to_string());
        return;
    }
    match read_context_snapshot(project_root, &session_id) {
        Ok(entries) => {
            let status_tokens = state.side_panel.context_tokens_used;
            let status_window = state.side_panel.context_tokens_window;
            state.push_transcript(context_top_report(
                &entries,
                status_tokens,
                status_window,
                10,
            ));
        }
        Err(message) => state.push_error(format!("context top: {message}")),
    }
}

fn handle_undo_last_checkpoint(state: &mut TuiState, project_root: &Path) {
    match restore_latest_checkpoint(project_root) {
        Ok(message) => state.push_transcript(message),
        Err(err) => state.push_error(format!("undo: {err}")),
    }
}

/// Loads the session's context snapshot and pushes the resulting
/// turn list back to the TUI as `BranchPickerTurns`. Each entry is
/// summarised to a single short line (source + first 80 chars) so it
/// fits cleanly on a list row.
fn handle_branch_picker_open(
    state: &mut TuiState,
    project_root: &Path,
    event_tx: &std::sync::mpsc::Sender<(String, TuiRuntimeEvent)>,
) {
    let session_id = state.current_session_id.clone();
    if session_id.is_empty() {
        state.push_error("branch picker: no active session id".to_string());
        state.branch_picker = None;
        return;
    }
    let snapshot_path = context_snapshot_path(project_root, &session_id);
    if !snapshot_path.exists() {
        state.push_error("branch picker: no snapshot to fork from".to_string());
        state.branch_picker = None;
        return;
    }
    let bytes = match std::fs::read(&snapshot_path) {
        Ok(bytes) => bytes,
        Err(err) => {
            state.push_error(format!("branch picker: read error — {err}"));
            state.branch_picker = None;
            return;
        }
    };
    let entries: Vec<peridot_context::ContextEntry> = match serde_json::from_slice(&bytes) {
        Ok(entries) => entries,
        Err(err) => {
            state.push_error(format!("branch picker: parse error — {err}"));
            state.branch_picker = None;
            return;
        }
    };
    let mut seen: std::collections::BTreeMap<u64, peridot_tui::BranchPickerTurn> =
        std::collections::BTreeMap::new();
    for entry in entries {
        let id = entry.turn_id;
        seen.entry(id)
            .or_insert_with(|| peridot_tui::BranchPickerTurn {
                turn_id: id,
                source: source_label(&entry.source).to_string(),
                preview: preview_line(&entry.content, 80),
            });
    }
    let turns: Vec<peridot_tui::BranchPickerTurn> = seen.into_values().collect();
    let _ = event_tx.send((session_id, TuiRuntimeEvent::BranchPickerTurns { turns }));
}

fn source_label(source: &peridot_context::ContextSource) -> &'static str {
    match source {
        peridot_context::ContextSource::User => "user",
        peridot_context::ContextSource::Assistant => "assistant",
        peridot_context::ContextSource::Tool => "tool",
        peridot_context::ContextSource::PlanReminder => "plan",
        peridot_context::ContextSource::ReviewerComment => "review",
        peridot_context::ContextSource::External => "external",
        peridot_context::ContextSource::SubAgentSummary => "subagent",
    }
}

fn preview_line(content: &str, max_chars: usize) -> String {
    let single = content.replace(['\n', '\r', '\t'], " ");
    let trimmed = single.trim();
    if trimmed.chars().count() <= max_chars {
        trimmed.to_string()
    } else {
        let head: String = trimmed.chars().take(max_chars).collect();
        format!("{head}…")
    }
}

/// Sets the active session's compact-request flag so the running
/// agent loop performs a forced LLM recap on its next turn boundary.
/// No-op when there is no active session — the operator gets a
/// transcript notice either way.
fn handle_compact_context(
    state: &mut TuiState,
    router: &std::sync::Arc<std::sync::Mutex<SessionRouter>>,
) {
    let session_id = state.current_session_id.clone();
    if session_id.is_empty() {
        state.push_error("compact: no active session".to_string());
        return;
    }
    let queued = {
        let mut router = router.lock().unwrap();
        if let Some(handle) = router.get_mut(&session_id) {
            handle
                .compact_request
                .store(true, std::sync::atomic::Ordering::SeqCst);
            true
        } else {
            false
        }
    };
    if queued {
        state.push_transcript("compact: flag set — will fire on next turn");
    } else {
        state.push_error(format!("compact: session {session_id} not found"));
    }
}

fn handle_skill_load(state: &mut TuiState, project_root: &Path, name: &str, args: &str) {
    let session_id = state.current_session_id.clone();
    if session_id.is_empty() {
        state.push_error("skill: no active session".to_string());
        return;
    }
    let store = MemoryStore::new(project_root.join(".peridot/memory.db"));
    let active = match store.list_skills() {
        Ok(skills) => skills,
        Err(err) => {
            state.push_error(format!("skill `{name}`: failed to read skill store: {err}"));
            return;
        }
    };
    let Some(skill) = active.into_iter().find(|skill| skill.name == name) else {
        state.push_error(format!(
            "skill not found: {name}. Run `peridot run \"...\"` once to build relevant auto-skills, or type `/help`."
        ));
        return;
    };
    if let Err(err) = append_plan_reminder_to_context(
        project_root,
        &session_id,
        skill_plan_reminder(&skill, args),
    ) {
        state.push_error(format!("skill `{name}`: failed to update context: {err}"));
        return;
    }
    let _ = store.mark_skill_viewed(&skill.name, unix_timestamp());
    state.set_skill_suggestions(load_auto_skill_suggestions(project_root));
    let args_note = if args.trim().is_empty() {
        String::new()
    } else {
        format!(" with args `{}`", args.trim())
    };
    state.push_transcript(format!("Loaded skill `{}`{args_note}", skill.name));
}

/// Forks the live session's context at the given turn id by rewriting
/// the snapshot to contain only entries from turns `<= turn_id`. The
/// agent picks the truncated context up on its next run; the dropped
/// entries are surfaced in the transcript so the operator sees what
/// was abandoned. Tied to slash command `/branch turn <id>`.
fn handle_branch_turn(state: &mut TuiState, project_root: &Path, turn_id: u64) {
    let session_id = state.current_session_id.clone();
    if session_id.is_empty() {
        state.push_error("branch turn: no active session id".to_string());
        return;
    }
    let snapshot_path = context_snapshot_path(project_root, &session_id);
    if !snapshot_path.exists() {
        state.push_error("branch turn: no context snapshot to fork from".to_string());
        return;
    }
    let bytes = match std::fs::read(&snapshot_path) {
        Ok(bytes) => bytes,
        Err(err) => {
            state.push_error(format!("branch turn: failed to read snapshot — {err}"));
            return;
        }
    };
    let entries: Vec<peridot_context::ContextEntry> = match serde_json::from_slice(&bytes) {
        Ok(entries) => entries,
        Err(err) => {
            state.push_error(format!("branch turn: snapshot parse error — {err}"));
            return;
        }
    };
    let last_keep = entries.iter().rposition(|entry| entry.turn_id <= turn_id);
    let Some(last_keep) = last_keep else {
        state.push_error(format!(
            "branch turn: turn id {turn_id} not found in snapshot"
        ));
        return;
    };
    let kept = &entries[..=last_keep];
    let dropped_entries: Vec<peridot_context::ContextEntry> = entries[last_keep + 1..].to_vec();
    let dropped_count = dropped_entries.len();
    if !dropped_entries.is_empty() {
        let journal_path = branch_journal_path(project_root, &session_id);
        let mut journal = peridot_context::BranchJournal::load(&journal_path);
        journal.record(turn_id, dropped_entries);
        if let Err(err) = journal.save(&journal_path) {
            state.push_error(format!("branch turn: journal write error — {err}"));
        }
    }
    let serialized = match serde_json::to_vec(kept) {
        Ok(bytes) => bytes,
        Err(err) => {
            state.push_error(format!("branch turn: serialise error — {err}"));
            return;
        }
    };
    if let Err(err) = std::fs::write(&snapshot_path, &serialized) {
        state.push_error(format!("branch turn: write error — {err}"));
        return;
    }
    state.push_transcript(format!(
        "branch turn: forked at turn {turn_id} ({dropped_count} entries saved to journal)"
    ));
}

fn handle_branch_tree(state: &mut TuiState, project_root: &Path) {
    let session_id = state.current_session_id.clone();
    if session_id.is_empty() {
        state.push_error("branch tree: no active session id".to_string());
        return;
    }
    let journal_path = branch_journal_path(project_root, &session_id);
    let journal = peridot_context::BranchJournal::load(&journal_path);
    if journal.limbs.is_empty() {
        state.push_transcript(
            "branch tree: no abandoned limbs yet — fork with `/branch turn <id>` first",
        );
        return;
    }
    let mut lines = vec![format!("branch tree: {} limb(s)", journal.limbs.len())];
    lines.extend(journal.tree_summary());
    state.push_transcript(lines.join("\n"));
}

fn handle_branch_switch(state: &mut TuiState, project_root: &Path, index: usize) {
    let session_id = state.current_session_id.clone();
    if session_id.is_empty() {
        state.push_error("branch switch: no active session id".to_string());
        return;
    }
    let snapshot_path = context_snapshot_path(project_root, &session_id);
    if !snapshot_path.exists() {
        state.push_error("branch switch: no context snapshot".to_string());
        return;
    }
    let journal_path = branch_journal_path(project_root, &session_id);
    let mut journal = peridot_context::BranchJournal::load(&journal_path);
    let Some(limb) = journal.take_limb(index) else {
        state.push_error(format!(
            "branch switch: limb [{index}] not found (have {} limbs)",
            journal.limbs.len()
        ));
        return;
    };
    let bytes = match std::fs::read(&snapshot_path) {
        Ok(b) => b,
        Err(err) => {
            state.push_error(format!("branch switch: read snapshot — {err}"));
            return;
        }
    };
    let current_entries: Vec<peridot_context::ContextEntry> = match serde_json::from_slice(&bytes) {
        Ok(e) => e,
        Err(err) => {
            state.push_error(format!("branch switch: parse snapshot — {err}"));
            return;
        }
    };
    let fork_turn = limb.parent_turn_id;
    let last_keep = current_entries
        .iter()
        .rposition(|entry| entry.turn_id <= fork_turn);
    let Some(last_keep) = last_keep else {
        state.push_error(format!(
            "branch switch: fork point turn {fork_turn} not in current snapshot"
        ));
        journal.limbs.insert(index, limb);
        return;
    };
    let current_tail: Vec<peridot_context::ContextEntry> =
        current_entries[last_keep + 1..].to_vec();
    if !current_tail.is_empty() {
        journal.record(fork_turn, current_tail);
    }
    let mut new_entries = current_entries[..=last_keep].to_vec();
    new_entries.extend(limb.entries);
    let serialized = match serde_json::to_vec(&new_entries) {
        Ok(b) => b,
        Err(err) => {
            state.push_error(format!("branch switch: serialise — {err}"));
            return;
        }
    };
    if let Err(err) = std::fs::write(&snapshot_path, &serialized) {
        state.push_error(format!("branch switch: write — {err}"));
        return;
    }
    if let Err(err) = journal.save(&journal_path) {
        state.push_error(format!("branch switch: journal write — {err}"));
    }
    state.push_transcript(format!(
        "branch switch: swapped to limb [{index}] (fork@turn {fork_turn}). Submit your next task to continue."
    ));
}

/// Validates a branch name — bare-word identifiers only so a malicious
/// or fat-fingered `/branch save ../../etc/passwd` doesn't escape the
/// `.peridot/branches/` directory.
fn validate_branch_name(name: &str) -> Result<(), String> {
    if name.is_empty() {
        return Err("branch name must not be empty".to_string());
    }
    if name
        .chars()
        .any(|c| matches!(c, '/' | '\\' | '.' | ':' | ' '))
    {
        return Err(format!(
            "branch name '{name}' contains forbidden character (only ASCII letters / digits / `-` / `_` allowed)"
        ));
    }
    Ok(())
}

/// Copies the live session's `context.bin` snapshot into
/// `.peridot/branches/<name>/context.bin` so it can be restored later.
/// Refuses to overwrite an existing branch — operators must remove the
/// old one explicitly to avoid clobbering work.
fn handle_branch_save(state: &mut TuiState, project_root: &Path, name: &str) {
    if let Err(err) = validate_branch_name(name) {
        state.push_error(format!("branch save: {err}"));
        return;
    }
    let session_id = state.current_session_id.clone();
    if session_id.is_empty() {
        state.push_error("branch save: no active session id".to_string());
        return;
    }
    let src = project_root
        .join(".peridot/sessions")
        .join(&session_id)
        .join("context.bin");
    if !src.exists() {
        state.push_error(format!(
            "branch save: no context.bin yet for session {session_id} — submit at least one turn first"
        ));
        return;
    }
    let dst_dir = project_root.join(".peridot/branches").join(name);
    if dst_dir.exists() {
        state.push_error(format!(
            "branch save: '{name}' already exists — remove it manually first"
        ));
        return;
    }
    if let Err(err) = std::fs::create_dir_all(&dst_dir) {
        state.push_error(format!("branch save: create {}: {err}", dst_dir.display()));
        return;
    }
    let dst = dst_dir.join("context.bin");
    if let Err(err) = std::fs::copy(&src, &dst) {
        state.push_error(format!("branch save: copy: {err}"));
        return;
    }
    state.push_transcript(format!("branch: saved '{name}' from session {session_id}"));
}

/// Overwrites the active session's context snapshot with the named
/// branch's context. The TUI checks `is_agent_busy()` before
/// enqueueing, but we re-validate here so a racy command can't slip
/// past — the agent might still be inside `Finished` cleanup when the
/// queue drains, in which case the rename would race with the loop's
/// own snapshot write.
fn handle_branch_restore(state: &mut TuiState, project_root: &Path, name: &str) {
    if let Err(err) = validate_branch_name(name) {
        state.push_error(format!("branch restore: {err}"));
        return;
    }
    let session_id = state.current_session_id.clone();
    if session_id.is_empty() {
        state.push_error("branch restore: no active session id".to_string());
        return;
    }
    let src = project_root
        .join(".peridot/branches")
        .join(name)
        .join("context.bin");
    if !src.exists() {
        state.push_error(format!("branch restore: no branch named '{name}'"));
        return;
    }
    let session_dir = project_root.join(".peridot/sessions").join(&session_id);
    if let Err(err) = std::fs::create_dir_all(&session_dir) {
        state.push_error(format!(
            "branch restore: create {}: {err}",
            session_dir.display()
        ));
        return;
    }
    let dst = session_dir.join("context.bin");
    if let Err(err) = std::fs::copy(&src, &dst) {
        state.push_error(format!("branch restore: copy: {err}"));
        return;
    }
    state.push_transcript(format!(
        "branch: restored '{name}' into session {session_id}. Submit your next task to continue from that point."
    ));
}

/// Lists every branch directory under `.peridot/branches/` along with
/// its creation time (or modification time as a fallback). Sorts by
/// name so the output is stable.
fn handle_branch_list(state: &mut TuiState, project_root: &Path) {
    let dir = project_root.join(".peridot/branches");
    let Ok(entries) = std::fs::read_dir(&dir) else {
        state.push_transcript("branches: <none>");
        return;
    };
    let mut rows: Vec<(String, String)> = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        let stamp = path
            .metadata()
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_secs().to_string())
            .unwrap_or_else(|| "?".to_string());
        rows.push((name, stamp));
    }
    rows.sort();
    if rows.is_empty() {
        state.push_transcript("branches: <none>");
        return;
    }
    let mut lines = vec!["branches:".to_string()];
    for (name, stamp) in rows {
        lines.push(format!("  {name} (unix {stamp})"));
    }
    state.push_transcript(lines.join("\n"));
}

/// Scans every text-ish file under `project_root` for the canonical
/// TODO / FIXME / HACK / XXX / BUG markers and prints `path:line:
/// trimmed-text` for each hit. Heavy directories (`.git`, `target`,
/// `node_modules`, `.peridot`) are pruned so the scan stays sub-second
/// on a normal project; very large repositories are capped at 500 hits
/// with a "(further hits truncated)" footer so we don't dump a 10k-row
/// wall into the transcript.
fn handle_scan_todos(state: &mut TuiState, project_root: &Path) {
    const MAX_HITS: usize = 500;
    const SKIP_DIRS: &[&str] = &[
        ".git",
        "target",
        "node_modules",
        ".peridot",
        ".idea",
        ".vscode",
    ];
    const MARKERS: &[&str] = &["TODO", "FIXME", "HACK", "XXX", "BUG"];
    let mut hits: Vec<String> = Vec::new();
    let mut walked = 0usize;
    walk_for_todos(
        project_root,
        project_root,
        &mut hits,
        &mut walked,
        SKIP_DIRS,
        MARKERS,
        MAX_HITS,
    );
    if hits.is_empty() {
        state.push_transcript(format!(
            "todos: no markers found (scanned {walked} file(s))"
        ));
        return;
    }
    let mut body = format!("todos: {} hit(s) across {walked} file(s):\n", hits.len());
    body.push_str(&hits.join("\n"));
    if hits.len() == MAX_HITS {
        body.push_str("\n(further hits truncated)");
    }
    state.push_transcript(body);
}

fn handle_code_map(state: &mut TuiState, project_root: &Path, refresh: bool) {
    let index = if refresh {
        commands::refresh_code_map_index(project_root, 120, 80)
    } else {
        commands::load_or_refresh_code_map_index(project_root, 120, 80)
    };
    let Ok(index) = index else {
        state.push_error("codemap: failed to load workspace code map index");
        return;
    };
    let report = &index.report;
    if report.symbols.is_empty() && report.todos.is_empty() {
        state.push_transcript(format!(
            "codemap: no symbols or TODO markers found (scanned {} file(s))",
            report.walked_files
        ));
        return;
    }
    state.push_transcript(render_code_map_text(&index));
}

fn handle_code_map_find(state: &mut TuiState, project_root: &Path, query: &str) {
    let index = commands::load_or_refresh_code_map_index(project_root, 120, 80);
    let Ok(index) = index else {
        state.push_error("codemap: failed to load workspace code map index");
        return;
    };
    let report = commands::search_code_map_index(&index, query);
    if report.symbols.is_empty() && report.todos.is_empty() {
        state.push_transcript(format!(
            "codemap: no matches for '{query}' (indexed at {})",
            index.generated_at_unix
        ));
        return;
    }
    state.push_transcript(render_code_map_report(
        &report,
        index.generated_at_unix,
        Some(query),
    ));
}

fn handle_attach(state: &mut TuiState, project_root: &Path, path: &str) {
    const MAX_ATTACHMENT_BYTES: usize = 64 * 1024;
    match commands::load_text_attachment(project_root, path, MAX_ATTACHMENT_BYTES) {
        Ok(attachment) => {
            if state.current_session_id.is_empty() {
                state.push_error("attach: no active session".to_string());
                return;
            }
            let reminder = commands::attachment_plan_reminder(&attachment);
            match append_plan_reminder_to_context(project_root, &state.current_session_id, reminder)
            {
                Ok(()) => state.push_transcript(format!(
                    "attach: added {} ({} bytes) to session context",
                    attachment.path, attachment.bytes
                )),
                Err(err) => state.push_error(format!("attach: failed to update context: {err}")),
            }
        }
        Err(err) => state.push_error(err),
    }
}

fn handle_attachments(state: &mut TuiState, project_root: &Path) {
    if state.current_session_id.is_empty() {
        state.push_error("attachments: no active session".to_string());
        return;
    }
    match read_context_snapshot(project_root, &state.current_session_id) {
        Ok(entries) => {
            let artifacts = commands::attachments_from_context(&entries);
            state.push_transcript(render_attachments_text(&artifacts));
        }
        Err(err) => state.push_error(format!("attachments: failed to read context: {err}")),
    }
}

fn handle_detach(state: &mut TuiState, project_root: &Path, path: &str) {
    if state.current_session_id.is_empty() {
        state.push_error("detach: no active session".to_string());
        return;
    }
    match read_context_snapshot(project_root, &state.current_session_id) {
        Ok(entries) => {
            let (kept, removed) = commands::detach_attachments_from_context(entries, path);
            if removed.is_empty() {
                state.push_transcript(format!("detach: no attachment matched {path}"));
                return;
            }
            match write_context_snapshot(project_root, &state.current_session_id, &kept) {
                Ok(()) => state.push_transcript(format!(
                    "detach: removed {} attachment(s) matching {}",
                    removed.len(),
                    path
                )),
                Err(err) => state.push_error(format!("detach: failed to update context: {err}")),
            }
        }
        Err(err) => state.push_error(format!("detach: failed to read context: {err}")),
    }
}

fn render_attachments_text(artifacts: &[commands::AttachmentArtifact]) -> String {
    if artifacts.is_empty() {
        return "attachments: no files attached to this session".to_string();
    }
    let mut body = format!(
        "attachments: {} file(s) in session context:",
        artifacts.len()
    );
    for artifact in artifacts {
        let mode = if artifact.inlined {
            "inlined"
        } else {
            "placeholder"
        };
        body.push_str(&format!(
            "\n{}  {} bytes  {}  {}",
            artifact.path, artifact.bytes, artifact.media_type, mode
        ));
    }
    body
}

fn render_code_map_text(index: &commands::CodeMapIndex) -> String {
    render_code_map_report(&index.report, index.generated_at_unix, None)
}

fn render_code_map_report(
    report: &commands::CodeMapReport,
    generated_at_unix: u64,
    query: Option<&str>,
) -> String {
    let mut body = if let Some(query) = query {
        format!(
            "codemap: {} symbol match(es), {} TODO match(es) for '{}' across {} file(s) (indexed at {})",
            report.symbols.len(),
            report.todos.len(),
            query,
            report.walked_files,
            generated_at_unix,
        )
    } else {
        format!(
            "codemap: {} symbol(s), {} TODO marker(s) across {} file(s) (indexed at {})",
            report.symbols.len(),
            report.todos.len(),
            report.walked_files,
            generated_at_unix,
        )
    };
    if !report.symbols.is_empty() {
        body.push_str("\n\nSymbols:");
        for symbol in report.symbols.iter().take(40) {
            body.push_str(&format!(
                "\n{}:{}  {}  {}",
                symbol.path, symbol.line, symbol.kind, symbol.name
            ));
        }
        if report.symbols.len() > 40 || report.symbols_truncated {
            body.push_str("\n(symbols truncated)");
        }
    }
    if !report.todos.is_empty() {
        body.push_str("\n\nTODOs:");
        for todo in report.todos.iter().take(20) {
            body.push_str(&format!("\n{}:{}  {}", todo.path, todo.line, todo.text));
        }
        if report.todos.len() > 20 || report.todos_truncated {
            body.push_str("\n(TODO markers truncated)");
        }
    }
    body
}

#[allow(clippy::too_many_arguments)]
fn walk_for_todos(
    root: &Path,
    dir: &Path,
    hits: &mut Vec<String>,
    walked: &mut usize,
    skip_dirs: &[&str],
    markers: &[&str],
    cap: usize,
) {
    if hits.len() >= cap {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        if hits.len() >= cap {
            return;
        }
        let path = entry.path();
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        let name = entry.file_name();
        let name_str = name.to_string_lossy();
        if file_type.is_dir() {
            if skip_dirs.iter().any(|s| *s == name_str) {
                continue;
            }
            if name_str.starts_with('.') {
                continue;
            }
            walk_for_todos(root, &path, hits, walked, skip_dirs, markers, cap);
            continue;
        }
        if !file_type.is_file() || name_str.starts_with('.') {
            continue;
        }
        // Heuristic skip: anything larger than 1 MiB is probably a
        // binary asset or generated artefact; we don't want to read it.
        if entry.metadata().map(|m| m.len()).unwrap_or(0) > 1_000_000 {
            continue;
        }
        let Ok(content) = std::fs::read_to_string(&path) else {
            continue;
        };
        *walked += 1;
        let rel = path
            .strip_prefix(root)
            .unwrap_or(&path)
            .to_string_lossy()
            .replace('\\', "/");
        for (idx, line) in content.lines().enumerate() {
            if hits.len() >= cap {
                return;
            }
            if markers.iter().any(|m| line.contains(m)) {
                let snippet = line.trim();
                hits.push(format!("  {rel}:{}: {snippet}", idx + 1));
            }
        }
    }
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
    ask_user_pending: &AskUserPending,
    ask_user_next_id: &std::sync::Arc<std::sync::atomic::AtomicU64>,
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
        state.reasoning_effort,
        state.service_tier.clone(),
        options_template,
        config_template,
        &worktree_path,
        ask_user_pending,
        ask_user_next_id,
    );
}

/// Reads the project-local `config.toml` and renders one transcript line
/// per configured MCP server (or "<none>"). Reads through `peridot_common`
/// so we get the same `PeridotConfig` shape the live agent uses.
fn handle_mcp_list(state: &mut TuiState, project_root: &Path) {
    let path = project_root.join(".peridot/config.toml");
    let config = match read_project_config(&path) {
        Ok(config) => config,
        Err(err) => {
            state.push_error(format!("mcp list: {err}"));
            return;
        }
    };
    if config.mcp.is_empty() {
        state.push_transcript("mcp: <none configured>");
        return;
    }
    let mut lines = vec!["mcp servers:".to_string()];
    for entry in &config.mcp {
        let detail = match entry.transport {
            peridot_common::McpTransport::Stdio => entry.command.clone().unwrap_or_default(),
            peridot_common::McpTransport::Http => entry.url.clone().unwrap_or_default(),
        };
        lines.push(format!("  {} [{}] {}", entry.name, entry.transport, detail));
    }
    state.push_transcript(lines.join("\n"));
}

/// Appends a new `[[mcp]]` block to the project-local `config.toml`.
/// We deliberately do NOT round-trip through `PeridotConfig` serialisation
/// because that would expand every `#[serde(default)]` field and rewrite
/// the user's hand-edited toml. Instead we render just the new block,
/// optionally append it to the existing file, and validate against the
/// already-loaded config to refuse duplicates.
fn handle_mcp_add(
    state: &mut TuiState,
    project_root: &Path,
    name: &str,
    transport: &str,
    target: &str,
) {
    let path = project_root.join(".peridot/config.toml");
    let existing = match read_project_config(&path) {
        Ok(config) => config,
        Err(err) => {
            state.push_error(format!("mcp add: {err}"));
            return;
        }
    };
    if existing.mcp.iter().any(|m| m.name == name) {
        state.push_error(format!(
            "mcp add: '{name}' already configured — use /mcp remove first"
        ));
        return;
    }
    let block = match transport.to_ascii_lowercase().as_str() {
        "stdio" => {
            let mut parts = target.split_whitespace();
            let Some(command) = parts.next() else {
                state.push_error("mcp add: stdio transport requires a command".to_string());
                return;
            };
            let args: Vec<&str> = parts.collect();
            let args_toml = if args.is_empty() {
                String::new()
            } else {
                let quoted = args
                    .iter()
                    .map(|a| format!("\"{}\"", a.replace('"', "\\\"")))
                    .collect::<Vec<_>>()
                    .join(", ");
                format!("args = [{quoted}]\n")
            };
            format!(
                "\n[[mcp]]\nname = \"{}\"\ntransport = \"stdio\"\ncommand = \"{}\"\n{}",
                escape_toml_string(name),
                escape_toml_string(command),
                args_toml,
            )
        }
        "http" | "sse" => format!(
            "\n[[mcp]]\nname = \"{}\"\ntransport = \"http\"\nurl = \"{}\"\n",
            escape_toml_string(name),
            escape_toml_string(target),
        ),
        other => {
            state.push_error(format!(
                "mcp add: unknown transport '{other}' (use stdio or http)"
            ));
            return;
        }
    };
    let existing_content = match std::fs::read_to_string(&path) {
        Ok(content) => content,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(err) => {
            state.push_error(format!("mcp add: read {}: {err}", path.display()));
            return;
        }
    };
    let new_content = if existing_content.is_empty() {
        block.trim_start_matches('\n').to_string()
    } else if existing_content.ends_with('\n') {
        format!("{existing_content}{block}")
    } else {
        format!("{existing_content}\n{block}")
    };
    if let Err(err) = atomic_write(&path, &new_content) {
        state.push_error(format!("mcp add: write {}: {err}", path.display()));
        return;
    }
    state.push_transcript(format!(
        "mcp: added '{name}' to {}. Restart this session for it to take effect.",
        path.display()
    ));
}

/// Removes the named MCP server from `config.toml` by scanning for the
/// `[[mcp]]` block whose `name = "<name>"` line matches. Like
/// `handle_mcp_add`, this works directly on the raw text so the rest of
/// the operator's config keeps its original formatting / comments.
fn handle_mcp_remove(state: &mut TuiState, project_root: &Path, name: &str) {
    let path = project_root.join(".peridot/config.toml");
    let content = match std::fs::read_to_string(&path) {
        Ok(content) => content,
        Err(err) => {
            state.push_error(format!("mcp remove: read {}: {err}", path.display()));
            return;
        }
    };
    let Some(new_content) = remove_mcp_block(&content, name) else {
        state.push_error(format!("mcp remove: no server named '{name}'"));
        return;
    };
    if let Err(err) = atomic_write(&path, &new_content) {
        state.push_error(format!("mcp remove: write {}: {err}", path.display()));
        return;
    }
    state.push_transcript(format!(
        "mcp: removed '{name}' from {}. Restart this session to drop its tools from the registry.",
        path.display()
    ));
}

/// Walks the toml text line by line and drops the `[[mcp]]` block whose
/// `name = "<target>"` line matches. Returns `None` when no block names
/// the target so the caller can surface a "no such server" error.
fn remove_mcp_block(content: &str, target: &str) -> Option<String> {
    let lines: Vec<&str> = content.lines().collect();
    let mut blocks: Vec<(usize, usize, Option<String>)> = Vec::new();
    let mut current_start: Option<usize> = None;
    let mut current_name: Option<String> = None;
    for (idx, line) in lines.iter().enumerate() {
        let trimmed = line.trim();
        if trimmed == "[[mcp]]" {
            if let Some(start) = current_start.take() {
                blocks.push((start, idx, current_name.take()));
            }
            current_start = Some(idx);
        } else if let Some(name_value) = trimmed
            .strip_prefix("name")
            .and_then(|s| s.trim_start().strip_prefix('='))
            .map(|s| s.trim().trim_matches('"'))
            && current_start.is_some()
            && current_name.is_none()
        {
            current_name = Some(name_value.to_string());
        } else if trimmed.starts_with("[[") || trimmed.starts_with("[") {
            // New top-level block — close the current mcp block (if any).
            if let Some(start) = current_start.take() {
                blocks.push((start, idx, current_name.take()));
            }
        }
    }
    if let Some(start) = current_start.take() {
        blocks.push((start, lines.len(), current_name.take()));
    }
    let (start, end, _) = blocks
        .into_iter()
        .find(|(_, _, name)| name.as_deref() == Some(target))?;
    let mut kept: Vec<&str> = Vec::with_capacity(lines.len());
    kept.extend(lines.iter().take(start).copied());
    kept.extend(lines.iter().skip(end).copied());
    let mut result = kept.join("\n");
    if content.ends_with('\n') {
        result.push('\n');
    }
    Some(result)
}

/// Toml string-literal escaper covering the cases we encounter for MCP
/// names / commands / URLs (`"` and `\`). Sufficient for the constrained
/// inputs the slash command accepts — names are bare words, commands /
/// URLs typically don't contain control characters.
fn escape_toml_string(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}

/// Writes `content` to `path` atomically via `<path>.tmp` + rename so a
/// mid-write crash never leaves the live config truncated.
fn atomic_write(path: &Path, content: &str) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|err| format!("create {}: {err}", parent.display()))?;
    }
    let temp = path.with_extension("toml.tmp");
    std::fs::write(&temp, content).map_err(|err| format!("write {}: {err}", temp.display()))?;
    std::fs::rename(&temp, path)
        .map_err(|err| format!("rename {} -> {}: {err}", temp.display(), path.display()))
}

/// One-shot connectivity probe: constructs `peridot_mcp::McpClient` from
/// the named entry, calls `list_tools`, and reports the count or error.
/// Spawned on the runtime handle so we don't block the TUI event loop on
/// network I/O.
fn handle_mcp_test(
    handle: &tokio::runtime::Handle,
    state: &mut TuiState,
    project_root: &Path,
    name: &str,
) {
    let path = project_root.join(".peridot/config.toml");
    let config = match read_project_config(&path) {
        Ok(config) => config,
        Err(err) => {
            state.push_error(format!("mcp test: {err}"));
            return;
        }
    };
    let Some(entry) = config.mcp.iter().find(|m| m.name == name).cloned() else {
        state.push_error(format!("mcp test: no server named '{name}'"));
        return;
    };
    let probe = handle.block_on(async move {
        let client = peridot_mcp::McpClient::new(entry.clone());
        client.list_tools().await.map(|tools| tools.len())
    });
    match probe {
        Ok(count) => {
            state.push_transcript(format!("mcp: '{name}' reachable — {count} tool(s) exposed"))
        }
        Err(err) => state.push_error(format!("mcp test '{name}': {err}")),
    }
}

/// Loads the project-local `config.toml`, returning a default-populated
/// `PeridotConfig` when the file is missing so subsequent writes create
/// it from scratch. Surfaces a friendly error on malformed toml.
fn read_project_config(path: &Path) -> Result<PeridotConfig, String> {
    match std::fs::read_to_string(path) {
        Ok(content) => toml::from_str::<PeridotConfig>(&content)
            .map_err(|err| format!("failed to parse {}: {err}", path.display())),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(PeridotConfig::default()),
        Err(err) => Err(format!("failed to read {}: {err}", path.display())),
    }
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
    if matches!(
        lifecycle,
        SessionLifecycle::Done | SessionLifecycle::Failed | SessionLifecycle::Suspended
    ) {
        let _ = memory.set_meta("last_session_end_unix", &unix_timestamp().to_string());
    }
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

fn restore_latest_tui_state_from_disk(project_root: &Path) -> anyhow::Result<(String, TuiState)> {
    let memory = MemoryStore::new(project_root.join(".peridot/memory.db"));
    let records = memory.list_session_records()?;
    for record in records {
        if let Ok(restored) = restore_tui_state_from_disk(&record.id, project_root) {
            return Ok(restored);
        }
    }
    anyhow::bail!("no persisted sessions found")
}

fn hydrate_persisted_sessions(
    state: &mut TuiState,
    router: &std::sync::Arc<std::sync::Mutex<SessionRouter>>,
    project_root: &Path,
) {
    let memory = MemoryStore::new(project_root.join(".peridot/memory.db"));
    let Ok(records) = memory.list_session_records() else {
        return;
    };
    let sessions_root = project_root.join(".peridot").join("sessions");
    let mut router = router.lock().unwrap();
    for record in records {
        if peridot_memory::load_session_blob(&sessions_root, &record.id, "tui_state.json")
            .ok()
            .flatten()
            .is_none()
        {
            continue;
        }
        if !state.sessions.iter().any(|item| item.id == record.id) {
            let title = if record.summary.trim().is_empty() {
                record
                    .last_task
                    .as_deref()
                    .filter(|task| !task.trim().is_empty())
                    .unwrap_or(record.id.as_str())
            } else {
                record.summary.as_str()
            }
            .trim();
            let title = if title.is_empty() {
                record.id.as_str()
            } else {
                title
            };
            let mut item = SessionDirectoryItem::new(&record.id, title);
            item.status = agent_status_from_lifecycle(record.status);
            item.tokens = record.total_tokens;
            item.cost_usd = record.total_cost_usd;
            item.last_event_at_unix = record.updated_at_unix;
            state.sessions.push(item);
        }
        if router.get(&record.id).is_none() {
            let isolation = match record.worktree_branch.clone() {
                Some(branch) => WorkspaceIsolation::Worktree { branch },
                None => WorkspaceIsolation::Shared,
            };
            let mut handle =
                SessionHandle::new(&record.id, record.workspace_root.clone(), isolation);
            handle.lifecycle = record.status;
            handle.started_at_unix = record.created_at_unix;
            router.register(handle);
        }
    }
    if state.current_session_id.is_empty() {
        state.current_session_id = state
            .sessions
            .first()
            .map(|item| item.id.clone())
            .unwrap_or_default();
    }
    if !state.current_session_id.is_empty() {
        let _ = router.switch_to(&state.current_session_id);
    }
}

fn delete_persisted_session(project_root: &Path, id: &str) {
    let memory = MemoryStore::new(project_root.join(".peridot/memory.db"));
    let _ = memory.delete_session_record(id);
    let _ = memory.delete_session(id);
    let sessions_root = project_root.join(".peridot").join("sessions");
    let _ = peridot_memory::remove_session_dir(&sessions_root, id);
}

fn rename_persisted_session(project_root: &Path, id: &str, title: &str) {
    let memory = MemoryStore::new(project_root.join(".peridot/memory.db"));
    let _ = memory.save_session(&SessionSummary {
        id: id.to_string(),
        summary: title.to_string(),
    });
    if let Ok(Some(mut record)) = memory.get_session_record(id) {
        record.summary = title.to_string();
        record.updated_at_unix = unix_timestamp();
        let _ = memory.save_session_record(&record);
    }
    let sessions_root = project_root.join(".peridot").join("sessions");
    if let Ok(Some(bytes)) = peridot_memory::load_session_blob(&sessions_root, id, "tui_state.json")
        && let Ok(mut state) = serde_json::from_slice::<TuiState>(&bytes)
    {
        for item in &mut state.sessions {
            if item.id == id {
                item.title = title.to_string();
                item.title_generated = true;
            }
        }
        if let Ok(serialized) = serde_json::to_vec(&state) {
            let _ = save_session_blob(&sessions_root, id, "tui_state.json", &serialized);
        }
    }
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

fn agent_status_from_lifecycle(status: SessionLifecycle) -> peridot_tui::AgentRunStatus {
    match status {
        SessionLifecycle::Idle | SessionLifecycle::Suspended => peridot_tui::AgentRunStatus::Idle,
        SessionLifecycle::Running => peridot_tui::AgentRunStatus::Running,
        SessionLifecycle::Done => peridot_tui::AgentRunStatus::Succeeded,
        SessionLifecycle::Failed => peridot_tui::AgentRunStatus::Failed,
    }
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
    reasoning_effort: peridot_common::ReasoningEffort,
    service_tier: Option<String>,
    options_template: &run_loop::AgentTaskOptions,
    config_template: &PeridotConfig,
    project_template: &Path,
    ask_user_pending: &AskUserPending,
    ask_user_next_id: &std::sync::Arc<std::sync::atomic::AtomicU64>,
) {
    let mut options = options_template.clone();
    options.permission = permission;
    options.model = model;
    options.reasoning_effort = reasoning_effort;
    options.service_tier = service_tier;
    let token = peridot_core::CancelToken::new();
    let compact_flag = {
        let mut router = router.lock().unwrap();
        router.get_mut(&session_id).map(|h| {
            h.cancel = token.clone();
            h.compact_request.clone()
        })
    };
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
        compact_flag,
        ask_user_pending.clone(),
        ask_user_next_id.clone(),
        true,
    );
}

fn resolve_session_id(state: &TuiState, target: &str) -> Option<String> {
    state
        .sessions
        .iter()
        .find(|item| item.id == target || item.title == target)
        .map(|item| item.id.clone())
}

fn config_for_state(template: &PeridotConfig, state: &TuiState) -> PeridotConfig {
    let mut config = config_with_provider(template, state.header.provider.as_deref());
    config.models.service_tier = state.service_tier.clone();
    apply_approval_grants(&mut config, &state.approval_grants);
    config
}

fn relax_security_for_approval(config: &mut PeridotConfig, reason: &str) {
    if reason.contains("dependency installation") {
        config.security.ask_before_install = false;
    }
    if reason.contains("destructive shell command") {
        config.security.ask_before_delete = false;
    }
}

fn approval_grant_from_event(
    tool_name: String,
    reason: String,
    scope: ApprovalScope,
    parameters: &serde_json::Value,
) -> ApprovalGrant {
    ApprovalGrant {
        tool_name,
        reason,
        scope,
        command: parameters
            .get("command")
            .and_then(serde_json::Value::as_str)
            .map(normalize_shell_command_for_grant),
        path: parameters
            .get("path")
            .and_then(serde_json::Value::as_str)
            .map(str::to_string),
    }
}

fn apply_approval_grants(config: &mut PeridotConfig, grants: &[ApprovalGrant]) {
    for grant in grants {
        apply_approval_grant_to_config(config, grant);
    }
}

fn apply_approval_grant_to_config(config: &mut PeridotConfig, grant: &ApprovalGrant) {
    match grant.scope {
        ApprovalScope::Once => {
            if let Some(command) = grant.command.as_ref() {
                push_unique_string(
                    &mut config.security.approved_shell_commands,
                    command.clone(),
                );
            } else {
                relax_security_for_approval(config, &grant.reason);
            }
        }
        ApprovalScope::Session => relax_security_for_approval(config, &grant.reason),
        ApprovalScope::Command => {
            if let Some(command) = grant.command.as_ref() {
                push_unique_string(
                    &mut config.security.approved_shell_commands,
                    command.clone(),
                );
            } else {
                relax_security_for_approval(config, &grant.reason);
            }
        }
        ApprovalScope::Path => {
            if let Some(path) = grant.path.as_ref() {
                push_unique_string(
                    &mut config.security.approved_shell_path_scopes,
                    path.clone(),
                );
            } else if let Some(command) = grant.command.as_ref() {
                push_unique_string(
                    &mut config.security.approved_shell_commands,
                    command.clone(),
                );
            } else {
                relax_security_for_approval(config, &grant.reason);
            }
        }
    }
}

fn push_unique_string(values: &mut Vec<String>, value: String) {
    if !values.iter().any(|existing| existing == &value) {
        values.push(value);
    }
}

fn normalize_shell_command_for_grant(command: &str) -> String {
    command.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn tui_runtime_event_from_agent(event: AgentRunEvent) -> TuiRuntimeEvent {
    match event {
        AgentRunEvent::RunStarted { task } => TuiRuntimeEvent::RunStarted { task },
        AgentRunEvent::TurnStarted { turn_index } => TuiRuntimeEvent::TurnStarted { turn_index },
        AgentRunEvent::AssistantStarted { label } => TuiRuntimeEvent::AssistantStarted { label },
        AgentRunEvent::AssistantDelta { delta } => TuiRuntimeEvent::AssistantDelta { delta },
        AgentRunEvent::AssistantFinished { .. } => TuiRuntimeEvent::AssistantFinished,
        AgentRunEvent::Thinking { text } => TuiRuntimeEvent::Thinking { text },
        AgentRunEvent::ToolStarted {
            name,
            parameters,
            risk_class: _,
        } => TuiRuntimeEvent::ToolStarted { name, parameters },
        AgentRunEvent::ToolFinished { name, result } => TuiRuntimeEvent::ToolFinished {
            name,
            success: result.success,
            summary: result.summary,
            output: result.output,
        },
        AgentRunEvent::FileDiff(payload) => TuiRuntimeEvent::FileDiff(payload),
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
        AgentRunEvent::AutoFixAttempt {
            attempt,
            max,
            tool_name,
            passed,
        } => TuiRuntimeEvent::AutoFixAttempt {
            attempt,
            max,
            tool_name,
            passed,
        },
        AgentRunEvent::Recovery { message } => TuiRuntimeEvent::Recovery { message },
        AgentRunEvent::PhaseChanged { from, to, reason } => TuiRuntimeEvent::PhaseChanged {
            from: format!("{from:?}").to_ascii_lowercase(),
            to: format!("{to:?}").to_ascii_lowercase(),
            reason,
        },
        AgentRunEvent::ContextCompacted { compacted } => TuiRuntimeEvent::ContextCompacted {
            narrative: compacted.narrative,
            files_read_count: compacted.files_read.len(),
            untrusted_count: compacted.untrusted_inputs.len(),
        },
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
            context_tokens,
            message_tokens,
            system_tokens,
            tool_schema_tokens,
            overhead_tokens,
        } => TuiRuntimeEvent::ContextUtilizationChanged {
            tokens_used,
            threshold,
            context_tokens,
            message_tokens,
            system_tokens,
            tool_schema_tokens,
            overhead_tokens,
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

/// Hermes-style 7-day idle Curator entry point.
///
/// Returns immediately when any of the following hold so peridot stays
/// snappy on every invocation:
/// - `memory.auto_skills = false` (opt-out)
/// - no session activity has been recorded yet
/// - the last activity is fresher than 7 days
/// - the Curator already ran in the last 7 days
///
/// When the 7-day idle threshold trips, `last_curator_run_unix` is
/// stamped immediately so a follow-up fast-exit invocation doesn't
/// race the gate, the cheap 30/90-day rules run inline, and the LLM
/// reflection pass is spawned onto the tokio runtime as a detached
/// task. Long-running commands (TUI, agent loops) keep the runtime
/// alive long enough for the LLM pass to finish; fast-exit commands
/// (`peridot version`) may drop the task mid-flight, which is the
/// acceptable cost of not delaying every CLI run by a network call.
async fn maybe_run_idle_curator(config: &PeridotConfig, project_root: &Path) {
    const SEVEN_DAYS: u64 = 7 * 24 * 3600;
    const TWO_HOURS: u64 = 2 * 3600;
    if !config.memory.auto_skills {
        return;
    }
    let store = MemoryStore::new(project_root.join(".peridot/memory.db"));
    let Ok(last_activity) = store.last_activity_unix() else {
        return;
    };
    if last_activity == 0 {
        return;
    }
    let last_curator: u64 = store
        .get_meta("last_curator_run_unix")
        .ok()
        .flatten()
        .and_then(|raw| raw.parse().ok())
        .unwrap_or(0);
    let last_session_end: u64 = store
        .get_meta("last_session_end_unix")
        .ok()
        .flatten()
        .and_then(|raw| raw.parse().ok())
        .unwrap_or(0);
    let now = run_state::unix_timestamp();
    if now.saturating_sub(last_activity) < SEVEN_DAYS {
        return;
    }
    if now.saturating_sub(last_curator) < SEVEN_DAYS {
        return;
    }
    // Hermes-aligned: require 2+ hours since the last session ended to
    // avoid firing the Curator the instant a user reopens a project
    // after a quiet week.
    let idle_since = last_session_end.max(last_activity);
    if now.saturating_sub(idle_since) < TWO_HOURS {
        return;
    }

    // Stamp the timestamp before doing anything else so a fast-exit
    // command that races with the spawned task doesn't trip the same
    // gate again on its next run.
    let _ = store.set_meta("last_curator_run_unix", &now.to_string());

    eprintln!("[curator] 7+ days idle — applying 30/90-day rules + spawning LLM pass");
    if let Ok(decisions) = store.apply_auto_rules(now, false) {
        for (name, verdict) in &decisions {
            if matches!(verdict, peridot_memory::AutoRuleVerdict::Archive) {
                let _ = move_auto_skill_to_archive(project_root, name);
            }
        }
    }

    let curator_model = config
        .memory
        .curator_model
        .clone()
        .unwrap_or_else(|| config.models.main.clone());
    let project_root = project_root.to_path_buf();
    let config = config.clone();
    tokio::spawn(async move {
        let store = MemoryStore::new(project_root.join(".peridot/memory.db"));
        let provider = match providers::live_provider(&config, &curator_model, &project_root).await
        {
            Ok(provider) => provider,
            Err(err) => {
                eprintln!("[curator] background provider unavailable: {err}");
                return;
            }
        };
        // Pass 1: Curator (single-session skill review + 30/90 day
        // archive). Always runs on the 7-day trigger.
        match curator::run_llm_curator(
            provider.as_ref(),
            &curator_model,
            &store,
            &project_root,
            now,
        )
        .await
        {
            Ok(report) => eprintln!(
                "[curator] background pass done: evaluated {}, applied {}, ignored {}",
                report.evaluated.len(),
                report.applied.len(),
                report.ignored.len(),
            ),
            Err(err) => eprintln!("[curator] background LLM failed: {err}"),
        }
        // Pass 2: Reflection (cross-session n-gram pattern promotion).
        // Gated by memory.auto_skill_reflection (default true in 0.7.3+).
        if config.memory.auto_skill_reflection {
            match curator::run_ngram_reflection(
                provider.as_ref(),
                &curator_model,
                &store,
                &project_root,
                config.memory.ngram_min_count,
                config.memory.ngram_batch_cap,
                now,
                config.memory.skills_review,
            )
            .await
            {
                Ok(report) => eprintln!(
                    "[reflection] background pass done: promoted {}, skipped {}",
                    report.promoted.len(),
                    report.skipped.len(),
                ),
                Err(err) => eprintln!("[reflection] background pass failed: {err}"),
            }
        }
        // Pass 3: Harness self-tuning. Watches the operator's recent
        // tool usage and flips config defaults (git.auto_commit /
        // git.auto_branch) when a clear behavioural signal emerges.
        // Each field is auto-adjusted at most once across the
        // project's lifetime — once the harness has spoken, the
        // operator owns it.
        let config_path = project_root.join(".peridot/config.toml");
        if config_path.exists() {
            let report = harness_learn::run_pass(&store, &config, &config_path, &project_root, now);
            if !report.applied.is_empty() {
                for adjustment in &report.applied {
                    eprintln!(
                        "[harness] auto-adjusted {} → {} ({})",
                        adjustment.field, adjustment.new_value, adjustment.signal
                    );
                }
            }
            if !report.skipped.is_empty() {
                eprintln!("[harness] skipped {} adjustment(s)", report.skipped.len());
            }
        }
    });
}
