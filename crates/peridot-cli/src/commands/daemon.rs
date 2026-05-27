//! `peridot daemon` -- JSON-RPC over stdio server.
//!
//! Speaks line-delimited JSON-RPC 2.0 (`\n` framed) so VS Code and other
//! editor clients can drive Peridot bidirectionally. Responses and
//! notifications are serialized onto a single stdout writer task so concurrent
//! session tasks cannot interleave JSON frames.

use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::io::BufRead;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex as StdMutex};

use anyhow::Result;
use async_trait::async_trait;
use peridot_common::{
    AskUserAnswer, AskUserRequest, CancelToken, ExecutionMode, McpTransport, PeridotConfig,
    PermissionMode, ReasoningEffort, ToolCall,
};
use peridot_context::{BranchJournal, ContextEntry, ContextSource, estimate_tokens_for_text};
use peridot_core::{
    AgentRunEvent, AutoFixAction, ExportArtifact, GoalStatus, PlanStepUpdate, SlashCommand,
    SlashStateDelta, StopReason, SubagentModelChange, parse_slash_command, slash_state_delta,
};
use peridot_git::GitManager;
use peridot_memory::{MemoryStore, SessionLifecycle, SessionRecord, SessionSummary};
use peridot_tools::AskUserPort;
use peridot_tui::SettingItem;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::io::AsyncWriteExt;
use tokio::sync::{Mutex, mpsc, oneshot};

use crate::checkpoints::restore_latest_checkpoint;
use crate::commands::{
    AuthProvider, append_session_note, move_auto_skill_to_archive, read_managed_env_var,
    read_session_notes, read_stored_api_key, read_stored_openai_oauth_credentials,
    restore_archived_skill, session_count_summary,
};
use crate::run_loop::{AgentTaskOptions, MessageBusHookup, run_task_with_events};
use crate::session_router::{RouterMessageBus, SessionHandle, SessionRouter, WorkspaceIsolation};
use crate::worktree_cleanup::reconcile_stale_worktrees;

/// Shared daemon state cloned into per-session tasks.
#[derive(Clone)]
struct DaemonState {
    sessions: Arc<Mutex<HashMap<String, SessionEntry>>>,
    next_session_id: Arc<Mutex<u64>>,
    next_interaction_id: Arc<std::sync::atomic::AtomicU64>,
    ask_user_pending: Arc<std::sync::Mutex<HashMap<String, oneshot::Sender<AskUserAnswer>>>>,
    router: Arc<StdMutex<SessionRouter>>,
    project_root: Arc<PathBuf>,
    out: mpsc::UnboundedSender<String>,
    run_config: Arc<PeridotConfig>,
    run_template: Arc<AgentTaskOptions>,
    session_list_subscribed: Arc<AtomicBool>,
}

impl DaemonState {
    fn new(
        project_root: PathBuf,
        run_config: PeridotConfig,
        run_template: AgentTaskOptions,
        out: mpsc::UnboundedSender<String>,
    ) -> Self {
        Self {
            sessions: Arc::new(Mutex::new(HashMap::new())),
            next_session_id: Arc::new(Mutex::new(1)),
            next_interaction_id: Arc::new(std::sync::atomic::AtomicU64::new(1)),
            ask_user_pending: Arc::new(std::sync::Mutex::new(HashMap::new())),
            router: Arc::new(StdMutex::new(SessionRouter::new())),
            project_root: Arc::new(project_root),
            out,
            run_config: Arc::new(run_config),
            run_template: Arc::new(run_template),
            session_list_subscribed: Arc::new(AtomicBool::new(false)),
        }
    }

    async fn next_id(&self) -> String {
        let mut next = self.next_session_id.lock().await;
        let id = *next;
        *next += 1;
        format!("session-{}-{id}", std::process::id())
    }
}

struct SessionEntry {
    cancel: CancelToken,
    compact_request: Arc<AtomicBool>,
    task: Option<tokio::task::JoinHandle<()>>,
    spec: SessionRunSpec,
    usage: Arc<StdMutex<LiveSessionUsage>>,
    plan: Arc<StdMutex<LiveSessionPlan>>,
    goal: Arc<StdMutex<LiveSessionGoal>>,
    approval_grants: Vec<ApprovalGrant>,
    waiting_approval: Option<ApprovalRequestSnapshot>,
}

#[derive(Clone, Debug, Default)]
struct LiveSessionUsage {
    total_tokens: u64,
    cost_usd: f64,
    turns_used: u32,
    cost_limit: Option<f64>,
    turns_limit: Option<u32>,
    committee_planner_tokens: u64,
    committee_planner_cost_usd: f64,
    committee_reviewer_tokens: u64,
    committee_reviewer_cost_usd: f64,
}

impl LiveSessionUsage {
    fn record_event(&mut self, event: &AgentRunEvent) {
        match event {
            AgentRunEvent::UsageUpdated { usage } => {
                self.total_tokens = usage.input_tokens
                    + usage.output_tokens
                    + usage.cache_read_tokens
                    + usage.cache_creation_tokens
                    + usage.reasoning_output_tokens;
                self.cost_usd = usage.estimated_cost_usd;
            }
            AgentRunEvent::BudgetUpdated {
                cost_used,
                cost_limit,
                turns_used,
                turns_limit,
            } => {
                self.cost_usd = *cost_used;
                self.cost_limit = *cost_limit;
                self.turns_used = *turns_used;
                self.turns_limit = *turns_limit;
            }
            AgentRunEvent::CommitteeRoleUsage {
                role,
                cost_usd,
                tokens,
            } => match role.as_str() {
                "planner" => {
                    self.committee_planner_cost_usd += *cost_usd;
                    self.committee_planner_tokens += *tokens;
                }
                "reviewer" => {
                    self.committee_reviewer_cost_usd += *cost_usd;
                    self.committee_reviewer_tokens += *tokens;
                }
                _ => {}
            },
            _ => {}
        }
    }
}

#[derive(Clone, Debug, Default)]
struct LiveSessionPlan {
    steps: Vec<PlanStepUpdate>,
    current: Option<u32>,
}

#[derive(Clone, Debug, Default)]
struct LiveSessionGoal {
    objective: Option<String>,
    status: Option<GoalStatus>,
    started_at_unix: Option<u64>,
}

#[derive(Clone)]
struct SessionRunSpec {
    task: String,
    mode: ExecutionMode,
    permission: PermissionMode,
    model: Option<String>,
    reasoning_effort: Option<ReasoningEffort>,
    service_tier: Option<Option<String>>,
    /// Snapshot of `.peridot/config.toml` taken when this session was
    /// requested. Carrying it on the spec instead of reading the daemon
    /// state's boot snapshot is what makes `settings.save` apply to the
    /// next session: the fresh disk content arrives here and flows
    /// straight into `run_session_task`'s config clone.
    config: PeridotConfig,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct ApprovalRequestSnapshot {
    tool_name: String,
    reason: String,
    #[serde(default)]
    parameters: Value,
}

#[derive(Clone, Debug)]
struct ApprovalGrant {
    tool_name: String,
    reason: String,
    scope: ApprovalScope,
    call_key: String,
    command: Option<String>,
    path: Option<String>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ApprovalScope {
    Once,
    Session,
    Command,
    Path,
}

/// JSON-RPC 2.0 request envelope.
#[derive(Debug, Deserialize)]
struct RpcRequest {
    #[serde(default)]
    jsonrpc: String,
    #[serde(default)]
    id: Option<Value>,
    method: String,
    #[serde(default)]
    params: Option<Value>,
}

/// JSON-RPC 2.0 success response.
#[derive(Debug, Serialize)]
struct RpcResponse {
    jsonrpc: &'static str,
    id: Value,
    result: Value,
}

/// JSON-RPC 2.0 error response.
#[derive(Debug, Serialize)]
struct RpcErrorResponse {
    jsonrpc: &'static str,
    id: Value,
    error: RpcError,
}

#[derive(Debug, Serialize)]
struct RpcError {
    code: i32,
    message: String,
}

/// Public entry point invoked by `peridot daemon`.
pub(crate) async fn run_daemon_command(
    project_root: &Path,
    config: &PeridotConfig,
    template: AgentTaskOptions,
) -> Result<()> {
    let (out_tx, out_rx) = mpsc::unbounded_channel::<String>();
    let writer = tokio::spawn(writer_task(out_rx));
    let state = DaemonState::new(
        project_root.to_path_buf(),
        config.clone(),
        template,
        out_tx.clone(),
    );
    // Emit the handshake notification before reading any client traffic so an
    // editor can detect version skew on the very first daemon line, before it
    // sends `session.start` or any other request.
    emit_handshake(&state)?;

    let (line_tx, mut line_rx) = mpsc::unbounded_channel::<std::io::Result<String>>();
    let reader = tokio::task::spawn_blocking(move || {
        let stdin = std::io::stdin();
        let reader = std::io::BufReader::new(stdin.lock());
        for line in reader.lines() {
            if line_tx.send(line).is_err() {
                break;
            }
        }
    });

    while let Some(line) = line_rx.recv().await {
        let line = match line {
            Ok(line) => line,
            Err(err) => {
                emit_error(
                    &state,
                    Value::Null,
                    -32603,
                    format!("stdin read error: {err}"),
                )?;
                continue;
            }
        };
        if dispatch_line(&state, &line).await? {
            break;
        }
    }

    shutdown_sessions(&state).await;
    drop(state);
    drop(out_tx);
    let _ = writer.await;
    let _ = reader.await;
    Ok(())
}

async fn writer_task(mut rx: mpsc::UnboundedReceiver<String>) {
    let mut stdout = tokio::io::stdout();
    while let Some(line) = rx.recv().await {
        if stdout.write_all(line.as_bytes()).await.is_err() {
            break;
        }
        if stdout.write_all(b"\n").await.is_err() {
            break;
        }
        if stdout.flush().await.is_err() {
            break;
        }
    }
}

async fn shutdown_sessions(state: &DaemonState) {
    let mut sessions = state.sessions.lock().await;
    let now = crate::run_state::unix_timestamp();
    let store = MemoryStore::new(state.project_root.join(".peridot/memory.db"));
    for (session_id, entry) in sessions.drain() {
        let _ = store.update_session_lifecycle(&session_id, SessionLifecycle::Suspended, now);
        entry.cancel.cancel();
        if let Some(task) = entry.task {
            task.abort();
        }
    }
    *state
        .router
        .lock()
        .expect("daemon session router mutex poisoned") = SessionRouter::new();
}

async fn dispatch_line(state: &DaemonState, line: &str) -> Result<bool> {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return Ok(false);
    }
    let request: RpcRequest = match serde_json::from_str(trimmed) {
        Ok(request) => request,
        Err(err) => {
            emit_error(state, Value::Null, -32700, format!("parse error: {err}"))?;
            return Ok(false);
        }
    };
    dispatch_request(state, request).await
}

async fn dispatch_request(state: &DaemonState, request: RpcRequest) -> Result<bool> {
    if request.jsonrpc != "2.0" {
        emit_error(
            state,
            request.id.unwrap_or(Value::Null),
            -32600,
            format!("expected jsonrpc=2.0, got {}", request.jsonrpc),
        )?;
        return Ok(false);
    }

    match request.method.as_str() {
        "peridot.version" => {
            emit_response(
                state,
                request.id.unwrap_or(Value::Null),
                serde_json::json!({ "version": env!("CARGO_PKG_VERSION") }),
            )?;
        }
        "peridot.status" => {
            handle_status(state, request.id.unwrap_or(Value::Null)).await?;
        }
        "session.command_catalog" => {
            emit_response(
                state,
                request.id.unwrap_or(Value::Null),
                slash_command_catalog_result(command_catalog_surface(request.params.as_ref())),
            )?;
        }
        "skills.list" => {
            emit_response(
                state,
                request.id.unwrap_or(Value::Null),
                skills_list_result(state),
            )?;
        }
        "session.list" => {
            handle_session_list(state, request.id.unwrap_or(Value::Null)).await?;
        }
        "session.subscribe_list" => {
            state.session_list_subscribed.store(true, Ordering::Relaxed);
            handle_session_list(state, request.id.unwrap_or(Value::Null)).await?;
        }
        "peridot.echo" => match request.params {
            Some(Value::Object(map)) => {
                let echo = map.get("text").cloned().unwrap_or(Value::Null);
                emit_response(
                    state,
                    request.id.unwrap_or(Value::Null),
                    serde_json::json!({ "echo": echo }),
                )?;
            }
            _ => {
                emit_error(
                    state,
                    request.id.unwrap_or(Value::Null),
                    -32602,
                    "params must be an object with a `text` field".to_string(),
                )?;
            }
        },
        "session.start" => {
            handle_session_start(state, request.id.unwrap_or(Value::Null), request.params).await?;
        }
        "session.generate_title" => {
            handle_session_generate_title(state, request.id.unwrap_or(Value::Null), request.params)
                .await?;
        }
        "session.command" => {
            handle_session_command(state, request.id.unwrap_or(Value::Null), request.params)
                .await?;
        }
        "session.cancel" => {
            handle_session_cancel(state, request.id.unwrap_or(Value::Null), request.params).await?;
        }
        "settings.list" => {
            handle_settings_list(state, request.id.unwrap_or(Value::Null)).await?;
        }
        "settings.save" => {
            handle_settings_save(state, request.id.unwrap_or(Value::Null), request.params).await?;
        }
        "interaction.respond" => {
            handle_interaction_respond(state, request.id.unwrap_or(Value::Null), request.params)
                .await?;
        }
        "approval.respond" => {
            handle_approval_respond(state, request.id.unwrap_or(Value::Null), request.params)
                .await?;
        }
        "shutdown" => {
            if let Some(id) = request.id {
                emit_response(state, id, serde_json::json!({ "shutdown": true }))?;
            }
            return Ok(true);
        }
        other => {
            emit_error(
                state,
                request.id.unwrap_or(Value::Null),
                -32601,
                format!("method not found: {other}"),
            )?;
        }
    }
    Ok(false)
}

fn command_catalog_surface(params: Option<&Value>) -> Option<&str> {
    params
        .and_then(Value::as_object)
        .and_then(|object| object.get("surface"))
        .and_then(Value::as_str)
        .filter(|surface| !surface.trim().is_empty())
}

fn slash_command_catalog_result(surface: Option<&str>) -> Value {
    let commands: Vec<Value> = peridot_tui::slash_command_catalog()
        .iter()
        .filter(|spec| {
            surface
                .is_none_or(|surface| peridot_tui::slash_command_surfaces(spec).contains(&surface))
        })
        .map(|spec| {
            serde_json::json!({
                "name": spec.name,
                "description": spec.description,
                "arg_hint": spec.arg_hint,
                "category": spec.category,
                "surfaces": peridot_tui::slash_command_surfaces(spec),
                "arg_options": peridot_tui::slash_command_arg_options(spec),
            })
        })
        .collect();
    serde_json::json!({ "commands": commands })
}

fn handle_command_help(raw_command: &str, surface: Option<&str>) -> Value {
    let items = slash_help_items(surface);
    let total = items.len();
    let mut result = serde_json::json!({
        "kind": "help",
        "title": "Slash Commands",
        "message": format!("{total} slash command(s) available"),
        "severity": "info",
        "command": raw_command,
        "items": items,
        "total": total,
    });
    if let Some(surface) = surface {
        result["surface"] = Value::String(surface.to_string());
    }
    result
}

fn slash_help_items(surface: Option<&str>) -> Vec<Value> {
    peridot_tui::slash_command_catalog()
        .iter()
        .filter(|spec| {
            surface
                .is_none_or(|surface| peridot_tui::slash_command_surfaces(spec).contains(&surface))
        })
        .map(|spec| {
            let label = match spec.arg_hint {
                Some(hint) => format!("{} {}", spec.name, hint),
                None => spec.name.to_string(),
            };
            serde_json::json!({
                "label": label,
                "detail": spec.description,
                "source": spec.category,
            })
        })
        .collect()
}

fn skills_list_result(state: &DaemonState) -> Value {
    let store = peridot_memory::MemoryStore::new(state.project_root.join(".peridot/memory.db"));
    let skills = store.list_skills().unwrap_or_default();
    let skills: Vec<Value> = skills
        .into_iter()
        .filter(|skill| skill.scope == "auto")
        .map(|skill| {
            serde_json::json!({
                "name": skill.name,
                "description": skill_description(&skill),
                "scope": skill.scope,
            })
        })
        .collect();
    serde_json::json!({ "skills": skills })
}

fn skill_description(skill: &peridot_memory::StoredSkill) -> String {
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

async fn handle_session_list(state: &DaemonState, id: Value) -> Result<()> {
    emit_response(state, id, session_list_result(state).await)
}

async fn session_list_result(state: &DaemonState) -> Value {
    let store = MemoryStore::new(state.project_root.join(".peridot/memory.db"));
    let records = store.list_session_records().unwrap_or_default();
    let legacy = store.list_sessions().unwrap_or_default();
    let running_sessions = state.sessions.lock().await;
    let mut rows = BTreeMap::<String, Value>::new();
    for session in legacy {
        rows.insert(
            session.id.clone(),
            serde_json::json!({
                "id": session.id,
                "title": session.summary,
                "summary": session.summary,
                "status": "idle",
                "running": false,
                "updated_at_unix": 0,
            }),
        );
    }
    for record in records {
        let running =
            running_sessions.contains_key(&record.id) || record.status == SessionLifecycle::Running;
        rows.insert(
            record.id.clone(),
            serde_json::json!({
                "id": record.id,
                "title": record_title(&record),
                "summary": record.summary,
                "status": format!("{:?}", record.status).to_ascii_lowercase(),
                "running": running,
                "updated_at_unix": record.updated_at_unix,
                "last_task": record.last_task,
                "total_tokens": record.total_tokens,
                "total_cost_usd": record.total_cost_usd,
                "turns_used": record.turns_used,
            }),
        );
    }
    for (id, entry) in running_sessions.iter() {
        rows.entry(id.clone()).or_insert_with(|| {
            serde_json::json!({
                "id": id,
                "title": session_title_from_task(&entry.spec.task),
                "summary": compact_daemon_summary(&entry.spec.task),
                "status": "running",
                "running": true,
                "updated_at_unix": crate::run_state::unix_timestamp(),
                "last_task": entry.spec.task,
                "total_tokens": 0,
                "total_cost_usd": 0.0,
                "turns_used": 0,
            })
        });
    }
    serde_json::json!({
        "sessions": rows.into_values().collect::<Vec<_>>(),
    })
}

fn record_title(record: &SessionRecord) -> String {
    record
        .last_task
        .as_deref()
        .map(session_title_from_task)
        .filter(|title| !title.is_empty())
        .unwrap_or_else(|| record.summary.clone())
}

fn session_title_from_task(task: &str) -> String {
    let trimmed = task.trim();
    if trimmed.is_empty() {
        return "Untitled session".to_string();
    }
    compact_chars(trimmed, 80)
}

fn compact_daemon_summary(task: &str) -> String {
    format!("task=\"{}\" running", compact_chars(task.trim(), 160))
}

fn compact_chars(value: &str, max_chars: usize) -> String {
    if value.chars().count() <= max_chars {
        return value.to_string();
    }
    value
        .chars()
        .take(max_chars.saturating_sub(1))
        .chain(std::iter::once('…'))
        .collect()
}

async fn handle_status(state: &DaemonState, id: Value) -> Result<()> {
    let config = state.run_config.as_ref();
    let auth = auth_status(config).await;
    let worktree_cleanup = reconcile_stale_worktrees(state.project_root.as_ref());
    emit_response(
        state,
        id,
        serde_json::json!({
            "version": env!("CARGO_PKG_VERSION"),
            "project_root": state.project_root.as_ref(),
            "provider": config.auth.primary,
            "model": config.models.main,
            "reasoning_effort": format!("{:?}", config.models.reasoning_effort),
            "mode": format!("{:?}", config.defaults.mode),
            "permission": format!("{:?}", state.run_template.permission),
            "auth": auth,
            "worktree_cleanup": worktree_cleanup,
        }),
    )
}

async fn auth_status(config: &PeridotConfig) -> Value {
    let provider = config.auth.primary.as_str();
    match provider {
        "claude-api" => api_key_status("ANTHROPIC_API_KEY", AuthProvider::ClaudeApi),
        "openai-api" => api_key_status("OPENAI_API_KEY", AuthProvider::OpenaiApi),
        "openrouter-api" => {
            let configured = std::env::var("OPENROUTER_API_KEY").ok().is_some()
                || read_managed_env_var("OPENROUTER_API_KEY")
                    .ok()
                    .flatten()
                    .is_some();
            serde_json::json!({
                "provider": provider,
                "configured": configured,
                "method": "api_key",
                "source": if configured { "env_or_peridot_env" } else { "missing" },
            })
        }
        "openai-oauth" => {
            let env_configured = std::env::var("OPENAI_ACCESS_TOKEN").ok().is_some();
            let stored = read_stored_openai_oauth_credentials().await.ok().flatten();
            let account_configured = std::env::var("OPENAI_CODEX_ACCOUNT_ID").ok().is_some()
                || stored
                    .as_ref()
                    .and_then(|credentials| credentials.account_id.as_deref())
                    .is_some();
            serde_json::json!({
                "provider": provider,
                "configured": env_configured || stored.is_some(),
                "account_configured": account_configured,
                "method": "oauth",
                "source": if env_configured { "env" } else if stored.is_some() { "stored" } else { "missing" },
            })
        }
        _ => serde_json::json!({
            "provider": provider,
            "configured": false,
            "method": "unknown",
            "source": "unknown_provider",
        }),
    }
}

fn api_key_status(env_var: &str, provider: AuthProvider) -> Value {
    let env_configured = std::env::var(env_var).ok().is_some();
    let stored_configured = read_stored_api_key(provider).ok().flatten().is_some();
    serde_json::json!({
        "provider": provider.id(),
        "configured": env_configured || stored_configured,
        "method": "api_key",
        "source": if env_configured { "env" } else if stored_configured { "stored" } else { "missing" },
    })
}

async fn handle_session_start(state: &DaemonState, id: Value, params: Option<Value>) -> Result<()> {
    let Some(Value::Object(params)) = params else {
        emit_error(
            state,
            id,
            -32602,
            "params must be an object with a `task` field".to_string(),
        )?;
        return Ok(());
    };
    let Some(task) = params
        .get("task")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|task| !task.is_empty())
        .map(str::to_string)
    else {
        emit_error(
            state,
            id,
            -32602,
            "params.task must be a non-empty string".to_string(),
        )?;
        return Ok(());
    };

    // Pull a fresh PeridotConfig off disk so changes saved via
    // `settings.save` apply from the next session start. Falls back to
    // the boot snapshot when the file is missing or unparseable —
    // blocking session start on a config glitch would be worse than
    // running with the last known-good snapshot.
    let fresh_config =
        reload_run_config_from_disk(state).unwrap_or_else(|| (*state.run_config).clone());

    let mode = match optional_str(&params, "mode") {
        Some(value) => match parse_execution_mode(value) {
            Some(mode) => mode,
            None => {
                emit_error(
                    state,
                    id,
                    -32602,
                    "params.mode must be one of plan, execute, or goal".to_string(),
                )?;
                return Ok(());
            }
        },
        None => fresh_config.defaults.mode,
    };
    let permission = match optional_str(&params, "permission") {
        Some(value) => match parse_permission_mode(value) {
            Some(permission) => permission,
            None => {
                emit_error(
                    state,
                    id,
                    -32602,
                    "params.permission must be one of safe, auto, or yolo".to_string(),
                )?;
                return Ok(());
            }
        },
        None => state.run_template.permission,
    };
    let model = optional_str(&params, "model").map(str::to_string);
    let reasoning_effort = match optional_str(&params, "reasoning_effort") {
        Some(value) => match ReasoningEffort::parse(value) {
            Some(effort) => Some(effort),
            None => {
                emit_error(
                    state,
                    id,
                    -32602,
                    "params.reasoning_effort must be one of off, low, medium, high, or xhigh"
                        .to_string(),
                )?;
                return Ok(());
            }
        },
        None => None,
    };
    let service_tier = match optional_str(&params, "service_tier") {
        Some(value) => match parse_service_tier(value) {
            Some(tier) => Some(tier),
            None => {
                emit_error(
                    state,
                    id,
                    -32602,
                    "params.service_tier must be fast, priority, standard, default, or off"
                        .to_string(),
                )?;
                return Ok(());
            }
        },
        None => None,
    };

    let requested_session_id = optional_str(&params, "session_id")
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string);
    if let Some(requested) = requested_session_id.as_ref()
        && state.sessions.lock().await.contains_key(requested)
    {
        emit_error(
            state,
            id,
            -32602,
            format!("session_id is already running: {requested}"),
        )?;
        return Ok(());
    }

    let session_id = match requested_session_id {
        Some(session_id) => session_id,
        None => state.next_id().await,
    };
    let cancel = CancelToken::new();
    let cancel_for_task = cancel.clone();
    let compact_request = Arc::new(AtomicBool::new(false));
    let compact_request_for_task = compact_request.clone();
    let state_for_task = state.clone();
    let session_id_for_task = session_id.clone();
    let spec = SessionRunSpec {
        task,
        mode,
        permission,
        model,
        reasoning_effort,
        service_tier,
        config: fresh_config,
    };
    let spec_for_task = spec.clone();
    let usage = Arc::new(StdMutex::new(LiveSessionUsage::default()));
    let usage_for_task = usage.clone();
    let plan = Arc::new(StdMutex::new(LiveSessionPlan::default()));
    let plan_for_task = plan.clone();
    let goal = Arc::new(StdMutex::new(initial_live_goal(&spec)));
    let (start_tx, start_rx) = oneshot::channel::<()>();

    let handle = tokio::spawn(async move {
        let _ = start_rx.await;
        run_session_task(
            state_for_task,
            session_id_for_task,
            spec_for_task,
            cancel_for_task,
            compact_request_for_task,
            usage_for_task,
            plan_for_task,
        )
        .await;
    });

    state.sessions.lock().await.insert(
        session_id.clone(),
        SessionEntry {
            cancel,
            compact_request,
            task: Some(handle),
            spec: spec.clone(),
            usage,
            plan,
            goal,
            approval_grants: Vec::new(),
            waiting_approval: None,
        },
    );
    state
        .router
        .lock()
        .expect("daemon session router mutex poisoned")
        .register(SessionHandle::new(
            session_id.clone(),
            state.project_root.as_ref().clone(),
            WorkspaceIsolation::Shared,
        ));
    save_daemon_session_record(state, &session_id, &spec, SessionLifecycle::Running, None).await;
    emit_response(state, id, serde_json::json!({ "session_id": session_id }))?;
    emit_session_list_changed(state).await;
    let _ = start_tx.send(());
    Ok(())
}

/// JSON-RPC: `session.generate_title`
///
/// Generate a short LLM-authored title for a coding session from the user's
/// first task text. Wraps `crate::generate_session_title`, which uses the
/// configured main model with `reasoning_effort: Off, thinking: false` — so
/// this is a cheap one-shot completion, no tools.
///
/// Returns `{ "title": <string> | null }`. A `null` result means the provider
/// failed or returned an empty string; the caller (VS Code sidebar) is
/// expected to surface a `"No title"` fallback rather than silently using
/// the raw task text.
async fn handle_session_generate_title(
    state: &DaemonState,
    id: Value,
    params: Option<Value>,
) -> Result<()> {
    let map = match params {
        Some(Value::Object(map)) => map,
        _ => {
            return emit_error(
                state,
                id,
                -32602,
                "params must be an object with a `task` field".to_string(),
            );
        }
    };
    let task = match map.get("task").and_then(|v| v.as_str()) {
        Some(t) if !t.trim().is_empty() => t.to_string(),
        _ => {
            return emit_error(
                state,
                id,
                -32602,
                "`task` must be a non-empty string".to_string(),
            );
        }
    };
    let config = state.run_config.as_ref().clone();
    let project_root = state.project_root.as_ref().clone();
    // Reach across into the binary crate root for the shared helper.
    let title = crate::generate_session_title(&config, &project_root, &task).await;
    emit_response(state, id, serde_json::json!({ "title": title }))
}

/// Re-read `.peridot/config.toml` so the next session start picks up
/// any changes saved via `settings.save`. Returns `None` when the file
/// is missing or unparseable so the caller can transparently fall back
/// to the daemon's boot snapshot — a broken disk config must not block
/// the user from starting a session.
///
/// Cheap (<1ms for the ~1KB file) so re-running it per session start
/// is acceptable without caching or mtime tracking.
fn reload_run_config_from_disk(state: &DaemonState) -> Option<PeridotConfig> {
    let path = state.project_root.join(".peridot").join("config.toml");
    let raw = fs::read_to_string(&path).ok()?;
    toml::from_str(&raw).ok()
}

async fn save_daemon_session_record(
    state: &DaemonState,
    session_id: &str,
    spec: &SessionRunSpec,
    status: SessionLifecycle,
    summary: Option<&peridot_core::AgentRunSummary>,
) {
    let store = MemoryStore::new(state.project_root.join(".peridot/memory.db"));
    let now = crate::run_state::unix_timestamp();
    let existing = store.get_session_record(session_id).ok().flatten();
    let created_at = existing
        .as_ref()
        .map(|record| record.created_at_unix)
        .filter(|value| *value > 0)
        .unwrap_or(now);
    let mut record = existing
        .unwrap_or_else(|| SessionRecord::new(session_id, state.project_root.as_ref().clone()));
    record.summary = match summary {
        Some(summary) => format!(
            "task=\"{}\" stopped={:?} turns={} cost=${:.6}",
            compact_chars(&spec.task, 160),
            summary.stopped_reason,
            summary.turns.len(),
            summary.usage.estimated_cost_usd
        ),
        None => compact_daemon_summary(&spec.task),
    };
    record.status = status;
    record.created_at_unix = created_at;
    record.updated_at_unix = now;
    record.workspace_root = state.project_root.as_ref().clone();
    record.last_task = Some(spec.task.clone());
    if let Some(summary) = summary {
        record.total_tokens = summary.usage.input_tokens
            + summary.usage.output_tokens
            + summary.usage.cache_read_tokens
            + summary.usage.cache_creation_tokens
            + summary.usage.reasoning_output_tokens;
        record.total_cost_usd = summary.usage.estimated_cost_usd;
        record.turns_used = summary.turns.len() as u32;
    } else if let Some(usage) = live_session_usage_snapshot(state, session_id).await {
        record.total_tokens = record.total_tokens.max(usage.total_tokens);
        record.total_cost_usd = record.total_cost_usd.max(usage.cost_usd);
        record.turns_used = record.turns_used.max(usage.turns_used);
    }
    let _ = store.save_session_record(&record);
}

async fn live_session_usage_snapshot(
    state: &DaemonState,
    session_id: &str,
) -> Option<LiveSessionUsage> {
    state
        .sessions
        .lock()
        .await
        .get(session_id)
        .map(|entry| entry.usage.lock().unwrap().clone())
}

fn initial_live_goal(spec: &SessionRunSpec) -> LiveSessionGoal {
    if spec.mode == ExecutionMode::Goal {
        LiveSessionGoal {
            objective: Some(spec.task.clone()),
            status: Some(GoalStatus::Running),
            started_at_unix: Some(crate::run_state::unix_timestamp()),
        }
    } else {
        LiveSessionGoal::default()
    }
}

async fn update_daemon_session_lifecycle(
    state: &DaemonState,
    session_id: &str,
    status: SessionLifecycle,
) {
    let store = MemoryStore::new(state.project_root.join(".peridot/memory.db"));
    let _ = store.update_session_lifecycle(session_id, status, crate::run_state::unix_timestamp());
}

async fn emit_session_list_changed(state: &DaemonState) {
    if !state.session_list_subscribed.load(Ordering::Relaxed) {
        return;
    }
    let _ = emit_notification(
        state,
        "session.list_changed",
        session_list_result(state).await,
    );
}

/// JSON-RPC: `settings.list`
///
/// Reads `.peridot/config.toml` (creating it if missing) and returns the
/// curated [`SettingItem`] list the TUI's settings screen exposes. The
/// VS Code webview renders the same items as form controls so the two
/// surfaces stay in lock-step — every field added to `settings_registry`
/// flows through both UIs automatically.
///
/// Response: `{ "config_path": <abs path>, "items": [SettingItem, ...] }`.
async fn handle_settings_list(state: &DaemonState, id: Value) -> Result<()> {
    let project_root = state.project_root.as_ref().clone();
    let result = match super::config::init_project_config_value(&project_root) {
        Ok(r) => r,
        Err(err) => {
            return emit_error(
                state,
                id,
                -32000,
                format!("failed to prepare project config: {err}"),
            );
        }
    };
    let raw = match fs::read_to_string(&result.config_path) {
        Ok(s) => s,
        Err(err) => {
            return emit_error(
                state,
                id,
                -32000,
                format!("failed to read {}: {err}", result.config_path.display()),
            );
        }
    };
    let config: PeridotConfig = match toml::from_str(&raw) {
        Ok(c) => c,
        Err(err) => {
            return emit_error(
                state,
                id,
                -32000,
                format!("failed to parse {}: {err}", result.config_path.display()),
            );
        }
    };
    let items = super::settings::settings_registry(&config);
    emit_response(
        state,
        id,
        serde_json::json!({
            "config_path": result.config_path,
            "items": items,
        }),
    )
}

/// JSON-RPC: `settings.save`
///
/// Takes a mutated [`SettingItem`] list (same shape `settings.list`
/// returned), folds each value back into the on-disk config, and writes
/// `.peridot/config.toml`. Unknown ids in the payload are silently
/// ignored, matching the TUI screen's forward-compatible behaviour.
///
/// New sessions started after this RPC succeeds will see the new values
/// — see `reload_run_config_from_disk` for the snapshot refresh.
/// Already-running sessions are not touched.
///
/// Params: `{ "items": [SettingItem, ...] }`.
/// Response: `{ "saved": true, "config_path": <abs path> }`.
async fn handle_settings_save(state: &DaemonState, id: Value, params: Option<Value>) -> Result<()> {
    let Some(Value::Object(mut params)) = params else {
        return emit_error(
            state,
            id,
            -32602,
            "params must be an object with an `items` array".to_string(),
        );
    };
    let items_json = match params.remove("items") {
        Some(v) => v,
        None => {
            return emit_error(
                state,
                id,
                -32602,
                "missing `items` array in params".to_string(),
            );
        }
    };
    let items: Vec<SettingItem> = match serde_json::from_value(items_json) {
        Ok(v) => v,
        Err(err) => {
            return emit_error(state, id, -32602, format!("invalid `items` payload: {err}"));
        }
    };
    let project_root = state.project_root.as_ref().clone();
    let result = match super::config::init_project_config_value(&project_root) {
        Ok(r) => r,
        Err(err) => {
            return emit_error(
                state,
                id,
                -32000,
                format!("failed to prepare project config: {err}"),
            );
        }
    };
    let raw = match fs::read_to_string(&result.config_path) {
        Ok(s) => s,
        Err(err) => {
            return emit_error(
                state,
                id,
                -32000,
                format!("failed to read {}: {err}", result.config_path.display()),
            );
        }
    };
    let mut config: PeridotConfig = match toml::from_str(&raw) {
        Ok(c) => c,
        Err(err) => {
            return emit_error(
                state,
                id,
                -32000,
                format!("failed to parse existing config: {err}"),
            );
        }
    };
    super::settings::apply_settings_to_config(&items, &mut config);
    let serialized = match toml::to_string_pretty(&config) {
        Ok(s) => s,
        Err(err) => {
            return emit_error(
                state,
                id,
                -32000,
                format!("failed to serialize updated config: {err}"),
            );
        }
    };
    if let Err(err) = fs::write(&result.config_path, serialized) {
        return emit_error(
            state,
            id,
            -32000,
            format!("failed to write {}: {err}", result.config_path.display()),
        );
    }
    emit_response(
        state,
        id,
        serde_json::json!({
            "saved": true,
            "config_path": result.config_path,
        }),
    )
}

async fn run_session_task(
    state: DaemonState,
    session_id: String,
    spec: SessionRunSpec,
    cancel: CancelToken,
    compact_request: Arc<AtomicBool>,
    usage: Arc<StdMutex<LiveSessionUsage>>,
    plan: Arc<StdMutex<LiveSessionPlan>>,
) {
    let mut options = (*state.run_template).clone();
    options.permission = spec.permission;
    if let Some(model) = spec.model.clone() {
        options.model = model;
    }
    if let Some(reasoning_effort) = spec.reasoning_effort {
        options.reasoning_effort = reasoning_effort;
    }
    if let Some(service_tier) = spec.service_tier.clone() {
        options.service_tier = service_tier;
    }
    // Pick up config-controlled run parameters from the fresh snapshot
    // captured by `handle_session_start`. Fields the operator can edit
    // via the VS Code settings webview (max_turns, budget) need to live
    // here so a save → new-session round-trip actually changes behaviour.
    // CLI-only fields (`resume`, `mock_response_file`, `live`) stay on
    // the boot template untouched.
    options.max_turns = spec.config.defaults.max_turns;
    options.budget_usd = spec.config.defaults.budget_usd;
    let mut config = spec.config.clone();
    apply_session_approval_grants(&state, &session_id, &mut config).await;

    let ask_user_port = Arc::new(DaemonAskUserPort {
        state: state.clone(),
        session_id: session_id.clone(),
    });

    let context_snapshot_path = Some(context_snapshot_path(&state, &session_id));
    let approval_snapshot: Arc<std::sync::Mutex<Option<ApprovalRequestSnapshot>>> =
        Arc::new(std::sync::Mutex::new(None));
    let approval_snapshot_for_events = approval_snapshot.clone();
    let usage_for_events = usage.clone();
    let plan_for_events = plan.clone();
    let session_id_inner = session_id.clone();
    let state_inner = state.clone();
    let result = run_task_with_events(
        spec.task.clone(),
        spec.mode,
        options,
        config,
        state.project_root.as_ref().clone(),
        Some(cancel),
        Some(compact_request),
        context_snapshot_path,
        Some(ask_user_port),
        daemon_message_bus(&state, &session_id),
        move |event: AgentRunEvent| {
            if matches!(
                &event,
                AgentRunEvent::Finished { summary }
                    if summary.stopped_reason == StopReason::ApprovalRequired
            ) {
                return;
            }
            if let AgentRunEvent::ApprovalRequested {
                tool_name,
                reason,
                parameters,
            } = &event
            {
                *approval_snapshot_for_events.lock().unwrap() = Some(ApprovalRequestSnapshot {
                    tool_name: tool_name.clone(),
                    reason: reason.clone(),
                    parameters: parameters.clone(),
                });
            }
            usage_for_events.lock().unwrap().record_event(&event);
            if let AgentRunEvent::PlanUpdated { steps, current } = &event {
                *plan_for_events.lock().unwrap() = LiveSessionPlan {
                    steps: steps.clone(),
                    current: *current,
                };
            }
            let value = serde_json::to_value(&event).unwrap_or(Value::Null);
            emit_event(&state_inner, &session_id_inner, value);
        },
    )
    .await;

    match result {
        Ok(summary) => {
            if summary.stopped_reason == StopReason::ApprovalRequired {
                let approval = approval_snapshot.lock().unwrap().clone();
                mark_session_waiting_approval(&state, &session_id, approval.clone()).await;
                emit_event(
                    &state,
                    &session_id,
                    serde_json::json!({
                        "kind": "approval_waiting",
                        "request": approval,
                    }),
                );
                return;
            }
            let lifecycle = lifecycle_from_stop_reason(summary.stopped_reason.clone());
            save_daemon_session_record(&state, &session_id, &spec, lifecycle, Some(&summary)).await;
            emit_event(
                &state,
                &session_id,
                serde_json::json!({
                    "kind": "finished",
                    "stopped_reason": format!("{:?}", summary.stopped_reason),
                    "turns": summary.turns.len(),
                    "duration_ms": summary.duration_ms,
                }),
            );
        }
        Err(err) => {
            let approval = approval_snapshot.lock().unwrap().clone();
            if approval.is_some() && is_approval_required_error(&err) {
                mark_session_waiting_approval(&state, &session_id, approval.clone()).await;
                emit_event(
                    &state,
                    &session_id,
                    serde_json::json!({
                        "kind": "approval_waiting",
                        "request": approval,
                    }),
                );
                return;
            }
            update_daemon_session_lifecycle(&state, &session_id, SessionLifecycle::Failed).await;
            emit_event(
                &state,
                &session_id,
                serde_json::json!({
                    "kind": "error",
                    "message": err.to_string(),
                }),
            );
        }
    }

    state.sessions.lock().await.remove(&session_id);
    state
        .router
        .lock()
        .expect("daemon session router mutex poisoned")
        .close(&session_id);
    clear_pending_ask_user_for_session(&state, &session_id);
    emit_session_list_changed(&state).await;
}

fn lifecycle_from_stop_reason(reason: StopReason) -> SessionLifecycle {
    match reason {
        StopReason::Done => SessionLifecycle::Done,
        StopReason::Interrupted => SessionLifecycle::Suspended,
        StopReason::ApprovalRequired => SessionLifecycle::Running,
        StopReason::MaxTurns | StopReason::Budget => SessionLifecycle::Failed,
    }
}

fn daemon_message_bus(state: &DaemonState, session_id: &str) -> MessageBusHookup {
    let bus = RouterMessageBus::new(state.router.clone()).with_current_session(session_id);
    Some((
        Arc::new(bus) as Arc<dyn peridot_tools::AgentMessageBus>,
        session_id.to_string(),
    ))
}

struct DaemonAskUserPort {
    state: DaemonState,
    session_id: String,
}

#[async_trait]
impl AskUserPort for DaemonAskUserPort {
    async fn ask(&self, request: AskUserRequest) -> AskUserAnswer {
        let next = self
            .state
            .next_interaction_id
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let request_id = format!("{}:ask-user:{next}", self.session_id);
        let (tx, rx) = oneshot::channel();
        {
            let mut pending = self.state.ask_user_pending.lock().unwrap();
            pending.insert(request_id.clone(), tx);
        }
        emit_event(
            &self.state,
            &self.session_id,
            serde_json::json!({
                "kind": "ask_user_requested",
                "request_id": request_id,
                "request": request,
            }),
        );
        rx.await.unwrap_or(AskUserAnswer::Cancelled)
    }
}

async fn handle_session_cancel(
    state: &DaemonState,
    id: Value,
    params: Option<Value>,
) -> Result<()> {
    let Some(Value::Object(params)) = params else {
        emit_error(
            state,
            id,
            -32602,
            "params must be an object with a `session_id` field".to_string(),
        )?;
        return Ok(());
    };
    let Some(session_id) = params.get("session_id").and_then(Value::as_str) else {
        emit_error(
            state,
            id,
            -32602,
            "params.session_id must be a string".to_string(),
        )?;
        return Ok(());
    };

    let cancelled = if let Some(entry) = state.sessions.lock().await.remove(session_id) {
        entry.cancel.cancel();
        if let Some(task) = entry.task {
            task.abort();
        }
        state
            .router
            .lock()
            .expect("daemon session router mutex poisoned")
            .close(session_id);
        clear_pending_ask_user_for_session(state, session_id);
        update_daemon_session_lifecycle(state, session_id, SessionLifecycle::Suspended).await;
        true
    } else {
        false
    };
    emit_response(
        state,
        id,
        serde_json::json!({
            "cancelled": cancelled,
            "session_id": session_id,
        }),
    )?;
    if cancelled {
        emit_session_list_changed(state).await;
    }
    Ok(())
}

async fn handle_session_command(
    state: &DaemonState,
    id: Value,
    params: Option<Value>,
) -> Result<()> {
    let Some(Value::Object(params)) = params else {
        emit_error(
            state,
            id,
            -32602,
            "params must be an object with `command` and optional `session_id` fields".to_string(),
        )?;
        return Ok(());
    };
    let Some(raw_command) = params.get("command").and_then(Value::as_str) else {
        emit_error(
            state,
            id,
            -32602,
            "params.command must be a string".to_string(),
        )?;
        return Ok(());
    };
    let command_text = if raw_command.trim_start().starts_with('/') {
        raw_command.trim().to_string()
    } else {
        format!("/{}", raw_command.trim())
    };
    let Some(command) = parse_slash_command(&command_text) else {
        emit_error(
            state,
            id,
            -32602,
            format!("invalid slash command: {command_text}"),
        )?;
        return Ok(());
    };
    let session_id = optional_str(&params, "session_id").map(str::to_string);
    let surface = optional_str(&params, "surface").map(str::to_string);

    let result = if matches!(command, SlashCommand::Help) {
        Ok(handle_command_help(&command_text, surface.as_deref()))
    } else {
        execute_session_command(state, session_id.as_deref(), &command_text, command).await
    };

    match result {
        Ok(result) => {
            if let Some(session_id) = session_id.as_deref() {
                emit_event(
                    state,
                    session_id,
                    serde_json::json!({
                        "kind": "command_result",
                        "result": result,
                    }),
                );
            }
            emit_response(state, id, result)?;
        }
        Err(message) => {
            if let Some(session_id) = session_id.as_deref() {
                emit_event(
                    state,
                    session_id,
                    serde_json::json!({
                        "kind": "command_result",
                        "result": command_result("error", "Command failed", &message, "error"),
                    }),
                );
            }
            emit_error(state, id, -32010, message)?;
        }
    }
    Ok(())
}

async fn execute_session_command(
    state: &DaemonState,
    session_id: Option<&str>,
    raw_command: &str,
    command: SlashCommand,
) -> Result<Value, String> {
    let current_service_tier = session_service_tier(state, session_id).await;
    let state_delta = slash_state_delta(&command, current_service_tier.as_deref());
    apply_session_state_delta(state, session_id, &state_delta).await;
    match command {
        SlashCommand::Plan => Ok(command_result_with_state_delta(
            "setting",
            "Mode",
            "mode: plan",
            "info",
            &state_delta,
        )),
        SlashCommand::Execute => Ok(command_result_with_state_delta(
            "setting",
            "Mode",
            "mode: execute",
            "info",
            &state_delta,
        )),
        SlashCommand::Safe => Ok(command_result_with_state_delta(
            "setting",
            "Permission",
            "permission: safe",
            "info",
            &state_delta,
        )),
        SlashCommand::Auto => Ok(command_result_with_state_delta(
            "setting",
            "Permission",
            "permission: auto",
            "info",
            &state_delta,
        )),
        SlashCommand::Yolo => Ok(command_result_with_state_delta(
            "setting",
            "Permission",
            "permission: yolo",
            "info",
            &state_delta,
        )),
        SlashCommand::Model(model) => Ok(command_result_with_state_delta(
            "setting",
            "Model",
            &format!("model: {model}"),
            "info",
            &state_delta,
        )),
        SlashCommand::Provider(provider) => Ok(command_result_with_state_delta(
            "setting",
            "Provider",
            &format!("provider: {provider}"),
            "info",
            &state_delta,
        )),
        SlashCommand::Reasoning(effort) => Ok(command_result_with_state_delta(
            "setting",
            "Reasoning",
            &format!("reasoning: {effort}"),
            "info",
            &state_delta,
        )),
        SlashCommand::Fast(_value) => {
            let tier = state_delta.service_tier.clone().unwrap_or(None);
            let enabled = tier.as_deref() == Some("fast");
            Ok(command_result_with_state_delta(
                "setting",
                "Service Tier",
                if enabled {
                    "service tier: fast"
                } else {
                    "service tier: standard"
                },
                "info",
                &state_delta,
            ))
        }
        SlashCommand::Committee(mode) => Ok(command_result_with_state_delta(
            "setting",
            "Committee",
            &format!("committee: {mode:?}"),
            "info",
            &state_delta,
        )),
        SlashCommand::SubagentModel(change) => {
            let message = match change {
                SubagentModelChange::Set(model) => format!("subagent model: {model}"),
                SubagentModelChange::Reset => "subagent model: reset".to_string(),
            };
            Ok(command_result_with_state_delta(
                "setting",
                "Subagent",
                &message,
                "info",
                &state_delta,
            ))
        }
        SlashCommand::AutoFix(action) => {
            let message = match action {
                AutoFixAction::On => "autofix: on".to_string(),
                AutoFixAction::Off => "autofix: off".to_string(),
                AutoFixAction::MaxAttempts(max) => format!("autofix: {max} attempt(s)"),
            };
            Ok(command_result("setting", "Auto-fix", &message, "info"))
        }
        SlashCommand::Note(note) => handle_command_note(state, session_id, raw_command, &note),
        SlashCommand::Notes(last) => handle_command_notes(state, session_id, raw_command, last),
        SlashCommand::Lang(locale) => Ok(command_result_with_state_delta(
            "setting",
            "Language",
            &format!("language: {locale:?}"),
            "info",
            &state_delta,
        )),
        SlashCommand::Help => Ok(handle_command_help(raw_command, None)),
        SlashCommand::SkillList => handle_command_skill_list(state, raw_command),
        SlashCommand::SkillShow(name) => handle_command_skill_show(state, raw_command, &name),
        SlashCommand::SkillSearch(query) => handle_command_skill_search(state, raw_command, &query),
        SlashCommand::SkillArchived(query) => {
            handle_command_skill_archived(state, raw_command, &query)
        }
        SlashCommand::SkillPin(name) => handle_command_skill_pin(state, raw_command, &name, true),
        SlashCommand::SkillUnpin(name) => {
            handle_command_skill_pin(state, raw_command, &name, false)
        }
        SlashCommand::SkillArchive(name) => handle_command_skill_archive(state, raw_command, &name),
        SlashCommand::SkillRestore(name) => handle_command_skill_restore(state, raw_command, &name),
        SlashCommand::Cost => handle_command_cost(state, session_id, raw_command).await,
        SlashCommand::Info => handle_command_info(state, session_id, raw_command).await,
        SlashCommand::PlanShow => handle_command_plan_show(state, session_id, raw_command).await,
        SlashCommand::SessionSave => {
            handle_command_session_save(state, session_id, raw_command).await
        }
        SlashCommand::GoalPause
        | SlashCommand::GoalResume
        | SlashCommand::GoalClear
        | SlashCommand::GoalStatus => {
            handle_command_goal_control(state, session_id, raw_command, &command).await
        }
        SlashCommand::Skill { name, args } => {
            // Try the project's skill store. If the name exists, inject
            // the body as a PlanReminder-style context entry so the
            // model picks it up on the next turn. If not, surface a
            // clear "skill not found" rather than the generic invalid-
            // command error — this is the difference between "typo'd
            // command" and "asked for a skill I don't have."
            let project_root = state.project_root.as_ref().clone();
            let store = peridot_memory::MemoryStore::new(project_root.join(".peridot/memory.db"));
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or_default();
            let active = store
                .list_skills()
                .map_err(|err| format!("failed to read skill store: {err}"))?;
            let Some(skill) = active.into_iter().find(|s| s.name == name) else {
                return Err(format!(
                    "skill not found: {name}. Run `peridot run \"…\"` once \
                     to build relevant auto-skills, or try `/help`."
                ));
            };
            // Best-effort: stamp `last_used_at_unix` so the Curator's
            // staleness pass treats this skill as recently active.
            let _ = store.mark_skill_viewed(&skill.name, now);
            let trimmed_args = args.trim();
            let args_note = if trimmed_args.is_empty() {
                String::new()
            } else {
                format!("\n\nOperator passed args: {trimmed_args}")
            };
            if let Some(session_id) = session_id {
                append_plan_reminder_to_context(
                    state,
                    session_id,
                    skill_plan_reminder(&skill, &args),
                )?;
            }
            Ok(serde_json::json!({
                "kind": "skill",
                "title": format!("Skill: {}", skill.name),
                "message": format!("Loaded skill `{}`{}", skill.name, args_note),
                "severity": "info",
                "command": raw_command,
                "skill": {
                    "name": skill.name,
                    "description": skill.description,
                    "body": skill.body,
                },
            }))
        }
        SlashCommand::Clear => handle_command_clear(state, session_id, raw_command).await,
        SlashCommand::SidepanelToggle | SlashCommand::Collapse | SlashCommand::SessionNew(_) => {
            Ok(with_state_delta(
                serde_json::json!({
                    "kind": "client_action",
                    "action": "local",
                    "title": "Handled by Extension",
                    "message": format!("{raw_command}: handled by the extension UI"),
                    "severity": "info",
                    "command": raw_command,
                }),
                &state_delta,
            ))
        }
        SlashCommand::GoalStart(goal) => Ok(with_state_delta(
            start_task_result("goal", goal),
            &state_delta,
        )),
        SlashCommand::Fork(task) => Ok(start_task_result("fork", task)),
        SlashCommand::Teammate(task) => {
            Ok(start_task_result("teammate", format!("/teammate {task}")))
        }
        SlashCommand::Worktree { branch, task } => Ok(start_task_result(
            "worktree",
            format!("/worktree {branch} {task}"),
        )),
        SlashCommand::ContextTop => handle_command_context_top(state, session_id, raw_command),
        SlashCommand::Compact => handle_command_compact(state, session_id).await,
        SlashCommand::Rewind => handle_command_rewind(state, session_id, raw_command),
        SlashCommand::Diff => handle_command_diff(state, raw_command),
        SlashCommand::Undo => handle_command_undo(state, raw_command),
        SlashCommand::Todos => handle_command_todos(state, raw_command),
        SlashCommand::CodeMap => handle_command_codemap(state, raw_command, false),
        SlashCommand::CodeMapStatus => handle_command_codemap_status(state, raw_command),
        SlashCommand::CodeMapRefresh => handle_command_codemap(state, raw_command, true),
        SlashCommand::CodeMapFind(query) => handle_command_codemap_find(state, raw_command, &query),
        SlashCommand::CodeMapLocate(query) => {
            handle_command_codemap_locate(state, raw_command, &query)
        }
        SlashCommand::CodeMapOutline(path) => {
            handle_command_codemap_outline(state, raw_command, &path)
        }
        SlashCommand::CodeMapRefs(query) => handle_command_codemap_refs(state, raw_command, &query),
        SlashCommand::Attach(path) => handle_command_attach(state, session_id, raw_command, &path),
        SlashCommand::Attachments => handle_command_attachments(state, session_id, raw_command),
        SlashCommand::Detach(path) => handle_command_detach(state, session_id, raw_command, &path),
        SlashCommand::Export(artifacts) => {
            handle_command_export(state, session_id, raw_command, &artifacts)
        }
        SlashCommand::SessionList => handle_command_session_list(state, raw_command).await,
        SlashCommand::SessionCount => handle_command_session_count(state, raw_command),
        SlashCommand::SessionDelete(target) => {
            handle_command_session_delete(state, raw_command, &target).await
        }
        SlashCommand::SessionSwitch(target) => {
            handle_command_session_switch(state, raw_command, &target).await
        }
        SlashCommand::SessionClose(target) => {
            handle_command_session_close(state, raw_command, &target).await
        }
        SlashCommand::SessionRename { target, title } => {
            handle_command_session_rename(state, raw_command, &target, &title).await
        }
        SlashCommand::McpList => handle_command_mcp_list(state, raw_command),
        SlashCommand::McpAdd {
            name,
            transport,
            target,
        } => handle_command_mcp_add(state, raw_command, &name, &transport, &target),
        SlashCommand::McpRemove(name) => handle_command_mcp_remove(state, raw_command, &name),
        SlashCommand::McpTest(name) => handle_command_mcp_test(state, raw_command, &name).await,
        SlashCommand::BranchSave(name) => {
            handle_command_branch_save(state, session_id, raw_command, &name)
        }
        SlashCommand::BranchRestore(name) => {
            handle_command_branch_restore(state, session_id, raw_command, &name)
        }
        SlashCommand::BranchList => handle_command_branch_list(state, raw_command),
        SlashCommand::BranchPicker => handle_command_branch_picker(state, session_id, raw_command),
        SlashCommand::BranchTurn(turn_id) => {
            handle_command_branch_turn(state, session_id, raw_command, turn_id)
        }
        SlashCommand::BranchTree => handle_command_branch_tree(state, session_id, raw_command),
        SlashCommand::BranchSwitch(index) => {
            handle_command_branch_switch(state, session_id, raw_command, index)
        }
    }
}

async fn session_service_tier(state: &DaemonState, session_id: Option<&str>) -> Option<String> {
    if let Some(session_id) = session_id
        && let Some(entry) = state.sessions.lock().await.get(session_id)
    {
        return match entry.spec.service_tier.as_ref() {
            Some(Some(tier)) => Some(tier.clone()),
            Some(None) => None,
            None => state.run_template.service_tier.clone(),
        };
    }
    state.run_template.service_tier.clone()
}

async fn update_session_spec<F>(state: &DaemonState, session_id: Option<&str>, update: F)
where
    F: FnOnce(&mut SessionRunSpec),
{
    let Some(session_id) = session_id else {
        return;
    };
    if let Some(entry) = state.sessions.lock().await.get_mut(session_id) {
        update(&mut entry.spec);
    }
}

async fn apply_session_state_delta(
    state: &DaemonState,
    session_id: Option<&str>,
    delta: &SlashStateDelta,
) {
    if delta.is_empty() {
        return;
    }
    update_session_spec(state, session_id, |spec| {
        if let Some(mode) = delta.mode {
            spec.mode = mode;
        }
        if let Some(permission) = delta.permission {
            spec.permission = permission;
        }
        if let Some(model) = delta.model.as_ref() {
            spec.model = Some(model.clone());
        }
        if let Some(provider) = delta.provider.as_ref() {
            spec.config.auth.primary = provider.clone();
        }
        if let Some(reasoning_effort) = delta.reasoning_effort {
            spec.reasoning_effort = Some(reasoning_effort);
        }
        if let Some(service_tier) = delta.service_tier.as_ref() {
            spec.service_tier = Some(service_tier.clone());
        }
    })
    .await;
}

fn command_result(kind: &str, title: &str, message: &str, severity: &str) -> Value {
    serde_json::json!({
        "kind": kind,
        "title": title,
        "message": message,
        "severity": severity,
    })
}

fn command_result_with_state_delta(
    kind: &str,
    title: &str,
    message: &str,
    severity: &str,
    delta: &SlashStateDelta,
) -> Value {
    with_state_delta(command_result(kind, title, message, severity), delta)
}

fn with_state_delta(mut result: Value, delta: &SlashStateDelta) -> Value {
    if !delta.is_empty()
        && let Some(object) = result.as_object_mut()
        && let Ok(value) = serde_json::to_value(delta)
    {
        object.insert("state_delta".to_string(), value);
    }
    result
}

fn start_task_result(label: &str, task: String) -> Value {
    serde_json::json!({
        "kind": "start_task",
        "title": label,
        "message": format!("{label}: starting"),
        "task": task,
        "label": label,
        "severity": "info",
    })
}

async fn handle_command_session_list(
    state: &DaemonState,
    raw_command: &str,
) -> Result<Value, String> {
    let result = session_list_result(state).await;
    let sessions = result["sessions"].as_array().cloned().unwrap_or_default();
    let items: Vec<Value> = sessions
        .iter()
        .map(|session| {
            let id = session["id"].as_str().unwrap_or_default();
            let title = session["title"]
                .as_str()
                .or_else(|| session["summary"].as_str())
                .unwrap_or(id);
            let status = session["status"].as_str().unwrap_or("idle");
            let detail = match session["last_task"].as_str() {
                Some(task) if !task.trim().is_empty() => task,
                _ => status,
            };
            serde_json::json!({
                "label": title,
                "detail": detail,
                "source": status,
                "session_id": id,
            })
        })
        .collect();
    Ok(serde_json::json!({
        "kind": "session_list",
        "title": "Sessions",
        "message": if items.is_empty() { "sessions: <none>".to_string() } else { format!("sessions: {} total", items.len()) },
        "severity": "info",
        "command": raw_command,
        "items": items,
        "sessions": sessions,
        "total": items.len(),
    }))
}

fn handle_command_session_count(state: &DaemonState, raw_command: &str) -> Result<Value, String> {
    let store = MemoryStore::new(state.project_root.join(".peridot/memory.db"));
    let records = store
        .list_session_records()
        .map_err(|err| format!("failed to read session records: {err}"))?;
    let summary = session_count_summary(&records);
    let items = vec![
        serde_json::json!({ "label": "idle", "detail": summary.idle.to_string() }),
        serde_json::json!({ "label": "running", "detail": summary.running.to_string() }),
        serde_json::json!({ "label": "suspended", "detail": summary.suspended.to_string() }),
        serde_json::json!({ "label": "done", "detail": summary.done.to_string() }),
        serde_json::json!({ "label": "failed", "detail": summary.failed.to_string() }),
    ];
    Ok(serde_json::json!({
        "kind": "session_count",
        "title": "Session Count",
        "message": format!(
            "session count: {} total ({} running, {} suspended, {} done, {} failed)",
            summary.total, summary.running, summary.suspended, summary.done, summary.failed
        ),
        "severity": "info",
        "command": raw_command,
        "items": items,
        "total": summary.total,
        "idle": summary.idle,
        "running": summary.running,
        "suspended": summary.suspended,
        "done": summary.done,
        "failed": summary.failed,
    }))
}

async fn handle_command_clear(
    state: &DaemonState,
    session_id: Option<&str>,
    raw_command: &str,
) -> Result<Value, String> {
    let (cancelled, deleted) = if let Some(session_id) = session_id {
        let cancelled =
            remove_live_daemon_session(state, session_id, SessionLifecycle::Suspended).await;
        let deleted = delete_persisted_session_for_daemon(state, session_id)?;
        if cancelled || deleted {
            emit_session_list_changed(state).await;
        }
        (cancelled, deleted)
    } else {
        (false, false)
    };
    let message = if session_id.is_some() {
        "clear: transcript + context wiped, new session"
    } else {
        "clear: no active daemon session; clear local transcript"
    };
    Ok(serde_json::json!({
        "kind": "client_action",
        "action": "clear",
        "title": "Clear",
        "message": message,
        "severity": "info",
        "command": raw_command,
        "session_id": session_id,
        "deleted": deleted,
        "cancelled": cancelled,
        "items": [
            { "label": "session", "detail": session_id.unwrap_or("<none>") },
            { "label": "deleted persisted data", "detail": deleted.to_string() },
            { "label": "cancelled live run", "detail": cancelled.to_string() },
        ],
    }))
}

async fn handle_command_session_delete(
    state: &DaemonState,
    raw_command: &str,
    target: &str,
) -> Result<Value, String> {
    handle_command_session_remove(
        state,
        raw_command,
        target,
        "session_delete",
        "Session Delete",
    )
    .await
}

async fn handle_command_session_close(
    state: &DaemonState,
    raw_command: &str,
    target: &str,
) -> Result<Value, String> {
    handle_command_session_remove(state, raw_command, target, "session_close", "Session Close")
        .await
}

async fn handle_command_session_switch(
    state: &DaemonState,
    raw_command: &str,
    target: &str,
) -> Result<Value, String> {
    let session_id = resolve_session_target_id(state, target).await?;
    let Some(session_id) = session_id else {
        return Ok(serde_json::json!({
            "kind": "session_switch",
            "title": "Session Switch",
            "message": format!("session switch: {target} not found"),
            "severity": "error",
            "command": raw_command,
            "target": target,
            "switched": false,
        }));
    };
    let sessions = session_list_result(state).await;
    let session = sessions["sessions"]
        .as_array()
        .and_then(|items| items.iter().find(|item| item["id"] == session_id));
    let title = session
        .and_then(|item| item["title"].as_str())
        .unwrap_or(session_id.as_str());
    let status = session
        .and_then(|item| item["status"].as_str())
        .unwrap_or("idle");
    let running = session
        .and_then(|item| item["running"].as_bool())
        .unwrap_or(false);
    Ok(serde_json::json!({
        "kind": "session_switch",
        "title": "Session Switch",
        "message": format!("session switch: {session_id}"),
        "severity": "info",
        "command": raw_command,
        "session_id": session_id,
        "target": target,
        "session_title": title,
        "status": status,
        "running": running,
        "switched": true,
        "items": [
            { "label": "session", "detail": session_id },
            { "label": "title", "detail": title },
            { "label": "status", "detail": status },
        ],
    }))
}

async fn handle_command_session_remove(
    state: &DaemonState,
    raw_command: &str,
    target: &str,
    kind: &str,
    title: &str,
) -> Result<Value, String> {
    let session_id = resolve_session_target_id(state, target)
        .await?
        .unwrap_or_else(|| target.to_string());
    let cancelled =
        remove_live_daemon_session(state, &session_id, SessionLifecycle::Suspended).await;
    let deleted = delete_persisted_session_for_daemon(state, &session_id)?;
    if cancelled || deleted {
        emit_session_list_changed(state).await;
    }
    let (label, success_word) = if kind == "session_close" {
        ("close", "closed")
    } else {
        ("delete", "deleted")
    };
    Ok(serde_json::json!({
        "kind": kind,
        "title": title,
        "message": format!("session {label}: {session_id} {}", if deleted || cancelled { success_word } else { "not found" }),
        "severity": if deleted || cancelled { "info" } else { "error" },
        "command": raw_command,
        "session_id": session_id,
        "target": target,
        "deleted": deleted,
        "cancelled": cancelled,
        "items": [
            { "label": "session", "detail": session_id },
            { "label": "deleted persisted data", "detail": deleted.to_string() },
            { "label": "cancelled live run", "detail": cancelled.to_string() },
        ],
    }))
}

async fn handle_command_session_rename(
    state: &DaemonState,
    raw_command: &str,
    target: &str,
    title: &str,
) -> Result<Value, String> {
    let session_id = resolve_session_target_id(state, target)
        .await?
        .unwrap_or_else(|| target.to_string());
    let renamed = rename_persisted_session_for_daemon(state, &session_id, title)?;
    if renamed {
        emit_session_list_changed(state).await;
    }
    Ok(serde_json::json!({
        "kind": "session_rename",
        "title": "Session Rename",
        "message": if renamed {
            format!("session rename: {session_id} -> {title}")
        } else {
            format!("session rename: {session_id} not found")
        },
        "severity": if renamed { "info" } else { "error" },
        "command": raw_command,
        "session_id": session_id,
        "target": target,
        "session_title": title,
        "renamed": renamed,
        "items": [
            { "label": "session", "detail": session_id },
            { "label": "title", "detail": title },
            { "label": "renamed", "detail": renamed.to_string() },
        ],
    }))
}

async fn resolve_session_target_id(
    state: &DaemonState,
    target: &str,
) -> Result<Option<String>, String> {
    let target = target.trim();
    if target.is_empty() {
        return Ok(None);
    }
    let list = session_list_result(state).await;
    let sessions = list["sessions"].as_array().cloned().unwrap_or_default();
    if let Some(id) = sessions
        .iter()
        .filter_map(|session| session["id"].as_str())
        .find(|id| *id == target)
    {
        return Ok(Some(id.to_string()));
    }

    let needle = target.to_ascii_lowercase();
    let mut exact = Vec::new();
    let mut partial = Vec::new();
    for session in &sessions {
        let id = session["id"].as_str().unwrap_or_default();
        let title = session["title"]
            .as_str()
            .or_else(|| session["summary"].as_str())
            .unwrap_or_default();
        let title_lower = title.to_ascii_lowercase();
        if title_lower == needle {
            exact.push(id.to_string());
        } else if title_lower.contains(&needle) {
            partial.push(id.to_string());
        }
    }
    let matches = if exact.is_empty() { partial } else { exact };
    match matches.as_slice() {
        [] => Ok(None),
        [id] => Ok(Some(id.clone())),
        many => Err(format!(
            "session target '{target}' is ambiguous: {}",
            many.join(", ")
        )),
    }
}

async fn remove_live_daemon_session(
    state: &DaemonState,
    session_id: &str,
    lifecycle: SessionLifecycle,
) -> bool {
    let removed = if let Some(entry) = state.sessions.lock().await.remove(session_id) {
        entry.cancel.cancel();
        if let Some(task) = entry.task {
            task.abort();
        }
        true
    } else {
        false
    };
    if removed {
        state
            .router
            .lock()
            .expect("daemon session router mutex poisoned")
            .close(session_id);
        clear_pending_ask_user_for_session(state, session_id);
        update_daemon_session_lifecycle(state, session_id, lifecycle).await;
    }
    removed
}

fn delete_persisted_session_for_daemon(
    state: &DaemonState,
    session_id: &str,
) -> Result<bool, String> {
    let store = MemoryStore::new(state.project_root.join(".peridot/memory.db"));
    let deleted_summary = store
        .delete_session(session_id)
        .map_err(|err| format!("delete session summary: {err}"))?;
    let deleted_record = store
        .delete_session_record(session_id)
        .map_err(|err| format!("delete session record: {err}"))?;
    let sessions_root = state.project_root.join(".peridot").join("sessions");
    let deleted_blobs = peridot_memory::remove_session_dir(&sessions_root, session_id)
        .map_err(|err| format!("delete session blobs: {err}"))?;
    Ok(deleted_summary || deleted_record || deleted_blobs)
}

fn rename_persisted_session_for_daemon(
    state: &DaemonState,
    session_id: &str,
    title: &str,
) -> Result<bool, String> {
    let store = MemoryStore::new(state.project_root.join(".peridot/memory.db"));
    let existing_summary = store
        .get_session(session_id)
        .map_err(|err| format!("read session summary: {err}"))?;
    let existing_record = store
        .get_session_record(session_id)
        .map_err(|err| format!("read session record: {err}"))?;
    let sessions_root = state.project_root.join(".peridot").join("sessions");
    let existing_blob =
        peridot_memory::load_session_blob(&sessions_root, session_id, "tui_state.json")
            .map_err(|err| format!("read session blob: {err}"))?;
    if existing_summary.is_none() && existing_record.is_none() && existing_blob.is_none() {
        return Ok(false);
    }
    store
        .save_session(&SessionSummary {
            id: session_id.to_string(),
            summary: title.to_string(),
        })
        .map_err(|err| format!("save session summary: {err}"))?;
    if let Some(mut record) = existing_record {
        record.summary = title.to_string();
        record.updated_at_unix = crate::run_state::unix_timestamp();
        store
            .save_session_record(&record)
            .map_err(|err| format!("save session record: {err}"))?;
    }
    if let Some(bytes) = existing_blob
        && let Ok(mut tui_state) = serde_json::from_slice::<peridot_tui::TuiState>(&bytes)
    {
        for item in &mut tui_state.sessions {
            if item.id == session_id {
                item.title = title.to_string();
                item.title_generated = true;
            }
        }
        let serialized = serde_json::to_vec(&tui_state)
            .map_err(|err| format!("serialize session blob: {err}"))?;
        peridot_memory::save_session_blob(
            &sessions_root,
            session_id,
            "tui_state.json",
            &serialized,
        )
        .map_err(|err| format!("save session blob: {err}"))?;
    }
    Ok(true)
}

async fn handle_command_session_save(
    state: &DaemonState,
    session_id: Option<&str>,
    raw_command: &str,
) -> Result<Value, String> {
    let Some(session_id) = session_id else {
        return Err("session save requires an active session".to_string());
    };
    let live_spec = state
        .sessions
        .lock()
        .await
        .get(session_id)
        .map(|entry| entry.spec.clone());
    let store = MemoryStore::new(state.project_root.join(".peridot/memory.db"));
    if let Some(spec) = live_spec.as_ref() {
        save_daemon_session_record(state, session_id, spec, SessionLifecycle::Running, None).await;
    } else if let Some(mut record) = store
        .get_session_record(session_id)
        .map_err(|err| format!("failed to read session record: {err}"))?
    {
        record.updated_at_unix = crate::run_state::unix_timestamp();
        store
            .save_session_record(&record)
            .map_err(|err| format!("failed to save session record: {err}"))?;
    } else {
        return Err(format!("session not found: {session_id}"));
    }
    let record = store
        .get_session_record(session_id)
        .map_err(|err| format!("failed to read saved session record: {err}"))?
        .ok_or_else(|| format!("session not found after save: {session_id}"))?;
    let title = record_title(&record);
    store
        .save_session(&SessionSummary {
            id: record.id.clone(),
            summary: if record.summary.trim().is_empty() {
                title.clone()
            } else {
                record.summary.clone()
            },
        })
        .map_err(|err| format!("failed to save legacy session summary: {err}"))?;
    Ok(serde_json::json!({
        "kind": "session_save",
        "title": "Session Saved",
        "message": format!("session saved: {session_id}"),
        "severity": "info",
        "command": raw_command,
        "session_id": session_id,
        "status": format!("{:?}", record.status).to_ascii_lowercase(),
        "summary": record.summary,
        "label": title,
        "updated_at_unix": record.updated_at_unix,
        "total_tokens": record.total_tokens,
        "total_cost_usd": record.total_cost_usd,
        "turns_used": record.turns_used,
        "items": [
            { "label": "session", "detail": session_id },
            { "label": "status", "detail": format!("{:?}", record.status).to_ascii_lowercase() },
            { "label": "tokens", "detail": record.total_tokens.to_string() },
            { "label": "cost", "detail": format!("${:.4}", record.total_cost_usd) },
        ],
    }))
}

async fn handle_command_goal_control(
    state: &DaemonState,
    session_id: Option<&str>,
    raw_command: &str,
    command: &SlashCommand,
) -> Result<Value, String> {
    let Some(session_id) = session_id else {
        return Ok(goal_command_result(
            raw_command,
            None,
            "none",
            None,
            0,
            0,
            "goal: no active session",
        ));
    };
    let Some((goal, plan)) = ({
        let sessions = state.sessions.lock().await;
        sessions.get(session_id).map(|entry| {
            let mut goal = entry.goal.lock().unwrap();
            if goal.objective.is_none() && entry.spec.mode == ExecutionMode::Goal {
                *goal = initial_live_goal(&entry.spec);
            }
            match command {
                SlashCommand::GoalPause if goal.objective.is_some() => {
                    goal.status = Some(GoalStatus::Paused);
                }
                SlashCommand::GoalResume if goal.objective.is_some() => {
                    goal.status = Some(GoalStatus::Running);
                }
                SlashCommand::GoalClear => {
                    *goal = LiveSessionGoal {
                        objective: None,
                        status: Some(GoalStatus::Cleared),
                        started_at_unix: None,
                    };
                    *entry.plan.lock().unwrap() = LiveSessionPlan::default();
                }
                _ => {}
            }
            (goal.clone(), entry.plan.lock().unwrap().clone())
        })
    }) else {
        return Ok(goal_command_result(
            raw_command,
            Some(session_id),
            "missing",
            None,
            0,
            0,
            &format!("goal: session not found: {session_id}"),
        ));
    };
    let done = plan.steps.iter().filter(|step| step.done).count();
    let total = plan.steps.len();
    let status = goal
        .status
        .as_ref()
        .map(goal_status_label)
        .unwrap_or("none");
    let message = match command {
        SlashCommand::GoalPause if goal.objective.is_some() => "goal: paused".to_string(),
        SlashCommand::GoalResume if goal.objective.is_some() => "goal: resumed".to_string(),
        SlashCommand::GoalClear => "goal: cleared".to_string(),
        _ => format!("goal: {status} {done}/{total} steps done"),
    };
    Ok(goal_command_result(
        raw_command,
        Some(session_id),
        status,
        goal.objective.as_deref(),
        done,
        total,
        &message,
    )
    .with_object("started_at_unix", goal.started_at_unix))
}

trait JsonObjectExt {
    fn with_object(self, key: &str, value: Option<u64>) -> Value;
}

impl JsonObjectExt for Value {
    fn with_object(mut self, key: &str, value: Option<u64>) -> Value {
        if let Some(object) = self.as_object_mut() {
            object.insert(
                key.to_string(),
                serde_json::to_value(value).unwrap_or(Value::Null),
            );
        }
        self
    }
}

fn goal_command_result(
    raw_command: &str,
    session_id: Option<&str>,
    status: &str,
    objective: Option<&str>,
    done: usize,
    total: usize,
    message: &str,
) -> Value {
    serde_json::json!({
        "kind": "goal",
        "title": "Goal",
        "message": message,
        "severity": "info",
        "command": raw_command,
        "session_id": session_id,
        "status": status,
        "objective": objective,
        "done": done,
        "total": total,
        "items": [
            { "label": "status", "detail": status },
            { "label": "objective", "detail": objective.unwrap_or("<none>") },
            { "label": "steps", "detail": format!("{done}/{total}") },
        ],
    })
}

fn goal_status_label(status: &GoalStatus) -> &'static str {
    match status {
        GoalStatus::Running => "running",
        GoalStatus::Paused => "paused",
        GoalStatus::Done => "done",
        GoalStatus::Cleared => "cleared",
    }
}

async fn handle_command_plan_show(
    state: &DaemonState,
    session_id: Option<&str>,
    raw_command: &str,
) -> Result<Value, String> {
    let plan = if let Some(session_id) = session_id {
        state
            .sessions
            .lock()
            .await
            .get(session_id)
            .map(|entry| entry.plan.lock().unwrap().clone())
    } else {
        None
    }
    .unwrap_or_default();
    let done = plan.steps.iter().filter(|step| step.done).count();
    let total = plan.steps.len();
    let items: Vec<Value> = plan
        .steps
        .iter()
        .enumerate()
        .map(|(index, step)| {
            let current = plan.current == Some(index as u32);
            let status = if step.done {
                "done"
            } else if current {
                "in_progress"
            } else {
                "pending"
            };
            serde_json::json!({
                "label": format!("{}. {}", index + 1, step.label),
                "detail": status,
                "source": status,
                "step_index": index,
                "done": step.done,
                "current": current,
            })
        })
        .collect();
    Ok(serde_json::json!({
        "kind": "plan",
        "title": "Plan",
        "message": if total == 0 {
            "plan: <empty>".to_string()
        } else {
            format!("plan: {done}/{total} steps")
        },
        "severity": "info",
        "command": raw_command,
        "items": items,
        "session_id": session_id,
        "steps": plan
            .steps
            .iter()
            .enumerate()
            .map(|(index, step)| {
                let current = plan.current == Some(index as u32);
                let status = if step.done {
                    "done"
                } else if current {
                    "in_progress"
                } else {
                    "pending"
                };
                serde_json::json!({
                    "text": step.label,
                    "label": step.label,
                    "done": step.done,
                    "status": status,
                    "current": current,
                })
            })
            .collect::<Vec<_>>(),
        "current": plan.current,
        "done": done,
        "total": total,
    }))
}

#[derive(Clone, Debug)]
struct CostSessionRow {
    id: String,
    title: String,
    status: String,
    task: Option<String>,
    executor_tokens: u64,
    executor_cost_usd: f64,
    committee_tokens: u64,
    committee_cost_usd: f64,
    turns_used: u32,
}

impl CostSessionRow {
    fn all_in_tokens(&self) -> u64 {
        self.executor_tokens + self.committee_tokens
    }

    fn all_in_cost_usd(&self) -> f64 {
        self.executor_cost_usd + self.committee_cost_usd
    }
}

async fn handle_command_cost(
    state: &DaemonState,
    session_id: Option<&str>,
    raw_command: &str,
) -> Result<Value, String> {
    let rows = cost_session_rows(state).await?;
    let current = session_id.and_then(|id| rows.iter().find(|row| row.id == id).cloned());
    let executor_tokens: u64 = rows.iter().map(|row| row.executor_tokens).sum();
    let executor_cost_usd: f64 = rows.iter().map(|row| row.executor_cost_usd).sum();
    let committee_tokens: u64 = rows.iter().map(|row| row.committee_tokens).sum();
    let committee_cost_usd: f64 = rows.iter().map(|row| row.committee_cost_usd).sum();
    let total_tokens = executor_tokens + committee_tokens;
    let total_cost_usd = executor_cost_usd + committee_cost_usd;
    let budget_limit = current_budget_limit(state, session_id).await;
    let budget_pct = budget_limit
        .filter(|limit| *limit > 0.0)
        .map(|limit| total_cost_usd / limit * 100.0);
    let current_cost = current
        .as_ref()
        .map(CostSessionRow::all_in_cost_usd)
        .unwrap_or(total_cost_usd);
    let current_tokens = current
        .as_ref()
        .map(CostSessionRow::all_in_tokens)
        .unwrap_or(total_tokens);
    let items: Vec<Value> = rows
        .iter()
        .map(|row| {
            serde_json::json!({
                "label": row.title,
                "detail": format!(
                    "${:.4} · {} tok · {} turn(s)",
                    row.all_in_cost_usd(),
                    row.all_in_tokens(),
                    row.turns_used
                ),
                "source": row.status,
                "session_id": row.id,
                "tokens": row.all_in_tokens(),
                "total_cost_usd": row.all_in_cost_usd(),
                "executor_tokens": row.executor_tokens,
                "executor_cost_usd": row.executor_cost_usd,
                "committee_tokens": row.committee_tokens,
                "committee_cost_usd": row.committee_cost_usd,
                "turns_used": row.turns_used,
                "last_task": row.task,
            })
        })
        .collect();
    let message = if rows.is_empty() {
        "cost: $0.0000 · tokens: 0 · sessions: 0".to_string()
    } else if let Some(limit) = budget_limit.filter(|limit| *limit > 0.0) {
        format!(
            "cost: ${current_cost:.4} · tokens: {current_tokens} · aggregate: ${total_cost_usd:.4} / ${limit:.4} ({:.0}%) across {} session(s)",
            budget_pct.unwrap_or_default(),
            rows.len()
        )
    } else {
        format!(
            "cost: ${current_cost:.4} · tokens: {current_tokens} · aggregate: ${total_cost_usd:.4} across {} session(s)",
            rows.len()
        )
    };
    Ok(serde_json::json!({
        "kind": "cost",
        "title": "Cost",
        "message": message,
        "severity": "info",
        "command": raw_command,
        "items": items,
        "session_id": session_id,
        "session_count": rows.len(),
        "current_cost_usd": current_cost,
        "current_tokens": current_tokens,
        "total_tokens": total_tokens,
        "total_cost_usd": total_cost_usd,
        "executor_tokens": executor_tokens,
        "executor_cost_usd": executor_cost_usd,
        "committee_tokens": committee_tokens,
        "committee_cost_usd": committee_cost_usd,
        "budget_limit_usd": budget_limit,
        "budget_pct": budget_pct,
    }))
}

async fn cost_session_rows(state: &DaemonState) -> Result<Vec<CostSessionRow>, String> {
    let store = MemoryStore::new(state.project_root.join(".peridot/memory.db"));
    let records = store
        .list_session_records()
        .map_err(|err| format!("failed to read session records: {err}"))?;
    let mut rows = BTreeMap::<String, CostSessionRow>::new();
    for record in records {
        rows.insert(
            record.id.clone(),
            CostSessionRow {
                id: record.id.clone(),
                title: record_title(&record),
                status: format!("{:?}", record.status).to_ascii_lowercase(),
                task: record.last_task.clone(),
                executor_tokens: record.total_tokens,
                executor_cost_usd: record.total_cost_usd,
                committee_tokens: 0,
                committee_cost_usd: 0.0,
                turns_used: record.turns_used,
            },
        );
    }
    for (id, entry) in state.sessions.lock().await.iter() {
        let usage = entry.usage.lock().unwrap().clone();
        let row = rows.entry(id.clone()).or_insert_with(|| CostSessionRow {
            id: id.clone(),
            title: session_title_from_task(&entry.spec.task),
            status: "running".to_string(),
            task: Some(entry.spec.task.clone()),
            executor_tokens: 0,
            executor_cost_usd: 0.0,
            committee_tokens: 0,
            committee_cost_usd: 0.0,
            turns_used: 0,
        });
        row.status = "running".to_string();
        row.task = Some(entry.spec.task.clone());
        row.executor_tokens = row.executor_tokens.max(usage.total_tokens);
        row.executor_cost_usd = row.executor_cost_usd.max(usage.cost_usd);
        row.turns_used = row.turns_used.max(usage.turns_used);
        row.committee_tokens = usage.committee_planner_tokens + usage.committee_reviewer_tokens;
        row.committee_cost_usd =
            usage.committee_planner_cost_usd + usage.committee_reviewer_cost_usd;
    }
    Ok(rows.into_values().collect())
}

async fn current_budget_limit(state: &DaemonState, session_id: Option<&str>) -> Option<f64> {
    if let Some(session_id) = session_id
        && let Some(entry) = state.sessions.lock().await.get(session_id)
    {
        let usage = entry.usage.lock().unwrap().clone();
        return usage.cost_limit.or_else(|| {
            (entry.spec.config.defaults.budget_usd > 0.0)
                .then_some(entry.spec.config.defaults.budget_usd)
        });
    }
    (state.run_template.budget_usd > 0.0).then_some(state.run_template.budget_usd)
}

async fn handle_command_info(
    state: &DaemonState,
    session_id: Option<&str>,
    raw_command: &str,
) -> Result<Value, String> {
    let live_spec = if let Some(session_id) = session_id {
        state
            .sessions
            .lock()
            .await
            .get(session_id)
            .map(|entry| entry.spec.clone())
    } else {
        None
    };
    let store = MemoryStore::new(state.project_root.join(".peridot/memory.db"));
    let record = session_id.and_then(|id| {
        store
            .list_session_records()
            .unwrap_or_default()
            .into_iter()
            .find(|record| record.id == id)
    });
    let config = live_spec
        .as_ref()
        .map(|spec| spec.config.clone())
        .unwrap_or_else(|| state.run_config.as_ref().clone());
    let session_label = session_id.unwrap_or("<none>");
    let workspace_root = record
        .as_ref()
        .map(|record| record.workspace_root.clone())
        .unwrap_or_else(|| state.project_root.as_ref().clone());
    let workspace = workspace_root
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("<unknown>")
        .to_string();
    let model = live_spec
        .as_ref()
        .and_then(|spec| spec.model.clone())
        .unwrap_or_else(|| {
            if live_spec.is_some() {
                config.models.main.clone()
            } else {
                state.run_template.model.clone()
            }
        });
    let provider = config.auth.primary.clone();
    let mode = live_spec
        .as_ref()
        .map(|spec| spec.mode)
        .unwrap_or(config.defaults.mode);
    let permission = live_spec
        .as_ref()
        .map(|spec| spec.permission)
        .unwrap_or(state.run_template.permission);
    let reasoning_effort = live_spec
        .as_ref()
        .and_then(|spec| spec.reasoning_effort)
        .unwrap_or(state.run_template.reasoning_effort);
    let service_tier = match live_spec
        .as_ref()
        .and_then(|spec| spec.service_tier.as_ref())
    {
        Some(Some(tier)) => Some(tier.clone()),
        Some(None) => None,
        None => state.run_template.service_tier.clone(),
    };
    let status = if live_spec.is_some() {
        "running".to_string()
    } else if let Some(record) = record.as_ref() {
        format!("{:?}", record.status).to_ascii_lowercase()
    } else if session_id.is_some() {
        "unknown".to_string()
    } else {
        "workspace".to_string()
    };
    let turns_used = record.as_ref().map(|record| record.turns_used).unwrap_or(0);
    let total_tokens = record
        .as_ref()
        .map(|record| record.total_tokens)
        .unwrap_or(0);
    let total_cost_usd = record
        .as_ref()
        .map(|record| record.total_cost_usd)
        .unwrap_or(0.0);
    let mut items = vec![
        serde_json::json!({ "label": "session", "detail": session_label }),
        serde_json::json!({ "label": "workspace", "detail": workspace }),
        serde_json::json!({ "label": "status", "detail": status }),
        serde_json::json!({ "label": "model", "detail": model }),
        serde_json::json!({ "label": "provider", "detail": provider }),
        serde_json::json!({ "label": "mode", "detail": mode.to_string() }),
        serde_json::json!({ "label": "permission", "detail": permission.to_string() }),
        serde_json::json!({ "label": "reasoning", "detail": reasoning_effort.to_string() }),
        serde_json::json!({
            "label": "service tier",
            "detail": service_tier.as_deref().unwrap_or("standard")
        }),
        serde_json::json!({ "label": "turns", "detail": turns_used.to_string() }),
        serde_json::json!({ "label": "tokens", "detail": total_tokens.to_string() }),
        serde_json::json!({ "label": "cost", "detail": format!("${total_cost_usd:.4}") }),
    ];
    if let Some(record) = record.as_ref() {
        if let Some(task) = record.last_task.as_deref()
            && !task.trim().is_empty()
        {
            items.push(serde_json::json!({ "label": "last task", "detail": task }));
        }
        if let Some(branch) = record.worktree_branch.as_deref() {
            items.push(serde_json::json!({ "label": "worktree branch", "detail": branch }));
        }
    }
    Ok(serde_json::json!({
        "kind": "info",
        "title": "Session Info",
        "message": format!(
            "info: session {session_label} · workspace {workspace} · model {model} · provider {provider} · mode {mode} · permission {permission} · turn {turns_used} · tokens {total_tokens} · cost ${total_cost_usd:.4}"
        ),
        "severity": "info",
        "command": raw_command,
        "items": items,
        "session_id": session_id,
        "workspace": workspace,
        "workspace_root": workspace_root,
        "status": status,
        "model": model,
        "provider": provider,
        "mode": mode.to_string(),
        "permission": permission.to_string(),
        "reasoning_effort": reasoning_effort.to_string(),
        "service_tier": service_tier,
        "committee_mode": config.committee.mode.to_string(),
        "turns_used": turns_used,
        "total_tokens": total_tokens,
        "total_cost_usd": total_cost_usd,
    }))
}

async fn handle_interaction_respond(
    state: &DaemonState,
    id: Value,
    params: Option<Value>,
) -> Result<()> {
    let Some(Value::Object(params)) = params else {
        emit_error(
            state,
            id,
            -32602,
            "params must be an object with `request_id` and `answer` fields".to_string(),
        )?;
        return Ok(());
    };
    let Some(request_id) = params.get("request_id").and_then(Value::as_str) else {
        emit_error(
            state,
            id,
            -32602,
            "params.request_id must be a string".to_string(),
        )?;
        return Ok(());
    };
    let Some(answer_value) = params.get("answer") else {
        emit_error(state, id, -32602, "params.answer is required".to_string())?;
        return Ok(());
    };
    let answer = match parse_ask_user_answer(answer_value) {
        Ok(answer) => answer,
        Err(err) => {
            emit_error(
                state,
                id,
                -32602,
                format!("params.answer is not a valid AskUserAnswer: {err}"),
            )?;
            return Ok(());
        }
    };

    let accepted = {
        let sender = state.ask_user_pending.lock().unwrap().remove(request_id);
        sender
            .map(|sender| sender.send(answer).is_ok())
            .unwrap_or(false)
    };
    emit_response(
        state,
        id,
        serde_json::json!({
            "accepted": accepted,
            "request_id": request_id,
        }),
    )
}

async fn handle_approval_respond(
    state: &DaemonState,
    id: Value,
    params: Option<Value>,
) -> Result<()> {
    let Some(Value::Object(params)) = params else {
        emit_error(
            state,
            id,
            -32602,
            "params must be an object with `session_id` and `approved` fields".to_string(),
        )?;
        return Ok(());
    };
    let Some(session_id) = params.get("session_id").and_then(Value::as_str) else {
        emit_error(
            state,
            id,
            -32602,
            "params.session_id must be a string".to_string(),
        )?;
        return Ok(());
    };
    let Some(approved) = params.get("approved").and_then(Value::as_bool) else {
        emit_error(
            state,
            id,
            -32602,
            "params.approved must be a boolean".to_string(),
        )?;
        return Ok(());
    };
    let scope = match params
        .get("scope")
        .and_then(Value::as_str)
        .map(parse_approval_scope)
        .transpose()
    {
        Ok(scope) => scope.unwrap_or(ApprovalScope::Once),
        Err(message) => {
            emit_error(state, id, -32602, message)?;
            return Ok(());
        }
    };

    if !approved {
        let removed = if let Some(entry) = state.sessions.lock().await.remove(session_id) {
            entry.cancel.cancel();
            if let Some(task) = entry.task {
                task.abort();
            }
            state
                .router
                .lock()
                .expect("daemon session router mutex poisoned")
                .close(session_id);
            true
        } else {
            false
        };
        clear_pending_ask_user_for_session(state, session_id);
        emit_event(
            state,
            session_id,
            serde_json::json!({
                "kind": "approval_denied",
                "session_id": session_id,
            }),
        );
        emit_response(
            state,
            id,
            serde_json::json!({
                "accepted": removed,
                "resumed": false,
                "session_id": session_id,
            }),
        )?;
        return Ok(());
    }

    let (spec, cancel_for_task, compact_request, usage, plan, parameters_overridden) = {
        let mut sessions = state.sessions.lock().await;
        let Some(entry) = sessions.get_mut(session_id) else {
            emit_response(
                state,
                id,
                serde_json::json!({
                    "accepted": false,
                    "resumed": false,
                    "session_id": session_id,
                    "message": "session not found",
                }),
            )?;
            return Ok(());
        };
        let Some(snapshot) = entry.waiting_approval.clone() else {
            emit_response(
                state,
                id,
                serde_json::json!({
                    "accepted": false,
                    "resumed": false,
                    "session_id": session_id,
                    "message": "session is not waiting for approval",
                }),
            )?;
            return Ok(());
        };
        let snapshot = match approval_snapshot_from_response(&snapshot, &params) {
            Ok(snapshot) => snapshot,
            Err(message) => {
                emit_error(state, id, -32602, message)?;
                return Ok(());
            }
        };
        let parameters_overridden = params.get("parameters").is_some();
        let grant = approval_grant_from_snapshot(&snapshot, scope);
        entry.approval_grants.push(grant);
        entry.waiting_approval = None;
        let cancel = CancelToken::new();
        let compact_request = entry.compact_request.clone();
        entry.cancel = cancel.clone();
        if let Some(handle) = state
            .router
            .lock()
            .expect("daemon session router mutex poisoned")
            .get_mut(session_id)
        {
            handle.cancel = cancel.clone();
        }
        let spec = entry.spec.clone();
        let usage = entry.usage.clone();
        let plan = entry.plan.clone();
        (
            spec,
            cancel,
            compact_request,
            usage,
            plan,
            parameters_overridden,
        )
    };

    if parameters_overridden {
        let _ = rewrite_pending_resume_parameters(state, session_id, &params["parameters"]);
    }

    let state_for_task = state.clone();
    let session_id_for_task = session_id.to_string();
    let spec_for_task = spec.clone();
    let handle = tokio::spawn(async move {
        run_session_task(
            state_for_task,
            session_id_for_task,
            spec_for_task,
            cancel_for_task,
            compact_request,
            usage,
            plan,
        )
        .await;
    });
    if let Some(entry) = state.sessions.lock().await.get_mut(session_id) {
        entry.task = Some(handle);
    }
    emit_event(
        state,
        session_id,
        serde_json::json!({
            "kind": "approval_resumed",
            "scope": approval_scope_label(scope),
            "parameters_overridden": parameters_overridden,
        }),
    );
    emit_response(
        state,
        id,
        serde_json::json!({
            "accepted": true,
            "resumed": true,
            "session_id": session_id,
            "parameters_overridden": parameters_overridden,
        }),
    )
}

fn clear_pending_ask_user_for_session(state: &DaemonState, session_id: &str) {
    let prefix = format!("{session_id}:");
    let mut pending = state.ask_user_pending.lock().unwrap();
    pending.retain(|request_id, _| !request_id.starts_with(&prefix));
}

async fn mark_session_waiting_approval(
    state: &DaemonState,
    session_id: &str,
    approval: Option<ApprovalRequestSnapshot>,
) {
    if let Some(entry) = state.sessions.lock().await.get_mut(session_id) {
        entry.task = None;
        entry.waiting_approval = approval;
    }
}

async fn apply_session_approval_grants(
    state: &DaemonState,
    session_id: &str,
    config: &mut PeridotConfig,
) {
    let grants = state
        .sessions
        .lock()
        .await
        .get(session_id)
        .map(|entry| entry.approval_grants.clone())
        .unwrap_or_default();
    for grant in grants {
        apply_approval_grant_to_config(config, &grant);
    }
}

fn approval_grant_from_snapshot(
    snapshot: &ApprovalRequestSnapshot,
    scope: ApprovalScope,
) -> ApprovalGrant {
    ApprovalGrant {
        tool_name: snapshot.tool_name.clone(),
        reason: snapshot.reason.clone(),
        scope,
        call_key: approved_tool_call_key(&snapshot.tool_name, &snapshot.parameters),
        command: snapshot
            .parameters
            .get("command")
            .and_then(Value::as_str)
            .map(normalize_shell_command_for_grant),
        path: snapshot
            .parameters
            .get("path")
            .and_then(Value::as_str)
            .map(str::to_string),
    }
}

fn approval_snapshot_from_response(
    snapshot: &ApprovalRequestSnapshot,
    params: &serde_json::Map<String, Value>,
) -> Result<ApprovalRequestSnapshot, String> {
    if let Some(tool_name) = params.get("tool_name").and_then(Value::as_str)
        && tool_name != snapshot.tool_name
    {
        return Err(format!(
            "params.tool_name `{tool_name}` does not match pending approval `{}`",
            snapshot.tool_name
        ));
    }
    if let Some(reason) = params.get("reason").and_then(Value::as_str)
        && reason != snapshot.reason
    {
        return Err("params.reason does not match the pending approval reason".to_string());
    }
    let mut next = snapshot.clone();
    if let Some(parameters) = params.get("parameters") {
        next.parameters = parameters.clone();
    }
    Ok(next)
}

fn rewrite_pending_resume_parameters(
    state: &DaemonState,
    session_id: &str,
    parameters: &Value,
) -> bool {
    let path = context_snapshot_path(state, session_id)
        .parent()
        .map(|parent| parent.join("pending_resume.bin"));
    let Some(path) = path else {
        return false;
    };
    let Ok(bytes) = std::fs::read(&path) else {
        return false;
    };
    let Ok(mut call) = serde_json::from_slice::<ToolCall>(&bytes) else {
        return false;
    };
    call.parameters = parameters.clone();
    serde_json::to_vec(&call)
        .ok()
        .and_then(|bytes| std::fs::write(&path, bytes).ok())
        .is_some()
}

fn apply_approval_grant_to_config(config: &mut PeridotConfig, grant: &ApprovalGrant) {
    match grant.scope {
        ApprovalScope::Once | ApprovalScope::Command => {
            push_unique_string(
                &mut config.security.approved_tool_calls,
                grant.call_key.clone(),
            );
            if let Some(command) = grant.command.as_ref() {
                push_unique_string(
                    &mut config.security.approved_shell_commands,
                    command.clone(),
                );
            } else {
                relax_security_for_approval(config, &grant.reason);
            }
        }
        ApprovalScope::Session => {
            push_unique_string(
                &mut config.security.approved_session_tools,
                grant.tool_name.clone(),
            );
            relax_security_for_approval(config, &grant.reason);
        }
        ApprovalScope::Path => {
            if let Some(path) = grant.path.as_ref() {
                push_unique_string(
                    &mut config.security.approved_tool_path_scopes,
                    approved_tool_path_key(&grant.tool_name, path),
                );
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

fn relax_security_for_approval(config: &mut PeridotConfig, reason: &str) {
    if reason.contains("dependency installation") {
        config.security.ask_before_install = false;
    }
    if reason.contains("destructive shell command") {
        config.security.ask_before_delete = false;
    }
}

fn push_unique_string(values: &mut Vec<String>, value: String) {
    if !values.iter().any(|existing| existing == &value) {
        values.push(value);
    }
}

fn approved_tool_call_key(tool_name: &str, parameters: &Value) -> String {
    let encoded = serde_json::to_string(parameters).unwrap_or_else(|_| parameters.to_string());
    format!("{tool_name}:{encoded}")
}

fn approved_tool_path_key(tool_name: &str, path: &str) -> String {
    format!("{tool_name}:{path}")
}

fn normalize_shell_command_for_grant(command: &str) -> String {
    command.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn parse_approval_scope(value: &str) -> Result<ApprovalScope, String> {
    match value.trim().to_ascii_lowercase().as_str() {
        "once" => Ok(ApprovalScope::Once),
        "session" => Ok(ApprovalScope::Session),
        "command" | "always" => Ok(ApprovalScope::Command),
        "path" => Ok(ApprovalScope::Path),
        _ => Err("params.scope must be one of once, session, command, or path".to_string()),
    }
}

fn approval_scope_label(scope: ApprovalScope) -> &'static str {
    match scope {
        ApprovalScope::Once => "once",
        ApprovalScope::Session => "session",
        ApprovalScope::Command => "command",
        ApprovalScope::Path => "path",
    }
}

fn is_approval_required_error(err: &anyhow::Error) -> bool {
    err.to_string().contains("requires explicit user approval")
}

fn context_snapshot_path(state: &DaemonState, session_id: &str) -> PathBuf {
    state
        .project_root
        .join(".peridot")
        .join("sessions")
        .join(session_id)
        .join("context.bin")
}

fn branch_journal_path(state: &DaemonState, session_id: &str) -> PathBuf {
    state
        .project_root
        .join(".peridot")
        .join("sessions")
        .join(session_id)
        .join("branches.json")
}

fn require_session_id(session_id: Option<&str>, command: &str) -> Result<String, String> {
    session_id
        .map(str::to_string)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| format!("{command}: no active session id"))
}

fn read_context_snapshot(
    state: &DaemonState,
    session_id: &str,
) -> Result<Vec<ContextEntry>, String> {
    let snapshot_path = context_snapshot_path(state, session_id);
    if !snapshot_path.exists() {
        return Err("no context snapshot has been written for this session yet".to_string());
    }
    let bytes = std::fs::read(&snapshot_path)
        .map_err(|err| format!("failed to read {}: {err}", snapshot_path.display()))?;
    serde_json::from_slice(&bytes)
        .map_err(|err| format!("failed to parse {}: {err}", snapshot_path.display()))
}

fn write_context_snapshot(
    state: &DaemonState,
    session_id: &str,
    entries: &[ContextEntry],
) -> Result<(), String> {
    let snapshot_path = context_snapshot_path(state, session_id);
    if let Some(parent) = snapshot_path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|err| format!("failed to create {}: {err}", parent.display()))?;
    }
    let bytes = serde_json::to_vec(entries)
        .map_err(|err| format!("failed to serialize context snapshot: {err}"))?;
    std::fs::write(&snapshot_path, bytes)
        .map_err(|err| format!("failed to write {}: {err}", snapshot_path.display()))
}

fn note_result_item(note: &Value) -> Value {
    let ts = note["ts"].as_u64().unwrap_or_default();
    let text = note["text"].as_str().unwrap_or("");
    serde_json::json!({
        "source": "note",
        "label": format!("[{ts}]"),
        "detail": text,
        "ts": ts,
        "text": text,
    })
}

fn handle_command_note(
    state: &DaemonState,
    session_id: Option<&str>,
    raw_command: &str,
    note: &str,
) -> Result<Value, String> {
    let session_id = require_session_id(session_id, "note")?;
    let note = append_session_note(&state.project_root, &session_id, note)
        .map_err(|err| format!("note: failed to save session note: {err}"))?;
    let text = note["text"].as_str().unwrap_or("");
    Ok(serde_json::json!({
        "kind": "note",
        "title": "Note",
        "message": format!("note: {text}"),
        "severity": "info",
        "command": raw_command,
        "session_id": session_id,
        "note": note,
        "items": [note_result_item(&note)],
    }))
}

fn handle_command_notes(
    state: &DaemonState,
    session_id: Option<&str>,
    raw_command: &str,
    last: Option<usize>,
) -> Result<Value, String> {
    let session_id = require_session_id(session_id, "notes")?;
    let (notes, total) = read_session_notes(&state.project_root, &session_id, last)
        .map_err(|err| format!("notes: failed to read session notes: {err}"))?;
    let items: Vec<Value> = notes.iter().map(note_result_item).collect();
    let shown = items.len();
    let message = if shown == 0 {
        format!("notes: none for {session_id}")
    } else if shown < total {
        format!("notes: showing {shown} of {total} for {session_id}")
    } else {
        format!("notes: {total} for {session_id}")
    };
    Ok(serde_json::json!({
        "kind": "notes",
        "title": "Session Notes",
        "message": message,
        "severity": "info",
        "command": raw_command,
        "session_id": session_id,
        "last": last,
        "total": total,
        "items": items,
        "notes": notes,
    }))
}

fn append_plan_reminder_to_context(
    state: &DaemonState,
    session_id: &str,
    content: String,
) -> Result<(), String> {
    let mut entries = read_context_snapshot(state, session_id).unwrap_or_default();
    entries.push(ContextEntry::trusted(ContextSource::PlanReminder, content));
    write_context_snapshot(state, session_id, &entries)
}

fn skill_plan_reminder(skill: &peridot_memory::StoredSkill, args: &str) -> String {
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

fn handle_command_skill_list(state: &DaemonState, raw_command: &str) -> Result<Value, String> {
    command_skill_list_result(state, raw_command, None)
}

fn handle_command_skill_show(
    state: &DaemonState,
    raw_command: &str,
    name: &str,
) -> Result<Value, String> {
    let store = peridot_memory::MemoryStore::new(state.project_root.join(".peridot/memory.db"));
    let records = store
        .list_skill_records()
        .map_err(|err| format!("skills: failed to read skill store: {err}"))?;
    let skill = records
        .into_iter()
        .map(|record| record.skill)
        .find(|skill| skill.name == name)
        .ok_or_else(|| format!("skill not found: {name}"))?;
    let description = skill_description(&skill);
    let label = format!("/{}", skill.name);
    let archived = skill.archived_at_unix > 0;
    Ok(serde_json::json!({
        "kind": "skill_detail",
        "title": format!("Skill: {}", skill.name),
        "message": description.clone(),
        "severity": "info",
        "command": raw_command,
        "name": skill.name,
        "label": label,
        "detail": description,
        "scope": skill.scope,
        "pinned": skill.pinned_at_unix > 0,
        "archived": archived,
        "archived_at_unix": skill.archived_at_unix,
        "last_used_at_unix": skill.last_used_at_unix,
        "body": skill.body,
    }))
}

fn handle_command_skill_search(
    state: &DaemonState,
    raw_command: &str,
    query: &str,
) -> Result<Value, String> {
    let store = peridot_memory::MemoryStore::new(state.project_root.join(".peridot/memory.db"));
    let mut skills = store
        .search_skills(query)
        .map_err(|err| format!("skills: failed to search skill store: {err}"))?;
    skills.sort_by(|a, b| a.scope.cmp(&b.scope).then_with(|| a.name.cmp(&b.name)));
    let rows = skill_inventory_rows(&skills);
    Ok(serde_json::json!({
        "kind": "skills",
        "title": "Skills",
        "message": if rows.is_empty() {
            format!("skills: no matches for `{}`", query.trim())
        } else {
            format!("skills: {} match(es) for `{}`", rows.len(), query.trim())
        },
        "severity": "info",
        "command": raw_command,
        "query": query.trim(),
        "total": rows.len(),
        "items": rows,
    }))
}

fn handle_command_skill_archived(
    state: &DaemonState,
    raw_command: &str,
    query: &str,
) -> Result<Value, String> {
    let query = query.trim();
    let store = peridot_memory::MemoryStore::new(state.project_root.join(".peridot/memory.db"));
    let mut archived: Vec<_> = store
        .list_skill_records()
        .map_err(|err| format!("skills: failed to read skill store: {err}"))?
        .into_iter()
        .filter(|record| record.skill.archived_at_unix > 0)
        .filter(|record| {
            query.is_empty()
                || record.skill.name.contains(query)
                || record.skill.body.contains(query)
                || record.skill.description.contains(query)
        })
        .collect();
    archived.sort_by(|a, b| {
        a.skill
            .scope
            .cmp(&b.skill.scope)
            .then_with(|| a.skill.name.cmp(&b.skill.name))
    });
    let rows = archived_skill_inventory_rows(&archived);
    let message = if rows.is_empty() {
        if query.is_empty() {
            "skills: no archived skills".to_string()
        } else {
            format!("skills: no archived matches for `{query}`")
        }
    } else if query.is_empty() {
        format!("skills: {} archived", rows.len())
    } else {
        format!("skills: {} archived match(es) for `{query}`", rows.len())
    };
    Ok(serde_json::json!({
        "kind": "skills",
        "title": "Archived Skills",
        "message": message,
        "severity": "info",
        "command": raw_command,
        "query": query,
        "archived": true,
        "total": rows.len(),
        "items": rows,
    }))
}

fn handle_command_skill_pin(
    state: &DaemonState,
    raw_command: &str,
    name: &str,
    pinned: bool,
) -> Result<Value, String> {
    let store = peridot_memory::MemoryStore::new(state.project_root.join(".peridot/memory.db"));
    let ts = if pinned {
        crate::run_state::unix_timestamp()
    } else {
        0
    };
    let updated = store.set_skill_pinned(name, ts).map_err(|err| {
        let verb = if pinned { "pin" } else { "unpin" };
        format!("skills: failed to {verb} `{name}`: {err}")
    })?;
    if !updated {
        return Err(format!("skill not found: {name}"));
    }
    let verb = if pinned { "pinned" } else { "unpinned" };
    command_skill_list_result(state, raw_command, Some(format!("{verb} skill `{name}`")))
}

fn handle_command_skill_archive(
    state: &DaemonState,
    raw_command: &str,
    name: &str,
) -> Result<Value, String> {
    let store = peridot_memory::MemoryStore::new(state.project_root.join(".peridot/memory.db"));
    let updated = store
        .set_skill_archived(name, crate::run_state::unix_timestamp())
        .map_err(|err| format!("skills: failed to archive `{name}`: {err}"))?;
    if !updated {
        return Err(format!("skill not found: {name}"));
    }
    move_auto_skill_to_archive(&state.project_root, name)
        .map_err(|err| format!("skills: archived `{name}` but failed to move file: {err}"))?;
    command_skill_list_result(state, raw_command, Some(format!("archived skill `{name}`")))
}

fn handle_command_skill_restore(
    state: &DaemonState,
    raw_command: &str,
    name: &str,
) -> Result<Value, String> {
    let store = peridot_memory::MemoryStore::new(state.project_root.join(".peridot/memory.db"));
    restore_archived_skill(&store, &state.project_root, name)
        .map_err(|err| format!("skills: failed to restore `{name}`: {err}"))?;
    command_skill_list_result(state, raw_command, Some(format!("restored skill `{name}`")))
}

fn command_skill_list_result(
    state: &DaemonState,
    raw_command: &str,
    message: Option<String>,
) -> Result<Value, String> {
    let store = peridot_memory::MemoryStore::new(state.project_root.join(".peridot/memory.db"));
    let mut skills = store
        .list_skills()
        .map_err(|err| format!("skills: failed to read skill store: {err}"))?;
    skills.sort_by(|a, b| a.scope.cmp(&b.scope).then_with(|| a.name.cmp(&b.name)));
    let rows = skill_inventory_rows(&skills);
    let default_message = if rows.is_empty() {
        "skills: <none>".to_string()
    } else {
        format!("skills: {} active", rows.len())
    };
    Ok(serde_json::json!({
        "kind": "skills",
        "title": "Skills",
        "message": message.unwrap_or(default_message),
        "severity": "info",
        "command": raw_command,
        "total": rows.len(),
        "items": rows,
    }))
}

fn skill_inventory_rows(skills: &[peridot_memory::StoredSkill]) -> Vec<Value> {
    skills
        .iter()
        .map(|skill| {
            serde_json::json!({
                "label": format!("/{}", skill.name),
                "detail": skill_description(skill),
                "source": "skill",
                "scope": skill.scope,
                "last_used_at_unix": skill.last_used_at_unix,
                "pinned": skill.pinned_at_unix > 0,
            })
        })
        .collect()
}

fn archived_skill_inventory_rows(records: &[peridot_memory::SkillRecord]) -> Vec<Value> {
    records
        .iter()
        .map(|record| {
            let skill = &record.skill;
            serde_json::json!({
                "label": format!("/{}", skill.name),
                "detail": skill_description(skill),
                "source": "skill",
                "scope": skill.scope,
                "last_used_at_unix": skill.last_used_at_unix,
                "archived_at_unix": skill.archived_at_unix,
                "archived": true,
                "pinned": skill.pinned_at_unix > 0,
            })
        })
        .collect()
}

fn source_label(source: &ContextSource) -> &'static str {
    match source {
        ContextSource::User => "user",
        ContextSource::Assistant => "assistant",
        ContextSource::Tool => "tool",
        ContextSource::PlanReminder => "plan",
        ContextSource::ReviewerComment => "review",
        ContextSource::External => "external",
        ContextSource::SubAgentSummary => "subagent",
    }
}

fn preview_line(content: &str, max_chars: usize) -> String {
    let single = content.replace(['\n', '\r', '\t'], " ");
    let trimmed = single.trim();
    if trimmed.chars().count() <= max_chars {
        trimmed.to_string()
    } else {
        let head: String = trimmed.chars().take(max_chars).collect();
        format!("{head}...")
    }
}

fn handle_command_context_top(
    state: &DaemonState,
    session_id: Option<&str>,
    raw_command: &str,
) -> Result<Value, String> {
    let session_id = require_session_id(session_id, "context top")?;
    let entries = read_context_snapshot(state, &session_id)?;
    if entries.is_empty() {
        return Ok(serde_json::json!({
            "kind": "context_top",
            "title": "Context",
            "message": "context top: <empty>",
            "severity": "info",
            "command": raw_command,
            "items": [],
        }));
    }
    let mut source_totals: BTreeMap<&'static str, usize> = BTreeMap::new();
    let mut rows: Vec<(&ContextEntry, usize)> = entries
        .iter()
        .map(|entry| {
            let tokens = estimate_tokens_for_text(&entry.content);
            *source_totals
                .entry(source_label(&entry.source))
                .or_default() += tokens;
            (entry, tokens)
        })
        .collect();
    rows.sort_by_key(|row| std::cmp::Reverse(row.1));
    let estimated_total: usize = rows.iter().map(|(_, tokens)| *tokens).sum();
    let items: Vec<Value> = rows
        .into_iter()
        .take(10)
        .map(|(entry, tokens)| {
            serde_json::json!({
                "label": source_label(&entry.source),
                "detail": preview_line(&entry.content, 160),
                "tokens": tokens,
                "turn_id": entry.turn_id,
                "untrusted": entry.untrusted,
                "tool_call_id": entry.tool_call_id,
            })
        })
        .collect();
    Ok(serde_json::json!({
        "kind": "context_top",
        "title": "Context",
        "message": format!("context top: {} entries · estimated {} tok", entries.len(), estimated_total),
        "severity": "info",
        "command": raw_command,
        "source_totals": source_totals,
        "items": items,
    }))
}

async fn handle_command_compact(
    state: &DaemonState,
    session_id: Option<&str>,
) -> Result<Value, String> {
    let session_id = require_session_id(session_id, "compact")?;
    let queued = {
        let sessions = state.sessions.lock().await;
        sessions
            .get(&session_id)
            .map(|entry| {
                entry.compact_request.store(true, Ordering::SeqCst);
                true
            })
            .unwrap_or(false)
    };
    if queued {
        Ok(command_result(
            "compact",
            "Compact",
            "compact: flag set - will fire on next turn",
            "info",
        ))
    } else {
        Err(format!("compact: session {session_id} is not running"))
    }
}

fn handle_command_rewind(
    state: &DaemonState,
    session_id: Option<&str>,
    raw_command: &str,
) -> Result<Value, String> {
    let session_id = require_session_id(session_id, "rewind")?;
    let entries = read_context_snapshot(state, &session_id)?;
    let (kept, rewind) = crate::commands::rewind_context_entries(entries)?;
    write_context_snapshot(state, &session_id, &kept)?;
    Ok(serde_json::json!({
        "kind": "rewind",
        "title": "Rewind",
        "message": format!(
            "rewind: restored last prompt and removed {} context entr{}",
            rewind.removed_count,
            if rewind.removed_count == 1 { "y" } else { "ies" }
        ),
        "severity": "info",
        "command": raw_command,
        "session_id": session_id,
        "restored_prompt": rewind.restored_prompt,
        "removed_context_entries": rewind.removed_count,
        "kept_context_entries": rewind.kept_count,
        "rewind_turn_id": rewind.rewind_turn_id,
        "items": [
            { "label": "removed context entries", "detail": rewind.removed_count.to_string() },
            { "label": "kept context entries", "detail": rewind.kept_count.to_string() },
        ],
    }))
}

fn handle_command_diff(state: &DaemonState, raw_command: &str) -> Result<Value, String> {
    let diff = GitManager::new(state.project_root.as_ref().clone())
        .diff()
        .map_err(|err| format!("diff: {err}"))?;
    Ok(serde_json::json!({
        "kind": "diff",
        "title": "Working Tree Diff",
        "message": if diff.trim().is_empty() { "diff: no changes" } else { "diff: working tree changes" },
        "severity": "info",
        "command": raw_command,
        "diff": diff,
    }))
}

fn handle_command_undo(state: &DaemonState, raw_command: &str) -> Result<Value, String> {
    let message = restore_latest_checkpoint(state.project_root.as_ref())
        .map_err(|err| format!("undo: {err}"))?;
    Ok(serde_json::json!({
        "kind": "undo",
        "title": "Undo",
        "message": message,
        "severity": "info",
        "command": raw_command,
    }))
}

fn handle_command_todos(state: &DaemonState, raw_command: &str) -> Result<Value, String> {
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
    let mut hits = Vec::new();
    let mut walked = 0usize;
    walk_for_todos(
        state.project_root.as_ref(),
        state.project_root.as_ref(),
        &mut hits,
        &mut walked,
        SKIP_DIRS,
        MARKERS,
        MAX_HITS,
    );
    let message = if hits.is_empty() {
        format!("todos: no markers found (scanned {walked} file(s))")
    } else {
        format!("todos: {} hit(s) across {walked} file(s)", hits.len())
    };
    Ok(serde_json::json!({
        "kind": "todos",
        "title": "TODOs",
        "message": message,
        "severity": "info",
        "command": raw_command,
        "items": hits,
        "truncated": hits.len() == MAX_HITS,
    }))
}

fn handle_command_codemap(
    state: &DaemonState,
    raw_command: &str,
    refresh: bool,
) -> Result<Value, String> {
    let index = if refresh {
        crate::commands::CodeMapIndexLoad {
            index: crate::commands::refresh_code_map_index(state.project_root.as_ref(), 120, 80)
                .map_err(|err| format!("codemap: failed to load index: {err}"))?,
            refreshed: true,
        }
    } else {
        crate::commands::load_or_refresh_code_map_index_with_status(
            state.project_root.as_ref(),
            120,
            80,
        )
        .map_err(|err| format!("codemap: failed to load index: {err}"))?
    };
    Ok(code_map_result(
        raw_command,
        "Workspace Code Map",
        None,
        &index.index.report,
        index.index.generated_at_unix,
        index.refreshed,
    ))
}

fn handle_command_codemap_status(state: &DaemonState, raw_command: &str) -> Result<Value, String> {
    let status = crate::commands::code_map_status(state.project_root.as_ref())
        .map_err(|err| format!("codemap: failed to check status: {err}"))?;
    Ok(code_map_status_result(raw_command, &status))
}

fn handle_command_codemap_find(
    state: &DaemonState,
    raw_command: &str,
    query: &str,
) -> Result<Value, String> {
    let load = crate::commands::load_or_refresh_code_map_index_with_status(
        state.project_root.as_ref(),
        120,
        80,
    )
    .map_err(|err| format!("codemap: failed to load index: {err}"))?;
    let report = crate::commands::search_code_map_index(&load.index, query);
    Ok(code_map_result(
        raw_command,
        "Workspace Code Map Search",
        Some(query),
        &report,
        load.index.generated_at_unix,
        load.refreshed,
    ))
}

fn handle_command_codemap_locate(
    state: &DaemonState,
    raw_command: &str,
    query: &str,
) -> Result<Value, String> {
    let load = crate::commands::load_or_refresh_code_map_index_with_status(
        state.project_root.as_ref(),
        120,
        80,
    )
    .map_err(|err| format!("codemap: failed to load index: {err}"))?;
    let report = crate::commands::locate_code_map_symbols(&load.index, query);
    Ok(code_map_result(
        raw_command,
        "Workspace Symbol Locations",
        Some(query),
        &report,
        load.index.generated_at_unix,
        load.refreshed,
    ))
}

fn handle_command_codemap_outline(
    state: &DaemonState,
    raw_command: &str,
    path: &str,
) -> Result<Value, String> {
    let load = crate::commands::load_or_refresh_code_map_index_with_status(
        state.project_root.as_ref(),
        120,
        80,
    )
    .map_err(|err| format!("codemap: failed to load index: {err}"))?;
    let report = crate::commands::outline_code_map_file(&load.index, path);
    Ok(code_map_result(
        raw_command,
        "Workspace File Outline",
        Some(path),
        &report,
        load.index.generated_at_unix,
        load.refreshed,
    ))
}

fn handle_command_codemap_refs(
    state: &DaemonState,
    raw_command: &str,
    query: &str,
) -> Result<Value, String> {
    let load = crate::commands::load_or_refresh_code_map_index_with_status(
        state.project_root.as_ref(),
        120,
        80,
    )
    .map_err(|err| format!("codemap: failed to load index: {err}"))?;
    let report = crate::commands::find_code_map_references(
        state.project_root.as_ref(),
        &load.index,
        query,
        80,
    );
    Ok(code_map_result(
        raw_command,
        "Workspace Symbol References",
        Some(query),
        &report,
        load.index.generated_at_unix,
        load.refreshed,
    ))
}

fn code_map_status_result(raw_command: &str, status: &crate::commands::CodeMapStatus) -> Value {
    let state = if !status.index_exists {
        "missing"
    } else if status.stale {
        "stale"
    } else {
        "fresh"
    };
    let generated = status
        .generated_at_unix
        .map(|ts| ts.to_string())
        .unwrap_or_else(|| "none".to_string());
    let newest = status
        .newest_source_mtime_unix
        .map(|ts| ts.to_string())
        .unwrap_or_else(|| "none".to_string());
    let message =
        format!("codemap: index {state} (indexed at {generated}, newest source {newest})");
    serde_json::json!({
        "kind": "codemap_status",
        "title": "Workspace Code Map Status",
        "message": message,
        "severity": if status.stale { "warning" } else { "info" },
        "command": raw_command,
        "index_exists": status.index_exists,
        "stale": status.stale,
        "generated_at_unix": status.generated_at_unix,
        "newest_source_mtime_unix": status.newest_source_mtime_unix,
        "source_files": status.source_files,
        "walked_files": status.walked_files,
        "symbol_count": status.symbol_count,
        "todo_count": status.todo_count,
        "items": [
            { "label": "state", "detail": state },
            { "label": "source files", "detail": status.source_files.to_string() },
            { "label": "indexed files", "detail": status.walked_files.to_string() },
            { "label": "symbols", "detail": status.symbol_count.to_string() },
            { "label": "TODOs", "detail": status.todo_count.to_string() },
        ],
    })
}

fn code_map_result(
    raw_command: &str,
    title: &str,
    query: Option<&str>,
    report: &crate::commands::CodeMapReport,
    generated_at_unix: u64,
    refreshed: bool,
) -> Value {
    let mut items = Vec::new();
    for symbol in report.symbols.iter().take(80) {
        items.push(serde_json::json!({
            "source": "symbol",
            "label": format!("{} {}", symbol.kind, symbol.name),
            "path": symbol.path,
            "line": symbol.line,
            "detail": symbol.signature,
        }));
    }
    for todo in report.todos.iter().take(40) {
        items.push(serde_json::json!({
            "source": "todo",
            "label": todo.marker,
            "path": todo.path,
            "line": todo.line,
            "detail": todo.text,
        }));
    }
    for reference in report.references.iter().take(80) {
        items.push(serde_json::json!({
            "source": "reference",
            "label": reference.symbol,
            "path": reference.path,
            "line": reference.line,
            "detail": reference.text,
        }));
    }
    let truncated = report.symbols_truncated
        || report.todos_truncated
        || report.references_truncated
        || report.symbols.len() > 80
        || report.todos.len() > 40
        || report.references.len() > 80;
    let reference_result = title.contains("References");
    let message = if let Some(query) = query {
        if reference_result {
            format!(
                "codemap: {} reference match(es) for '{}' across {} file(s) (indexed at {})",
                report.references.len(),
                query,
                report.walked_files,
                generated_at_unix,
            )
        } else {
            format!(
                "codemap: {} symbol match(es), {} TODO match(es) for '{}' across {} file(s) (indexed at {})",
                report.symbols.len(),
                report.todos.len(),
                query,
                report.walked_files,
                generated_at_unix,
            )
        }
    } else {
        format!(
            "codemap: {} symbol(s), {} TODO marker(s) across {} file(s) (indexed at {})",
            report.symbols.len(),
            report.todos.len(),
            report.walked_files,
            generated_at_unix,
        )
    };
    let mut result = serde_json::json!({
        "kind": "codemap",
        "title": title,
        "message": message,
        "severity": "info",
        "command": raw_command,
        "items": items,
        "symbol_count": report.symbols.len(),
        "todo_count": report.todos.len(),
        "reference_count": report.references.len(),
        "walked_files": report.walked_files,
        "generated_at_unix": generated_at_unix,
        "refreshed": refreshed,
        "truncated": truncated,
    });
    if let Some(query) = query {
        result["query"] = serde_json::json!(query);
    }
    result
}

fn handle_command_attach(
    state: &DaemonState,
    session_id: Option<&str>,
    raw_command: &str,
    path: &str,
) -> Result<Value, String> {
    const MAX_ATTACHMENT_BYTES: usize = 64 * 1024;
    let session_id = require_session_id(session_id, "attach")?;
    let attachment =
        crate::commands::load_text_attachment(&state.project_root, path, MAX_ATTACHMENT_BYTES)?;
    let inlined = attachment.content.is_some();
    let path = attachment.path.clone();
    let bytes = attachment.bytes;
    let media_type = attachment
        .media_type
        .clone()
        .unwrap_or_else(|| "text/plain".to_string());
    let content = attachment.content.clone();
    let detail = if inlined {
        format!("{bytes} bytes · inlined")
    } else {
        format!("{bytes} bytes · {media_type} placeholder")
    };
    append_plan_reminder_to_context(
        state,
        &session_id,
        crate::commands::attachment_plan_reminder(&attachment),
    )?;
    Ok(serde_json::json!({
        "kind": "attach",
        "title": "Attachment",
        "message": format!("attach: added {path} ({bytes} bytes) to session context"),
        "severity": "info",
        "command": raw_command,
        "attachment": {
            "path": path,
            "bytes": bytes,
            "media_type": media_type,
            "inlined": inlined,
            "content": content,
        },
        "items": [{
            "source": "attachment",
            "label": path,
            "path": path,
            "detail": detail,
            "bytes": bytes,
            "media_type": media_type,
            "inlined": inlined,
        }],
    }))
}

fn handle_command_attachments(
    state: &DaemonState,
    session_id: Option<&str>,
    raw_command: &str,
) -> Result<Value, String> {
    let session_id = require_session_id(session_id, "attachments")?;
    let entries = read_context_snapshot(state, &session_id)?;
    let attachments = crate::commands::attachments_from_context(&entries);
    let items: Vec<Value> = attachments
        .iter()
        .map(|attachment| {
            let mode = if attachment.inlined {
                "inlined"
            } else {
                "placeholder"
            };
            serde_json::json!({
                "source": "attachment",
                "label": attachment.path,
                "path": attachment.path,
                "detail": format!("{} bytes · {} · {}", attachment.bytes, attachment.media_type, mode),
                "bytes": attachment.bytes,
                "media_type": attachment.media_type,
                "inlined": attachment.inlined,
            })
        })
        .collect();
    Ok(serde_json::json!({
        "kind": "attachments",
        "title": "Session Attachments",
        "message": format!("attachments: {} file(s) in session context", attachments.len()),
        "severity": "info",
        "command": raw_command,
        "attachments": attachments,
        "items": items,
        "total": items.len(),
    }))
}

fn handle_command_detach(
    state: &DaemonState,
    session_id: Option<&str>,
    raw_command: &str,
    path: &str,
) -> Result<Value, String> {
    let session_id = require_session_id(session_id, "detach")?;
    let entries = read_context_snapshot(state, &session_id)?;
    let (kept, removed) = crate::commands::detach_attachments_from_context(entries, path);
    if removed.is_empty() {
        return Ok(serde_json::json!({
            "kind": "detach",
            "title": "Detach Attachment",
            "message": format!("detach: no attachment matched {path}"),
            "severity": "info",
            "command": raw_command,
            "removed_count": 0,
            "items": [],
        }));
    }
    write_context_snapshot(state, &session_id, &kept)?;
    let remaining = crate::commands::attachments_from_context(&kept);
    let items: Vec<Value> = removed
        .iter()
        .map(|attachment| {
            serde_json::json!({
                "source": "attachment",
                "label": attachment.path,
                "path": attachment.path,
                "detail": format!("{} bytes · removed", attachment.bytes),
                "bytes": attachment.bytes,
                "media_type": attachment.media_type,
                "inlined": attachment.inlined,
            })
        })
        .collect();
    Ok(serde_json::json!({
        "kind": "detach",
        "title": "Detach Attachment",
        "message": format!("detach: removed {} attachment(s) matching {path}", removed.len()),
        "severity": "info",
        "command": raw_command,
        "removed_count": removed.len(),
        "remaining_count": remaining.len(),
        "removed": removed,
        "attachments": remaining,
        "items": items,
    }))
}

fn handle_command_export(
    state: &DaemonState,
    session_id: Option<&str>,
    raw_command: &str,
    artifacts: &[ExportArtifact],
) -> Result<Value, String> {
    let session_id = require_session_id(session_id, "export")?;
    let selected = map_export_artifacts(artifacts);
    let out_dir = default_session_export_dir(&state.project_root, &session_id);
    let report = crate::commands::export_session_artifacts(
        &state.project_root,
        &session_id,
        &out_dir,
        &selected,
        false,
    )
    .map_err(|err| err.to_string())?;
    let items: Vec<Value> = report
        .artifacts
        .iter()
        .map(|artifact| {
            serde_json::json!({
                "source": "artifact",
                "label": artifact.path,
                "detail": format!("{} entries · {}", artifact.count, artifact.class),
            })
        })
        .collect();
    Ok(serde_json::json!({
        "kind": "session_export",
        "title": "Session Artifact Export",
        "message": format!("export: wrote {} artifact file(s) to {}", report.artifacts.len(), report.destination),
        "severity": "info",
        "command": raw_command,
        "id": report.id,
        "source": report.source,
        "destination": report.destination,
        "artifact_classes": report.artifact_classes,
        "files": report.files,
        "artifacts": report.artifacts,
        "items": items,
        "total": items.len(),
    }))
}

fn default_session_export_dir(project_root: &Path, session_id: &str) -> PathBuf {
    project_root.join(".peridot").join("exports").join(format!(
        "{}-{}",
        sanitize_export_segment(session_id),
        current_unix_secs()
    ))
}

fn sanitize_export_segment(value: &str) -> String {
    let sanitized: String = value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.') {
                ch
            } else {
                '-'
            }
        })
        .collect();
    let trimmed = sanitized.trim_matches('-');
    if trimmed.is_empty() {
        "session".to_string()
    } else {
        trimmed.to_string()
    }
}

fn current_unix_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or_default()
}

fn map_export_artifacts(
    artifacts: &[ExportArtifact],
) -> Vec<crate::commands::SessionExportArtifact> {
    artifacts
        .iter()
        .map(|artifact| match artifact {
            ExportArtifact::Full => crate::commands::SessionExportArtifact::Full,
            ExportArtifact::Attachments => crate::commands::SessionExportArtifact::Attachments,
            ExportArtifact::Notes => crate::commands::SessionExportArtifact::Notes,
            ExportArtifact::Timeline => crate::commands::SessionExportArtifact::Timeline,
        })
        .collect()
}

#[allow(clippy::too_many_arguments)]
fn walk_for_todos(
    root: &Path,
    dir: &Path,
    hits: &mut Vec<Value>,
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
            if skip_dirs.iter().any(|s| *s == name_str) || name_str.starts_with('.') {
                continue;
            }
            walk_for_todos(root, &path, hits, walked, skip_dirs, markers, cap);
            continue;
        }
        if !file_type.is_file() || name_str.starts_with('.') {
            continue;
        }
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
            if let Some(marker) = markers.iter().find(|m| line.contains(**m)) {
                hits.push(serde_json::json!({
                    "label": marker,
                    "path": rel,
                    "line": idx + 1,
                    "detail": preview_line(line.trim(), 240),
                }));
            }
        }
    }
}

fn config_path(state: &DaemonState) -> PathBuf {
    state.project_root.join(".peridot/config.toml")
}

fn handle_command_mcp_list(state: &DaemonState, raw_command: &str) -> Result<Value, String> {
    let path = config_path(state);
    let config = read_project_config(&path)?;
    let items: Vec<Value> = config
        .mcp
        .iter()
        .map(|entry| {
            let detail = match entry.transport {
                McpTransport::Stdio => {
                    let args = if entry.args.is_empty() {
                        String::new()
                    } else {
                        format!(" {}", entry.args.join(" "))
                    };
                    format!("{}{}", entry.command.clone().unwrap_or_default(), args)
                }
                McpTransport::Http => entry.url.clone().unwrap_or_default(),
            };
            serde_json::json!({
                "label": entry.name,
                "detail": detail,
                "transport": entry.transport.to_string(),
            })
        })
        .collect();
    Ok(serde_json::json!({
        "kind": "mcp",
        "title": "MCP Servers",
        "message": if items.is_empty() { "mcp: <none configured>".to_string() } else { format!("mcp: {} server(s)", items.len()) },
        "severity": "info",
        "command": raw_command,
        "items": items,
    }))
}

fn handle_command_mcp_add(
    state: &DaemonState,
    raw_command: &str,
    name: &str,
    transport: &str,
    target: &str,
) -> Result<Value, String> {
    let path = config_path(state);
    let existing = read_project_config(&path)?;
    if existing.mcp.iter().any(|m| m.name == name) {
        return Err(format!(
            "mcp add: '{name}' already configured - use /mcp remove first"
        ));
    }
    let block = match transport.to_ascii_lowercase().as_str() {
        "stdio" => {
            let mut parts = target.split_whitespace();
            let Some(command) = parts.next() else {
                return Err("mcp add: stdio transport requires a command".to_string());
            };
            let args: Vec<&str> = parts.collect();
            let args_toml = if args.is_empty() {
                String::new()
            } else {
                let quoted = args
                    .iter()
                    .map(|a| format!("\"{}\"", escape_toml_string(a)))
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
            return Err(format!(
                "mcp add: unknown transport '{other}' (use stdio or http)"
            ));
        }
    };
    let existing_content = match std::fs::read_to_string(&path) {
        Ok(content) => content,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(err) => return Err(format!("mcp add: read {}: {err}", path.display())),
    };
    let new_content = if existing_content.is_empty() {
        block.trim_start_matches('\n').to_string()
    } else if existing_content.ends_with('\n') {
        format!("{existing_content}{block}")
    } else {
        format!("{existing_content}\n{block}")
    };
    atomic_write(&path, &new_content)?;
    Ok(serde_json::json!({
        "kind": "mcp",
        "title": "MCP",
        "message": format!("mcp: added '{name}' to {}. Restart this session for it to take effect.", path.display()),
        "severity": "info",
        "command": raw_command,
    }))
}

fn handle_command_mcp_remove(
    state: &DaemonState,
    raw_command: &str,
    name: &str,
) -> Result<Value, String> {
    let path = config_path(state);
    let content = std::fs::read_to_string(&path)
        .map_err(|err| format!("mcp remove: read {}: {err}", path.display()))?;
    let Some(new_content) = remove_mcp_block(&content, name) else {
        return Err(format!("mcp remove: no server named '{name}'"));
    };
    atomic_write(&path, &new_content)?;
    Ok(serde_json::json!({
        "kind": "mcp",
        "title": "MCP",
        "message": format!("mcp: removed '{name}' from {}. Restart this session to drop its tools from the registry.", path.display()),
        "severity": "info",
        "command": raw_command,
    }))
}

async fn handle_command_mcp_test(
    state: &DaemonState,
    raw_command: &str,
    name: &str,
) -> Result<Value, String> {
    let path = config_path(state);
    let config = read_project_config(&path)?;
    let Some(entry) = config.mcp.iter().find(|m| m.name == name).cloned() else {
        return Err(format!("mcp test: no server named '{name}'"));
    };
    let client = peridot_mcp::McpClient::new(entry);
    let count = client
        .list_tools()
        .await
        .map_err(|err| format!("mcp test '{name}': {err}"))?
        .len();
    Ok(serde_json::json!({
        "kind": "mcp",
        "title": "MCP",
        "message": format!("mcp: '{name}' reachable - {count} tool(s) exposed"),
        "severity": "info",
        "command": raw_command,
    }))
}

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

fn handle_command_branch_save(
    state: &DaemonState,
    session_id: Option<&str>,
    raw_command: &str,
    name: &str,
) -> Result<Value, String> {
    validate_branch_name(name)?;
    let session_id = require_session_id(session_id, "branch save")?;
    let src = context_snapshot_path(state, &session_id);
    if !src.exists() {
        return Err(format!(
            "branch save: no context.bin yet for session {session_id} - submit at least one turn first"
        ));
    }
    let dst_dir = state.project_root.join(".peridot/branches").join(name);
    if dst_dir.exists() {
        return Err(format!(
            "branch save: '{name}' already exists - remove it manually first"
        ));
    }
    std::fs::create_dir_all(&dst_dir)
        .map_err(|err| format!("branch save: create {}: {err}", dst_dir.display()))?;
    let dst = dst_dir.join("context.bin");
    std::fs::copy(&src, &dst).map_err(|err| format!("branch save: copy: {err}"))?;
    Ok(serde_json::json!({
        "kind": "branch",
        "title": "Branch",
        "message": format!("branch: saved '{name}' from session {session_id}"),
        "severity": "info",
        "command": raw_command,
    }))
}

fn handle_command_branch_restore(
    state: &DaemonState,
    session_id: Option<&str>,
    raw_command: &str,
    name: &str,
) -> Result<Value, String> {
    validate_branch_name(name)?;
    let session_id = require_session_id(session_id, "branch restore")?;
    let src = state
        .project_root
        .join(".peridot/branches")
        .join(name)
        .join("context.bin");
    if !src.exists() {
        return Err(format!("branch restore: no branch named '{name}'"));
    }
    let session_dir = state
        .project_root
        .join(".peridot/sessions")
        .join(&session_id);
    std::fs::create_dir_all(&session_dir)
        .map_err(|err| format!("branch restore: create {}: {err}", session_dir.display()))?;
    let dst = session_dir.join("context.bin");
    std::fs::copy(&src, &dst).map_err(|err| format!("branch restore: copy: {err}"))?;
    Ok(serde_json::json!({
        "kind": "branch",
        "title": "Branch",
        "message": format!("branch: restored '{name}' into session {session_id}. Submit your next task to continue from that point."),
        "severity": "info",
        "command": raw_command,
    }))
}

fn handle_command_branch_list(state: &DaemonState, raw_command: &str) -> Result<Value, String> {
    let dir = state.project_root.join(".peridot/branches");
    let mut rows: Vec<Value> = Vec::new();
    if let Ok(entries) = std::fs::read_dir(&dir) {
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
            rows.push(serde_json::json!({ "label": name, "detail": format!("unix {stamp}") }));
        }
    }
    rows.sort_by(|a, b| {
        a.get("label")
            .and_then(Value::as_str)
            .cmp(&b.get("label").and_then(Value::as_str))
    });
    Ok(serde_json::json!({
        "kind": "branch",
        "title": "Branches",
        "message": if rows.is_empty() { "branches: <none>".to_string() } else { format!("branches: {} saved", rows.len()) },
        "severity": "info",
        "command": raw_command,
        "items": rows,
    }))
}

fn handle_command_branch_picker(
    state: &DaemonState,
    session_id: Option<&str>,
    raw_command: &str,
) -> Result<Value, String> {
    let session_id = require_session_id(session_id, "branch picker")?;
    let entries = read_context_snapshot(state, &session_id)?;
    let mut seen: BTreeMap<u64, Value> = BTreeMap::new();
    for entry in entries {
        seen.entry(entry.turn_id).or_insert_with(|| {
            serde_json::json!({
                "label": format!("turn {}", entry.turn_id),
                "detail": preview_line(&entry.content, 100),
                "turn_id": entry.turn_id,
                "source": source_label(&entry.source),
            })
        });
    }
    let items: Vec<Value> = seen.into_values().collect();
    Ok(serde_json::json!({
        "kind": "branch_picker",
        "title": "Branch Turns",
        "message": if items.is_empty() { "branch picker: no turns".to_string() } else { format!("branch picker: {} turn(s)", items.len()) },
        "severity": "info",
        "command": raw_command,
        "items": items,
    }))
}

fn handle_command_branch_turn(
    state: &DaemonState,
    session_id: Option<&str>,
    raw_command: &str,
    turn_id: u64,
) -> Result<Value, String> {
    let session_id = require_session_id(session_id, "branch turn")?;
    let snapshot_path = context_snapshot_path(state, &session_id);
    let entries = read_context_snapshot(state, &session_id)?;
    let Some(last_keep) = entries.iter().rposition(|entry| entry.turn_id <= turn_id) else {
        return Err(format!(
            "branch turn: turn id {turn_id} not found in snapshot"
        ));
    };
    let kept = &entries[..=last_keep];
    let dropped_entries: Vec<ContextEntry> = entries[last_keep + 1..].to_vec();
    let dropped_count = dropped_entries.len();
    if !dropped_entries.is_empty() {
        let journal_path = branch_journal_path(state, &session_id);
        let mut journal = BranchJournal::load(&journal_path);
        journal.record(turn_id, dropped_entries);
        journal
            .save(&journal_path)
            .map_err(|err| format!("branch turn: journal write error - {err}"))?;
    }
    let serialized =
        serde_json::to_vec(kept).map_err(|err| format!("branch turn: serialise error - {err}"))?;
    std::fs::write(&snapshot_path, &serialized)
        .map_err(|err| format!("branch turn: write error - {err}"))?;
    Ok(serde_json::json!({
        "kind": "branch",
        "title": "Branch",
        "message": format!("branch turn: forked at turn {turn_id} ({dropped_count} entries saved to journal)"),
        "severity": "info",
        "command": raw_command,
    }))
}

fn handle_command_branch_tree(
    state: &DaemonState,
    session_id: Option<&str>,
    raw_command: &str,
) -> Result<Value, String> {
    let session_id = require_session_id(session_id, "branch tree")?;
    let journal = BranchJournal::load(&branch_journal_path(state, &session_id));
    let items: Vec<Value> = journal
        .tree_summary()
        .into_iter()
        .map(|line| serde_json::json!({ "label": line }))
        .collect();
    Ok(serde_json::json!({
        "kind": "branch",
        "title": "Branch Tree",
        "message": if journal.limbs.is_empty() { "branch tree: no abandoned limbs yet - fork with `/branch turn <id>` first".to_string() } else { format!("branch tree: {} limb(s)", journal.limbs.len()) },
        "severity": "info",
        "command": raw_command,
        "items": items,
    }))
}

fn handle_command_branch_switch(
    state: &DaemonState,
    session_id: Option<&str>,
    raw_command: &str,
    index: usize,
) -> Result<Value, String> {
    let session_id = require_session_id(session_id, "branch switch")?;
    let snapshot_path = context_snapshot_path(state, &session_id);
    if !snapshot_path.exists() {
        return Err("branch switch: no context snapshot".to_string());
    }
    let journal_path = branch_journal_path(state, &session_id);
    let mut journal = BranchJournal::load(&journal_path);
    let Some(limb) = journal.take_limb(index) else {
        return Err(format!(
            "branch switch: limb [{index}] not found (have {} limbs)",
            journal.limbs.len()
        ));
    };
    let bytes = std::fs::read(&snapshot_path)
        .map_err(|err| format!("branch switch: read snapshot - {err}"))?;
    let current_entries: Vec<ContextEntry> = serde_json::from_slice(&bytes)
        .map_err(|err| format!("branch switch: parse snapshot - {err}"))?;
    let fork_turn = limb.parent_turn_id;
    let Some(last_keep) = current_entries
        .iter()
        .rposition(|entry| entry.turn_id <= fork_turn)
    else {
        journal.limbs.insert(index, limb);
        return Err(format!(
            "branch switch: fork point turn {fork_turn} not in current snapshot"
        ));
    };
    let current_tail: Vec<ContextEntry> = current_entries[last_keep + 1..].to_vec();
    if !current_tail.is_empty() {
        journal.record(fork_turn, current_tail);
    }
    let mut new_entries = current_entries[..=last_keep].to_vec();
    new_entries.extend(limb.entries);
    let serialized = serde_json::to_vec(&new_entries)
        .map_err(|err| format!("branch switch: serialise - {err}"))?;
    std::fs::write(&snapshot_path, &serialized)
        .map_err(|err| format!("branch switch: write - {err}"))?;
    journal
        .save(&journal_path)
        .map_err(|err| format!("branch switch: journal write - {err}"))?;
    Ok(serde_json::json!({
        "kind": "branch",
        "title": "Branch",
        "message": format!("branch switch: swapped to limb [{index}] (fork@turn {fork_turn}). Submit your next task to continue."),
        "severity": "info",
        "command": raw_command,
    }))
}

fn read_project_config(path: &Path) -> Result<PeridotConfig, String> {
    match std::fs::read_to_string(path) {
        Ok(content) => toml::from_str::<PeridotConfig>(&content)
            .map_err(|err| format!("failed to parse {}: {err}", path.display())),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(PeridotConfig::default()),
        Err(err) => Err(format!("failed to read {}: {err}", path.display())),
    }
}

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
        } else if (trimmed.starts_with("[[") || trimmed.starts_with('['))
            && let Some(start) = current_start.take()
        {
            blocks.push((start, idx, current_name.take()));
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

fn escape_toml_string(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}

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

fn parse_ask_user_answer(value: &Value) -> Result<AskUserAnswer, String> {
    let Value::Object(map) = value else {
        return Err("answer must be an object".to_string());
    };
    let kind = map
        .get("kind")
        .and_then(Value::as_str)
        .ok_or_else(|| "answer.kind must be a string".to_string())?;
    match kind {
        "cancelled" => Ok(AskUserAnswer::Cancelled),
        "selected" => {
            let index = map
                .get("index")
                .and_then(Value::as_u64)
                .ok_or_else(|| "selected answer requires numeric index".to_string())?
                as usize;
            let text = map
                .get("text")
                .and_then(Value::as_str)
                .ok_or_else(|| "selected answer requires text".to_string())?
                .to_string();
            Ok(AskUserAnswer::Selected { index, text })
        }
        "multi_selected" => {
            let indices = map
                .get("indices")
                .and_then(Value::as_array)
                .ok_or_else(|| "multi_selected answer requires indices".to_string())?
                .iter()
                .map(|value| {
                    value
                        .as_u64()
                        .map(|index| index as usize)
                        .ok_or_else(|| "multi_selected indices must be numbers".to_string())
                })
                .collect::<Result<Vec<_>, _>>()?;
            Ok(AskUserAnswer::MultiSelected { indices })
        }
        "text" => {
            let text = map
                .get("text")
                .or_else(|| map.get("value"))
                .or_else(|| map.get("0"))
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            Ok(AskUserAnswer::Text(text))
        }
        other => Err(format!("unknown answer.kind: {other}")),
    }
}

fn optional_str<'a>(params: &'a serde_json::Map<String, Value>, key: &str) -> Option<&'a str> {
    params
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

fn parse_execution_mode(value: &str) -> Option<ExecutionMode> {
    match value.trim().to_ascii_lowercase().as_str() {
        "plan" => Some(ExecutionMode::Plan),
        "execute" => Some(ExecutionMode::Execute),
        "goal" => Some(ExecutionMode::Goal),
        _ => None,
    }
}

fn parse_permission_mode(value: &str) -> Option<PermissionMode> {
    match value.trim().to_ascii_lowercase().as_str() {
        "safe" => Some(PermissionMode::Safe),
        "auto" => Some(PermissionMode::Auto),
        "yolo" => Some(PermissionMode::Yolo),
        _ => None,
    }
}

fn parse_service_tier(value: &str) -> Option<Option<String>> {
    match value.trim().to_ascii_lowercase().as_str() {
        "fast" | "priority" => Some(Some("fast".to_string())),
        "standard" | "default" | "off" | "none" | "false" => Some(None),
        _ => None,
    }
}

fn emit_response(state: &DaemonState, id: Value, result: Value) -> Result<()> {
    let envelope = RpcResponse {
        jsonrpc: "2.0",
        id,
        result,
    };
    emit_json(state, &envelope)
}

fn emit_error(state: &DaemonState, id: Value, code: i32, message: String) -> Result<()> {
    let envelope = RpcErrorResponse {
        jsonrpc: "2.0",
        id,
        error: RpcError { code, message },
    };
    emit_json(state, &envelope)
}

/// Emit a one-shot `peridot.handshake` notification before any other daemon
/// output. Carries the wire-format version of [`AgentRunEvent`] plus the
/// daemon's own crate version so an editor extension can detect skew (e.g.,
/// a stale extension talking to a fresher daemon that added an event variant).
///
/// JSON-RPC notification, not a request — there's no `id`. Clients that don't
/// know about handshakes can ignore it without error; clients that do know
/// can refuse to talk if the schema version differs from what they expect.
fn emit_handshake(state: &DaemonState) -> Result<()> {
    let envelope = serde_json::json!({
        "jsonrpc": "2.0",
        "method": "peridot.handshake",
        "params": {
            "schema_version": peridot_core::AGENT_RUN_EVENT_SCHEMA_VERSION,
            "daemon_version": env!("CARGO_PKG_VERSION"),
        },
    });
    emit_json(state, &envelope)
}

fn emit_event(state: &DaemonState, session_id: &str, event: Value) {
    let envelope = serde_json::json!({
        "jsonrpc": "2.0",
        "method": "event",
        "params": {
            "session_id": session_id,
            "event": event,
        },
    });
    if let Ok(line) = serde_json::to_string(&envelope) {
        let _ = state.out.send(line);
    }
}

fn emit_notification(state: &DaemonState, method: &str, params: Value) -> Result<()> {
    let envelope = serde_json::json!({
        "jsonrpc": "2.0",
        "method": method,
        "params": params,
    });
    emit_json(state, &envelope)
}

fn emit_json<T: Serialize>(state: &DaemonState, value: &T) -> Result<()> {
    let line = serde_json::to_string(value)?;
    let _ = state.out.send(line);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_options(mock_response_file: Option<PathBuf>) -> AgentTaskOptions {
        AgentTaskOptions {
            permission: PermissionMode::Auto,
            model: "mock".to_string(),
            reasoning_effort: peridot_common::ReasoningEffort::Off,
            service_tier: None,
            max_turns: 2,
            budget_usd: 1.0,
            resume: None,
            mock_response_file,
            live: false,
        }
    }

    fn test_project(name: &str) -> PathBuf {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "peridot-daemon-test-{name}-{}-{unique}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        root
    }

    async fn dispatch_and_collect(line: &str) -> Vec<Value> {
        dispatch_and_collect_with_options(line, test_options(None)).await
    }

    async fn dispatch_and_collect_with_options(
        line: &str,
        options: AgentTaskOptions,
    ) -> Vec<Value> {
        let (tx, mut rx) = mpsc::unbounded_channel::<String>();
        let root = test_project("dispatch");
        let state = DaemonState::new(root.clone(), PeridotConfig::default(), options, tx);
        let _ = dispatch_line(&state, line).await.unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        let mut values = Vec::new();
        while let Ok(line) = rx.try_recv() {
            values.push(serde_json::from_str(&line).unwrap());
        }
        shutdown_sessions(&state).await;
        let _ = std::fs::remove_dir_all(root);
        values
    }

    #[tokio::test]
    async fn version_method_returns_cargo_pkg_version() {
        let out =
            dispatch_and_collect(r#"{"jsonrpc":"2.0","id":1,"method":"peridot.version"}"#).await;
        assert_eq!(out[0]["jsonrpc"], "2.0");
        assert_eq!(out[0]["id"], 1);
        assert_eq!(out[0]["result"]["version"], env!("CARGO_PKG_VERSION"));
    }

    #[tokio::test]
    async fn status_method_returns_project_context() {
        let out =
            dispatch_and_collect(r#"{"jsonrpc":"2.0","id":9,"method":"peridot.status"}"#).await;
        assert_eq!(out[0]["jsonrpc"], "2.0");
        assert_eq!(out[0]["id"], 9);
        assert_eq!(out[0]["result"]["version"], env!("CARGO_PKG_VERSION"));
        assert_eq!(out[0]["result"]["provider"], "claude-api");
        assert_eq!(out[0]["result"]["model"], "claude-sonnet-4-6");
        assert!(out[0]["result"]["project_root"].as_str().is_some());
        assert_eq!(out[0]["result"]["auth"]["provider"], "claude-api");
        assert_eq!(out[0]["result"]["auth"]["method"], "api_key");
        assert!(out[0]["result"]["worktree_cleanup"].is_object());
    }

    #[tokio::test]
    async fn status_reconciles_stale_worktree_records() {
        let root = test_project("status-worktree-cleanup");
        let store = MemoryStore::new(root.join(".peridot/memory.db"));
        let mut record = SessionRecord::new("stale-worktree", root.join(".peridot/worktrees/wt"));
        record.status = SessionLifecycle::Running;
        record.worktree_branch = Some("peridot/stale-worktree".to_string());
        store.save_session_record(&record).unwrap();
        let (tx, mut rx) = mpsc::unbounded_channel::<String>();
        let state = DaemonState::new(
            root.clone(),
            PeridotConfig::default(),
            test_options(None),
            tx,
        );

        dispatch_line(
            &state,
            r#"{"jsonrpc":"2.0","id":91,"method":"peridot.status"}"#,
        )
        .await
        .unwrap();

        let line = rx.try_recv().unwrap();
        let value: Value = serde_json::from_str(&line).unwrap();
        let cleanup = &value["result"]["worktree_cleanup"];
        assert_eq!(cleanup["suspended_sessions"][0], "stale-worktree");
        assert_eq!(
            cleanup["missing_worktrees"][0]["session_id"],
            "stale-worktree"
        );
        let updated = store.get_session_record("stale-worktree").unwrap().unwrap();
        assert_eq!(updated.status, SessionLifecycle::Suspended);
        assert_eq!(updated.worktree_branch, None);
        assert_eq!(updated.workspace_root, root);
    }

    #[tokio::test]
    async fn command_catalog_method_returns_tui_catalog() {
        let out =
            dispatch_and_collect(r#"{"jsonrpc":"2.0","id":10,"method":"session.command_catalog"}"#)
                .await;
        assert_eq!(out[0]["jsonrpc"], "2.0");
        assert_eq!(out[0]["id"], 10);
        let commands = out[0]["result"]["commands"].as_array().unwrap();
        let catalog = peridot_tui::slash_command_catalog();
        assert_eq!(commands.len(), catalog.len());
        for (actual, expected) in commands.iter().zip(catalog.iter()) {
            assert_eq!(actual["name"], expected.name);
            assert_eq!(actual["description"], expected.description);
            assert_eq!(actual["category"], expected.category);
            let surfaces: Vec<&str> = actual["surfaces"]
                .as_array()
                .unwrap()
                .iter()
                .map(|value| value.as_str().unwrap())
                .collect();
            assert_eq!(surfaces, peridot_tui::slash_command_surfaces(expected));
            assert_eq!(
                actual["arg_hint"].as_str().unwrap_or(""),
                expected.arg_hint.unwrap_or("")
            );
            let arg_options: Vec<&str> = actual["arg_options"]
                .as_array()
                .unwrap()
                .iter()
                .map(|value| value.as_str().unwrap())
                .collect();
            assert_eq!(
                arg_options,
                peridot_tui::slash_command_arg_options(expected)
            );
        }
        assert!(commands.iter().any(|entry| entry["name"] == "/plan"));
        assert!(commands.iter().any(|entry| {
            entry["name"] == "/collapse" && entry["surfaces"] == serde_json::json!(["tui"])
        }));
        assert!(commands.iter().any(|entry| {
            entry["name"] == "/sidepanel" && entry["surfaces"] == serde_json::json!(["tui"])
        }));
        assert!(commands.iter().any(|entry| {
            entry["name"] == "/reasoning"
                && entry["arg_options"]
                    == serde_json::json!(["off", "low", "medium", "high", "xhigh"])
        }));
        assert!(
            commands
                .iter()
                .any(|entry| entry["name"] == "/branch switch")
        );
        assert!(
            commands
                .iter()
                .all(|entry| entry["description"].as_str().is_some())
        );
    }

    #[tokio::test]
    async fn command_catalog_method_filters_by_surface() {
        let out = dispatch_and_collect(
            r#"{"jsonrpc":"2.0","id":10,"method":"session.command_catalog","params":{"surface":"vscode"}}"#,
        )
        .await;
        assert_eq!(out[0]["jsonrpc"], "2.0");
        let commands = out[0]["result"]["commands"].as_array().unwrap();
        assert!(commands.iter().any(|entry| entry["name"] == "/plan"));
        assert!(!commands.iter().any(|entry| entry["name"] == "/collapse"));
        assert!(!commands.iter().any(|entry| entry["name"] == "/sidepanel"));
        assert!(!commands.iter().any(|entry| entry["name"] == "/lang"));
        assert!(commands.iter().all(|entry| {
            entry["surfaces"]
                .as_array()
                .unwrap()
                .iter()
                .any(|surface| surface == "vscode")
        }));
    }

    #[tokio::test]
    async fn session_command_help_returns_surface_filtered_catalog_rows() {
        let out = dispatch_and_collect(
            r#"{"jsonrpc":"2.0","id":11,"method":"session.command","params":{"command":"/help","surface":"vscode"}}"#,
        )
        .await;
        assert_eq!(out[0]["jsonrpc"], "2.0");
        let result = &out[0]["result"];
        assert_eq!(result["kind"], "help");
        assert_eq!(result["surface"], "vscode");
        let items = result["items"].as_array().unwrap();
        assert!(items.iter().any(|entry| entry["label"] == "/plan"));
        assert!(!items.iter().any(|entry| entry["label"] == "/collapse"));
        assert!(!items.iter().any(|entry| entry["label"] == "/sidepanel"));
        assert!(!items.iter().any(|entry| entry["label"] == "/lang <en|ko>"));
        assert_eq!(result["total"].as_u64().unwrap(), items.len() as u64);
    }

    #[tokio::test]
    async fn skills_list_returns_active_auto_skills() {
        let root = test_project("skills-list");
        let store = peridot_memory::MemoryStore::new(root.join(".peridot/memory.db"));
        store
            .save_skill(&peridot_memory::StoredSkill {
                name: "auto-fix-parser".into(),
                body: "repair parser tests".into(),
                description: "repair parser tests".into(),
                scope: "auto".into(),
                ..Default::default()
            })
            .unwrap();
        store
            .save_skill(&peridot_memory::StoredSkill {
                name: "community-skill".into(),
                body: "community".into(),
                scope: "community".into(),
                ..Default::default()
            })
            .unwrap();
        store
            .save_skill(&peridot_memory::StoredSkill {
                name: "archived-auto".into(),
                body: "old".into(),
                scope: "auto".into(),
                archived_at_unix: 1,
                ..Default::default()
            })
            .unwrap();
        let (tx, mut rx) = mpsc::unbounded_channel::<String>();
        let state = DaemonState::new(
            root.clone(),
            PeridotConfig::default(),
            test_options(None),
            tx,
        );

        let _ = dispatch_line(
            &state,
            r#"{"jsonrpc":"2.0","id":42,"method":"skills.list"}"#,
        )
        .await
        .unwrap();

        let line = rx.try_recv().unwrap();
        let value: Value = serde_json::from_str(&line).unwrap();
        let skills = value["result"]["skills"].as_array().unwrap();
        assert_eq!(skills.len(), 1);
        assert_eq!(skills[0]["name"], "auto-fix-parser");
        assert_eq!(skills[0]["description"], "repair parser tests");
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn session_command_skills_returns_active_skill_inventory() {
        let root = test_project("command-skills");
        let store = peridot_memory::MemoryStore::new(root.join(".peridot/memory.db"));
        store
            .save_skill(&peridot_memory::StoredSkill {
                name: "auto-fix-parser".into(),
                body: "repair parser tests".into(),
                description: "repair parser tests".into(),
                scope: "auto".into(),
                last_used_at_unix: 123,
                ..Default::default()
            })
            .unwrap();
        store
            .save_skill(&peridot_memory::StoredSkill {
                name: "review-flow".into(),
                body: "review checklist".into(),
                scope: "community".into(),
                pinned_at_unix: 456,
                ..Default::default()
            })
            .unwrap();
        let (tx, mut rx) = mpsc::unbounded_channel::<String>();
        let state = DaemonState::new(
            root.clone(),
            PeridotConfig::default(),
            test_options(None),
            tx,
        );

        let _ = dispatch_line(
            &state,
            r#"{"jsonrpc":"2.0","id":43,"method":"session.command","params":{"command":"/skills"}}"#,
        )
        .await
        .unwrap();

        let line = rx.try_recv().unwrap();
        let value: Value = serde_json::from_str(&line).unwrap();
        assert_eq!(value["result"]["kind"], "skills");
        assert_eq!(value["result"]["total"], 2);
        let items = value["result"]["items"].as_array().unwrap();
        assert!(items.iter().any(|item| {
            item["label"] == "/auto-fix-parser"
                && item["detail"] == "repair parser tests"
                && item["scope"] == "auto"
                && item["last_used_at_unix"] == 123
        }));
        assert!(items.iter().any(|item| {
            item["label"] == "/review-flow"
                && item["scope"] == "community"
                && item["pinned"] == true
        }));
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn session_command_note_persists_and_lists_notes() {
        let root = test_project("command-notes");
        let (tx, _rx) = mpsc::unbounded_channel::<String>();
        let state = DaemonState::new(
            root.clone(),
            PeridotConfig::default(),
            test_options(None),
            tx,
        );

        let value = execute_session_command(
            &state,
            Some("note-session"),
            "/note first checkpoint",
            SlashCommand::Note("first checkpoint".to_string()),
        )
        .await
        .unwrap();
        assert_eq!(value["kind"], "note");
        assert_eq!(value["session_id"], "note-session");
        assert_eq!(value["note"]["text"], "first checkpoint");
        assert!(
            root.join(".peridot/sessions/note-session/notes.ndjson")
                .is_file()
        );

        execute_session_command(
            &state,
            Some("note-session"),
            "/note second checkpoint",
            SlashCommand::Note("second checkpoint".to_string()),
        )
        .await
        .unwrap();

        let value = execute_session_command(
            &state,
            Some("note-session"),
            "/notes last 1",
            SlashCommand::Notes(Some(1)),
        )
        .await
        .unwrap();
        assert_eq!(value["kind"], "notes");
        assert_eq!(value["total"], 2);
        assert_eq!(value["items"].as_array().unwrap().len(), 1);
        assert_eq!(value["items"][0]["text"], "second checkpoint");
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn session_command_skills_show_returns_skill_detail() {
        let root = test_project("command-skills-show");
        let store = peridot_memory::MemoryStore::new(root.join(".peridot/memory.db"));
        store
            .save_skill(&peridot_memory::StoredSkill {
                name: "auto-fix-parser".into(),
                body: "repair parser tests\nrun cargo test".into(),
                description: "repair parser tests".into(),
                scope: "auto".into(),
                last_used_at_unix: 123,
                pinned_at_unix: 456,
                ..Default::default()
            })
            .unwrap();
        let (tx, mut rx) = mpsc::unbounded_channel::<String>();
        let state = DaemonState::new(
            root.clone(),
            PeridotConfig::default(),
            test_options(None),
            tx,
        );

        let _ = dispatch_line(
            &state,
            r#"{"jsonrpc":"2.0","id":44,"method":"session.command","params":{"command":"/skills show auto-fix-parser"}}"#,
        )
        .await
        .unwrap();

        let line = rx.try_recv().unwrap();
        let value: Value = serde_json::from_str(&line).unwrap();
        assert_eq!(value["result"]["kind"], "skill_detail");
        assert_eq!(value["result"]["name"], "auto-fix-parser");
        assert_eq!(value["result"]["label"], "/auto-fix-parser");
        assert_eq!(value["result"]["detail"], "repair parser tests");
        assert_eq!(value["result"]["scope"], "auto");
        assert_eq!(value["result"]["pinned"], true);
        assert_eq!(value["result"]["last_used_at_unix"], 123);
        assert_eq!(
            value["result"]["body"],
            "repair parser tests\nrun cargo test"
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn session_command_skills_search_returns_matching_inventory() {
        let root = test_project("command-skills-search");
        let store = peridot_memory::MemoryStore::new(root.join(".peridot/memory.db"));
        store
            .save_skill(&peridot_memory::StoredSkill {
                name: "auto-fix-parser".into(),
                body: "repair parser tests".into(),
                description: "repair parser tests".into(),
                scope: "auto".into(),
                ..Default::default()
            })
            .unwrap();
        store
            .save_skill(&peridot_memory::StoredSkill {
                name: "release-notes".into(),
                body: "prepare changelog".into(),
                description: "write release notes".into(),
                scope: "auto".into(),
                ..Default::default()
            })
            .unwrap();
        let (tx, mut rx) = mpsc::unbounded_channel::<String>();
        let state = DaemonState::new(
            root.clone(),
            PeridotConfig::default(),
            test_options(None),
            tx,
        );

        let _ = dispatch_line(
            &state,
            r#"{"jsonrpc":"2.0","id":44,"method":"session.command","params":{"command":"/skills search parser"}}"#,
        )
        .await
        .unwrap();

        let line = rx.try_recv().unwrap();
        let value: Value = serde_json::from_str(&line).unwrap();
        assert_eq!(value["result"]["kind"], "skills");
        assert_eq!(value["result"]["query"], "parser");
        assert_eq!(value["result"]["total"], 1);
        assert_eq!(value["result"]["items"][0]["label"], "/auto-fix-parser");
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn session_command_skills_pin_toggles_skill_inventory() {
        let root = test_project("command-skills-pin");
        let store = peridot_memory::MemoryStore::new(root.join(".peridot/memory.db"));
        store
            .save_skill(&peridot_memory::StoredSkill {
                name: "auto-fix-parser".into(),
                body: "repair parser tests".into(),
                description: "repair parser tests".into(),
                scope: "auto".into(),
                ..Default::default()
            })
            .unwrap();
        let (tx, mut rx) = mpsc::unbounded_channel::<String>();
        let state = DaemonState::new(
            root.clone(),
            PeridotConfig::default(),
            test_options(None),
            tx,
        );

        let _ = dispatch_line(
            &state,
            r#"{"jsonrpc":"2.0","id":44,"method":"session.command","params":{"command":"/skills pin auto-fix-parser"}}"#,
        )
        .await
        .unwrap();

        let line = rx.try_recv().unwrap();
        let value: Value = serde_json::from_str(&line).unwrap();
        assert_eq!(value["result"]["kind"], "skills");
        assert_eq!(value["result"]["message"], "pinned skill `auto-fix-parser`");
        assert_eq!(value["result"]["items"][0]["pinned"], true);
        assert!(
            store
                .list_skills()
                .unwrap()
                .iter()
                .any(|skill| skill.name == "auto-fix-parser" && skill.pinned_at_unix > 0)
        );

        let _ = dispatch_line(
            &state,
            r#"{"jsonrpc":"2.0","id":45,"method":"session.command","params":{"command":"/skills unpin auto-fix-parser"}}"#,
        )
        .await
        .unwrap();

        let line = rx.try_recv().unwrap();
        let value: Value = serde_json::from_str(&line).unwrap();
        assert_eq!(
            value["result"]["message"],
            "unpinned skill `auto-fix-parser`"
        );
        assert_eq!(value["result"]["items"][0]["pinned"], false);
        assert!(
            store
                .list_skills()
                .unwrap()
                .iter()
                .any(|skill| skill.name == "auto-fix-parser" && skill.pinned_at_unix == 0)
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn session_command_skills_archive_hides_skill_inventory() {
        let root = test_project("command-skills-archive");
        let skill_dir = root.join(".peridot/skills/auto");
        std::fs::create_dir_all(&skill_dir).unwrap();
        std::fs::write(skill_dir.join("auto-fix-parser.md"), "repair parser tests").unwrap();
        let store = peridot_memory::MemoryStore::new(root.join(".peridot/memory.db"));
        store
            .save_skill(&peridot_memory::StoredSkill {
                name: "auto-fix-parser".into(),
                body: "repair parser tests".into(),
                description: "repair parser tests".into(),
                scope: "auto".into(),
                ..Default::default()
            })
            .unwrap();
        let (tx, mut rx) = mpsc::unbounded_channel::<String>();
        let state = DaemonState::new(
            root.clone(),
            PeridotConfig::default(),
            test_options(None),
            tx,
        );

        let _ = dispatch_line(
            &state,
            r#"{"jsonrpc":"2.0","id":44,"method":"session.command","params":{"command":"/skills archive auto-fix-parser"}}"#,
        )
        .await
        .unwrap();

        let line = rx.try_recv().unwrap();
        let value: Value = serde_json::from_str(&line).unwrap();
        assert_eq!(value["result"]["kind"], "skills");
        assert_eq!(
            value["result"]["message"],
            "archived skill `auto-fix-parser`"
        );
        assert_eq!(value["result"]["total"], 0);
        assert!(store.list_skills().unwrap().is_empty());
        assert!(
            root.join(".peridot/skills/archive/auto-fix-parser.md")
                .is_file()
        );

        let _ = dispatch_line(
            &state,
            r#"{"jsonrpc":"2.0","id":45,"method":"session.command","params":{"command":"/skills archived parser"}}"#,
        )
        .await
        .unwrap();

        let line = rx.try_recv().unwrap();
        let value: Value = serde_json::from_str(&line).unwrap();
        assert_eq!(value["result"]["kind"], "skills");
        assert_eq!(value["result"]["archived"], true);
        assert_eq!(value["result"]["total"], 1);
        assert_eq!(value["result"]["items"][0]["label"], "/auto-fix-parser");
        assert_eq!(value["result"]["items"][0]["archived"], true);

        let _ = dispatch_line(
            &state,
            r#"{"jsonrpc":"2.0","id":47,"method":"session.command","params":{"command":"/skills show auto-fix-parser"}}"#,
        )
        .await
        .unwrap();

        let line = rx.try_recv().unwrap();
        let value: Value = serde_json::from_str(&line).unwrap();
        assert_eq!(value["result"]["kind"], "skill_detail");
        assert_eq!(value["result"]["archived"], true);
        assert_eq!(value["result"]["body"], "repair parser tests");

        let _ = dispatch_line(
            &state,
            r#"{"jsonrpc":"2.0","id":46,"method":"session.command","params":{"command":"/skills restore auto-fix-parser"}}"#,
        )
        .await
        .unwrap();

        let line = rx.try_recv().unwrap();
        let value: Value = serde_json::from_str(&line).unwrap();
        assert_eq!(value["result"]["kind"], "skills");
        assert_eq!(
            value["result"]["message"],
            "restored skill `auto-fix-parser`"
        );
        assert_eq!(value["result"]["total"], 1);
        assert_eq!(store.list_skills().unwrap().len(), 1);
        assert!(
            root.join(".peridot/skills/auto/auto-fix-parser.md")
                .is_file()
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn session_list_returns_persisted_records() {
        let root = test_project("session-list");
        let store = MemoryStore::new(root.join(".peridot/memory.db"));
        let mut record = SessionRecord::new("session-recorded", &root);
        record.summary = "recorded summary".into();
        record.status = SessionLifecycle::Suspended;
        record.created_at_unix = 10;
        record.updated_at_unix = 20;
        record.last_task = Some("recorded task".into());
        store.save_session_record(&record).unwrap();
        let (tx, mut rx) = mpsc::unbounded_channel::<String>();
        let state = DaemonState::new(
            root.clone(),
            PeridotConfig::default(),
            test_options(None),
            tx,
        );

        let _ = dispatch_line(
            &state,
            r#"{"jsonrpc":"2.0","id":44,"method":"session.list"}"#,
        )
        .await
        .unwrap();

        let line = rx.try_recv().unwrap();
        let value: Value = serde_json::from_str(&line).unwrap();
        let sessions = value["result"]["sessions"].as_array().unwrap();
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0]["id"], "session-recorded");
        assert_eq!(sessions[0]["title"], "recorded task");
        assert_eq!(sessions[0]["status"], "suspended");
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn session_command_count_returns_lifecycle_breakdown() {
        let root = test_project("session-command-count");
        let store = MemoryStore::new(root.join(".peridot/memory.db"));
        for (id, status) in [
            ("idle-one", SessionLifecycle::Idle),
            ("running-one", SessionLifecycle::Running),
            ("done-one", SessionLifecycle::Done),
            ("done-two", SessionLifecycle::Done),
            ("failed-one", SessionLifecycle::Failed),
        ] {
            let mut record = SessionRecord::new(id, &root);
            record.status = status;
            store.save_session_record(&record).unwrap();
        }
        let (tx, mut rx) = mpsc::unbounded_channel::<String>();
        let state = DaemonState::new(
            root.clone(),
            PeridotConfig::default(),
            test_options(None),
            tx,
        );

        dispatch_line(
            &state,
            r#"{"jsonrpc":"2.0","id":45,"method":"session.command","params":{"command":"/session count"}}"#,
        )
        .await
        .unwrap();

        let line = rx.try_recv().unwrap();
        let value: Value = serde_json::from_str(&line).unwrap();
        let result = &value["result"];
        assert_eq!(result["kind"], "session_count");
        assert_eq!(result["total"], 5);
        assert_eq!(result["idle"], 1);
        assert_eq!(result["running"], 1);
        assert_eq!(result["done"], 2);
        assert_eq!(result["failed"], 1);
        assert_eq!(result["items"].as_array().unwrap().len(), 5);
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn session_command_rename_updates_persisted_session() {
        let root = test_project("session-command-rename");
        let store = MemoryStore::new(root.join(".peridot/memory.db"));
        let mut record = SessionRecord::new("session-rename", &root);
        record.summary = "old title".into();
        record.last_task = Some("old task".into());
        store.save_session_record(&record).unwrap();
        store
            .save_session(&SessionSummary {
                id: "session-rename".into(),
                summary: "old title".into(),
            })
            .unwrap();
        let (tx, _rx) = mpsc::unbounded_channel::<String>();
        let state = DaemonState::new(
            root.clone(),
            PeridotConfig::default(),
            test_options(None),
            tx,
        );

        let result = execute_session_command(
            &state,
            Some("session-rename"),
            "/session rename session-rename release prep",
            SlashCommand::SessionRename {
                target: "session-rename".into(),
                title: "release prep".into(),
            },
        )
        .await
        .unwrap();

        assert_eq!(result["kind"], "session_rename");
        assert_eq!(result["session_id"], "session-rename");
        assert_eq!(result["session_title"], "release prep");
        assert_eq!(result["renamed"], true);
        let renamed = store.get_session_record("session-rename").unwrap().unwrap();
        assert_eq!(renamed.summary, "release prep");
        assert_eq!(
            store
                .get_session("session-rename")
                .unwrap()
                .unwrap()
                .summary,
            "release prep"
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn session_command_delete_removes_persisted_session() {
        let root = test_project("session-command-delete");
        let store = MemoryStore::new(root.join(".peridot/memory.db"));
        let record = SessionRecord::new("session-delete", &root);
        store.save_session_record(&record).unwrap();
        store
            .save_session(&SessionSummary {
                id: "session-delete".into(),
                summary: "delete me".into(),
            })
            .unwrap();
        let sessions_root = root.join(".peridot").join("sessions");
        peridot_memory::save_session_blob(
            &sessions_root,
            "session-delete",
            "tui_state.json",
            br#"{"sessions":[]}"#,
        )
        .unwrap();
        let (tx, _rx) = mpsc::unbounded_channel::<String>();
        let state = DaemonState::new(
            root.clone(),
            PeridotConfig::default(),
            test_options(None),
            tx,
        );

        let result = execute_session_command(
            &state,
            Some("session-delete"),
            "/session delete session-delete",
            SlashCommand::SessionDelete("session-delete".into()),
        )
        .await
        .unwrap();

        assert_eq!(result["kind"], "session_delete");
        assert_eq!(result["session_id"], "session-delete");
        assert_eq!(result["deleted"], true);
        assert!(
            store
                .get_session_record("session-delete")
                .unwrap()
                .is_none()
        );
        assert!(store.get_session("session-delete").unwrap().is_none());
        assert!(!sessions_root.join("session-delete").exists());
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn session_command_close_removes_live_and_persisted_session() {
        let root = test_project("session-command-close");
        let store = MemoryStore::new(root.join(".peridot/memory.db"));
        let record = SessionRecord::new("session-close", &root);
        store.save_session_record(&record).unwrap();
        store
            .save_session(&SessionSummary {
                id: "session-close".into(),
                summary: "close me".into(),
            })
            .unwrap();
        let sessions_root = root.join(".peridot").join("sessions");
        peridot_memory::save_session_blob(
            &sessions_root,
            "session-close",
            "tui_state.json",
            br#"{"sessions":[]}"#,
        )
        .unwrap();
        let (tx, _rx) = mpsc::unbounded_channel::<String>();
        let state = DaemonState::new(
            root.clone(),
            PeridotConfig::default(),
            test_options(None),
            tx,
        );
        state.sessions.lock().await.insert(
            "session-close".to_string(),
            SessionEntry {
                cancel: CancelToken::new(),
                compact_request: Arc::new(AtomicBool::new(false)),
                task: None,
                spec: SessionRunSpec {
                    task: "close active session".to_string(),
                    mode: ExecutionMode::Execute,
                    permission: PermissionMode::Auto,
                    model: None,
                    reasoning_effort: None,
                    service_tier: None,
                    config: PeridotConfig::default(),
                },
                usage: Arc::new(StdMutex::new(LiveSessionUsage::default())),
                plan: Arc::new(StdMutex::new(LiveSessionPlan::default())),
                goal: Arc::new(StdMutex::new(LiveSessionGoal::default())),
                approval_grants: Vec::new(),
                waiting_approval: None,
            },
        );

        let result = execute_session_command(
            &state,
            Some("session-close"),
            "/session close session-close",
            SlashCommand::SessionClose("session-close".into()),
        )
        .await
        .unwrap();

        assert_eq!(result["kind"], "session_close");
        assert_eq!(result["session_id"], "session-close");
        assert_eq!(result["deleted"], true);
        assert_eq!(result["cancelled"], true);
        assert!(!state.sessions.lock().await.contains_key("session-close"));
        assert!(store.get_session_record("session-close").unwrap().is_none());
        assert!(store.get_session("session-close").unwrap().is_none());
        assert!(!sessions_root.join("session-close").exists());
        shutdown_sessions(&state).await;
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn session_command_switch_resolves_persisted_session() {
        let root = test_project("session-command-switch");
        let store = MemoryStore::new(root.join(".peridot/memory.db"));
        let mut record = SessionRecord::new("session-switch", &root);
        record.summary = "switch target".into();
        record.status = SessionLifecycle::Suspended;
        store.save_session_record(&record).unwrap();
        let (tx, _rx) = mpsc::unbounded_channel::<String>();
        let state = DaemonState::new(
            root.clone(),
            PeridotConfig::default(),
            test_options(None),
            tx,
        );

        let result = execute_session_command(
            &state,
            None,
            "/session switch target",
            SlashCommand::SessionSwitch("target".into()),
        )
        .await
        .unwrap();

        assert_eq!(result["kind"], "session_switch");
        assert_eq!(result["session_id"], "session-switch");
        assert_eq!(result["session_title"], "switch target");
        assert_eq!(result["status"], "suspended");
        assert_eq!(result["switched"], true);
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn session_command_goal_start_returns_goal_start_task() {
        let (tx, _rx) = mpsc::unbounded_channel::<String>();
        let root = test_project("session-command-goal-start");
        let state = DaemonState::new(
            root.clone(),
            PeridotConfig::default(),
            test_options(None),
            tx,
        );

        let result = execute_session_command(
            &state,
            None,
            "/goal ship release",
            SlashCommand::GoalStart("ship release".into()),
        )
        .await
        .unwrap();

        assert_eq!(result["kind"], "start_task");
        assert_eq!(result["label"], "goal");
        assert_eq!(result["task"], "ship release");
        assert_eq!(result["state_delta"]["mode"], "goal");
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn session_command_clear_removes_live_and_persisted_session() {
        let root = test_project("session-command-clear");
        let store = MemoryStore::new(root.join(".peridot/memory.db"));
        let record = SessionRecord::new("session-clear", &root);
        store.save_session_record(&record).unwrap();
        store
            .save_session(&SessionSummary {
                id: "session-clear".into(),
                summary: "clear me".into(),
            })
            .unwrap();
        let sessions_root = root.join(".peridot").join("sessions");
        peridot_memory::save_session_blob(
            &sessions_root,
            "session-clear",
            "tui_state.json",
            br#"{"sessions":[]}"#,
        )
        .unwrap();
        let (tx, _rx) = mpsc::unbounded_channel::<String>();
        let state = DaemonState::new(
            root.clone(),
            PeridotConfig::default(),
            test_options(None),
            tx,
        );
        state.sessions.lock().await.insert(
            "session-clear".to_string(),
            SessionEntry {
                cancel: CancelToken::new(),
                compact_request: Arc::new(AtomicBool::new(false)),
                task: None,
                spec: SessionRunSpec {
                    task: "clear active session".to_string(),
                    mode: ExecutionMode::Execute,
                    permission: PermissionMode::Auto,
                    model: None,
                    reasoning_effort: None,
                    service_tier: None,
                    config: PeridotConfig::default(),
                },
                usage: Arc::new(StdMutex::new(LiveSessionUsage::default())),
                plan: Arc::new(StdMutex::new(LiveSessionPlan::default())),
                goal: Arc::new(StdMutex::new(LiveSessionGoal::default())),
                approval_grants: Vec::new(),
                waiting_approval: None,
            },
        );

        let result =
            execute_session_command(&state, Some("session-clear"), "/clear", SlashCommand::Clear)
                .await
                .unwrap();

        assert_eq!(result["kind"], "client_action");
        assert_eq!(result["action"], "clear");
        assert_eq!(result["session_id"], "session-clear");
        assert_eq!(result["deleted"], true);
        assert_eq!(result["cancelled"], true);
        assert!(!state.sessions.lock().await.contains_key("session-clear"));
        assert!(store.get_session_record("session-clear").unwrap().is_none());
        assert!(store.get_session("session-clear").unwrap().is_none());
        assert!(!sessions_root.join("session-clear").exists());
        shutdown_sessions(&state).await;
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn session_command_cost_returns_live_and_aggregate_usage() {
        let root = test_project("session-command-cost");
        let store = MemoryStore::new(root.join(".peridot/memory.db"));
        let mut active_record = SessionRecord::new("active-cost", &root);
        active_record.status = SessionLifecycle::Running;
        active_record.last_task = Some("active work".into());
        active_record.total_tokens = 1000;
        active_record.total_cost_usd = 0.05;
        active_record.turns_used = 2;
        store.save_session_record(&active_record).unwrap();
        let mut background_record = SessionRecord::new("background-cost", &root);
        background_record.status = SessionLifecycle::Done;
        background_record.last_task = Some("background work".into());
        background_record.total_tokens = 700;
        background_record.total_cost_usd = 0.04;
        background_record.turns_used = 1;
        store.save_session_record(&background_record).unwrap();

        let (tx, _rx) = mpsc::unbounded_channel::<String>();
        let mut options = test_options(None);
        options.budget_usd = 0.5;
        let state = DaemonState::new(root.clone(), PeridotConfig::default(), options, tx);
        let usage = Arc::new(StdMutex::new(LiveSessionUsage {
            total_tokens: 2000,
            cost_usd: 0.10,
            turns_used: 3,
            cost_limit: Some(0.5),
            turns_limit: Some(5),
            committee_planner_tokens: 120,
            committee_planner_cost_usd: 0.01,
            committee_reviewer_tokens: 180,
            committee_reviewer_cost_usd: 0.01,
        }));
        state.sessions.lock().await.insert(
            "active-cost".to_string(),
            SessionEntry {
                cancel: CancelToken::new(),
                compact_request: Arc::new(AtomicBool::new(false)),
                task: None,
                spec: SessionRunSpec {
                    task: "active work".to_string(),
                    mode: ExecutionMode::Execute,
                    permission: PermissionMode::Auto,
                    model: None,
                    reasoning_effort: None,
                    service_tier: None,
                    config: PeridotConfig::default(),
                },
                usage,
                plan: Arc::new(StdMutex::new(LiveSessionPlan::default())),
                goal: Arc::new(StdMutex::new(LiveSessionGoal::default())),
                approval_grants: Vec::new(),
                waiting_approval: None,
            },
        );

        let result =
            execute_session_command(&state, Some("active-cost"), "/cost", SlashCommand::Cost)
                .await
                .unwrap();

        assert_eq!(result["kind"], "cost");
        assert_eq!(result["session_id"], "active-cost");
        assert_eq!(result["session_count"], 2);
        assert_eq!(result["current_tokens"], 2300);
        assert_eq!(result["total_tokens"], 3000);
        assert_eq!(result["executor_tokens"], 2700);
        assert_eq!(result["committee_tokens"], 300);
        assert!((result["current_cost_usd"].as_f64().unwrap() - 0.12).abs() < 1e-9);
        assert!((result["total_cost_usd"].as_f64().unwrap() - 0.16).abs() < 1e-9);
        assert!((result["executor_cost_usd"].as_f64().unwrap() - 0.14).abs() < 1e-9);
        assert!((result["committee_cost_usd"].as_f64().unwrap() - 0.02).abs() < 1e-9);
        assert_eq!(result["budget_limit_usd"], 0.5);
        assert_eq!(result["items"].as_array().unwrap().len(), 2);
        shutdown_sessions(&state).await;
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn session_command_plan_show_returns_live_plan_snapshot() {
        let root = test_project("session-command-plan-show");
        let (tx, _rx) = mpsc::unbounded_channel::<String>();
        let state = DaemonState::new(
            root.clone(),
            PeridotConfig::default(),
            test_options(None),
            tx,
        );
        let plan = Arc::new(StdMutex::new(LiveSessionPlan {
            steps: vec![
                PlanStepUpdate {
                    label: "scan workspace".to_string(),
                    done: true,
                },
                PlanStepUpdate {
                    label: "apply patch".to_string(),
                    done: false,
                },
            ],
            current: Some(1),
        }));
        state.sessions.lock().await.insert(
            "session-plan".to_string(),
            SessionEntry {
                cancel: CancelToken::new(),
                compact_request: Arc::new(AtomicBool::new(false)),
                task: None,
                spec: SessionRunSpec {
                    task: "ship plan".to_string(),
                    mode: ExecutionMode::Execute,
                    permission: PermissionMode::Auto,
                    model: None,
                    reasoning_effort: None,
                    service_tier: None,
                    config: PeridotConfig::default(),
                },
                usage: Arc::new(StdMutex::new(LiveSessionUsage::default())),
                plan,
                goal: Arc::new(StdMutex::new(LiveSessionGoal::default())),
                approval_grants: Vec::new(),
                waiting_approval: None,
            },
        );

        let result = execute_session_command(
            &state,
            Some("session-plan"),
            "/plan show",
            SlashCommand::PlanShow,
        )
        .await
        .unwrap();

        assert_eq!(result["kind"], "plan");
        assert_eq!(result["message"], "plan: 1/2 steps");
        assert_eq!(result["done"], 1);
        assert_eq!(result["total"], 2);
        assert_eq!(result["current"], 1);
        assert_eq!(result["items"][0]["detail"], "done");
        assert_eq!(result["items"][1]["detail"], "in_progress");
        assert_eq!(result["steps"][1]["text"], "apply patch");
        shutdown_sessions(&state).await;
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn session_command_save_persists_live_session_record() {
        let root = test_project("session-command-save");
        let (tx, _rx) = mpsc::unbounded_channel::<String>();
        let state = DaemonState::new(
            root.clone(),
            PeridotConfig::default(),
            test_options(None),
            tx,
        );
        state.sessions.lock().await.insert(
            "session-save".to_string(),
            SessionEntry {
                cancel: CancelToken::new(),
                compact_request: Arc::new(AtomicBool::new(false)),
                task: None,
                spec: SessionRunSpec {
                    task: "save this session".to_string(),
                    mode: ExecutionMode::Execute,
                    permission: PermissionMode::Auto,
                    model: None,
                    reasoning_effort: None,
                    service_tier: None,
                    config: PeridotConfig::default(),
                },
                usage: Arc::new(StdMutex::new(LiveSessionUsage {
                    total_tokens: 1500,
                    cost_usd: 0.08,
                    turns_used: 4,
                    ..LiveSessionUsage::default()
                })),
                plan: Arc::new(StdMutex::new(LiveSessionPlan::default())),
                goal: Arc::new(StdMutex::new(LiveSessionGoal::default())),
                approval_grants: Vec::new(),
                waiting_approval: None,
            },
        );

        let result = execute_session_command(
            &state,
            Some("session-save"),
            "/session save",
            SlashCommand::SessionSave,
        )
        .await
        .unwrap();

        assert_eq!(result["kind"], "session_save");
        assert_eq!(result["session_id"], "session-save");
        assert_eq!(result["status"], "running");
        assert_eq!(result["total_tokens"], 1500);
        assert_eq!(result["turns_used"], 4);
        assert!((result["total_cost_usd"].as_f64().unwrap() - 0.08).abs() < 1e-9);
        let store = MemoryStore::new(root.join(".peridot/memory.db"));
        let record = store.get_session_record("session-save").unwrap().unwrap();
        assert_eq!(record.last_task.as_deref(), Some("save this session"));
        assert_eq!(record.total_tokens, 1500);
        assert_eq!(record.turns_used, 4);
        assert!(store.get_session("session-save").unwrap().is_some());
        shutdown_sessions(&state).await;
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn session_command_goal_controls_live_goal_state() {
        let root = test_project("session-command-goal-control");
        let (tx, _rx) = mpsc::unbounded_channel::<String>();
        let state = DaemonState::new(
            root.clone(),
            PeridotConfig::default(),
            test_options(None),
            tx,
        );
        let goal = Arc::new(StdMutex::new(LiveSessionGoal {
            objective: Some("finish migration".to_string()),
            status: Some(GoalStatus::Running),
            started_at_unix: Some(123),
        }));
        state.sessions.lock().await.insert(
            "session-goal".to_string(),
            SessionEntry {
                cancel: CancelToken::new(),
                compact_request: Arc::new(AtomicBool::new(false)),
                task: None,
                spec: SessionRunSpec {
                    task: "finish migration".to_string(),
                    mode: ExecutionMode::Goal,
                    permission: PermissionMode::Auto,
                    model: None,
                    reasoning_effort: None,
                    service_tier: None,
                    config: PeridotConfig::default(),
                },
                usage: Arc::new(StdMutex::new(LiveSessionUsage::default())),
                plan: Arc::new(StdMutex::new(LiveSessionPlan {
                    steps: vec![
                        PlanStepUpdate {
                            label: "scan".to_string(),
                            done: true,
                        },
                        PlanStepUpdate {
                            label: "patch".to_string(),
                            done: false,
                        },
                    ],
                    current: Some(1),
                })),
                goal,
                approval_grants: Vec::new(),
                waiting_approval: None,
            },
        );

        let pause = execute_session_command(
            &state,
            Some("session-goal"),
            "/goal pause",
            SlashCommand::GoalPause,
        )
        .await
        .unwrap();
        assert_eq!(pause["kind"], "goal");
        assert_eq!(pause["status"], "paused");
        assert_eq!(pause["objective"], "finish migration");
        assert_eq!(pause["done"], 1);
        assert_eq!(pause["total"], 2);

        let resume = execute_session_command(
            &state,
            Some("session-goal"),
            "/goal resume",
            SlashCommand::GoalResume,
        )
        .await
        .unwrap();
        assert_eq!(resume["status"], "running");

        let clear = execute_session_command(
            &state,
            Some("session-goal"),
            "/goal clear",
            SlashCommand::GoalClear,
        )
        .await
        .unwrap();
        assert_eq!(clear["status"], "cleared");
        assert!(clear["objective"].is_null());
        assert_eq!(clear["total"], 0);
        shutdown_sessions(&state).await;
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn session_command_info_returns_live_and_persisted_context() {
        let root = test_project("session-command-info");
        let store = MemoryStore::new(root.join(".peridot/memory.db"));
        let mut record = SessionRecord::new("session-info", &root);
        record.status = SessionLifecycle::Suspended;
        record.last_task = Some("inspect daemon info".into());
        record.total_tokens = 1234;
        record.total_cost_usd = 0.42;
        record.turns_used = 7;
        store.save_session_record(&record).unwrap();

        let mut config = PeridotConfig::default();
        config.auth.primary = "openai-api".to_string();
        config.models.main = "configured-model".to_string();
        let (tx, _rx) = mpsc::unbounded_channel::<String>();
        let state = DaemonState::new(root.clone(), config.clone(), test_options(None), tx);
        let mut spec_config = config;
        spec_config.auth.primary = "openrouter-api".to_string();
        state.sessions.lock().await.insert(
            "session-info".to_string(),
            SessionEntry {
                cancel: CancelToken::new(),
                compact_request: Arc::new(AtomicBool::new(false)),
                task: None,
                spec: SessionRunSpec {
                    task: "inspect daemon info".to_string(),
                    mode: ExecutionMode::Goal,
                    permission: PermissionMode::Safe,
                    model: Some("live-model".to_string()),
                    reasoning_effort: Some(peridot_common::ReasoningEffort::High),
                    service_tier: Some(Some("fast".to_string())),
                    config: spec_config,
                },
                usage: Arc::new(StdMutex::new(LiveSessionUsage::default())),
                plan: Arc::new(StdMutex::new(LiveSessionPlan::default())),
                goal: Arc::new(StdMutex::new(LiveSessionGoal::default())),
                approval_grants: Vec::new(),
                waiting_approval: None,
            },
        );

        execute_session_command(
            &state,
            Some("session-info"),
            "/provider claude-api",
            SlashCommand::Provider("claude-api".to_string()),
        )
        .await
        .unwrap();
        let result =
            execute_session_command(&state, Some("session-info"), "/info", SlashCommand::Info)
                .await
                .unwrap();

        assert_eq!(result["kind"], "info");
        assert_eq!(result["session_id"], "session-info");
        assert_eq!(result["status"], "running");
        assert_eq!(result["model"], "live-model");
        assert_eq!(result["provider"], "claude-api");
        assert_eq!(result["mode"], "goal");
        assert_eq!(result["permission"], "safe");
        assert_eq!(result["reasoning_effort"], "high");
        assert_eq!(result["service_tier"], "fast");
        assert_eq!(result["turns_used"], 7);
        assert_eq!(result["total_tokens"], 1234);
        assert_eq!(result["total_cost_usd"], 0.42);
        let items = result["items"].as_array().unwrap();
        assert!(items.iter().any(|item| {
            item["label"] == "last task" && item["detail"] == "inspect daemon info"
        }));
        shutdown_sessions(&state).await;
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn session_subscribe_list_emits_start_notifications() {
        let root = test_project("session-list-subscribe");
        let response_file = root.join("responses.jsonl");
        std::fs::write(
            &response_file,
            r#"{"action":"agent_done","parameters":{"summary":"done"}}
"#,
        )
        .unwrap();
        let (tx, mut rx) = mpsc::unbounded_channel::<String>();
        let state = DaemonState::new(
            root.clone(),
            PeridotConfig::default(),
            test_options(Some(response_file)),
            tx,
        );

        let _ = dispatch_line(
            &state,
            r#"{"jsonrpc":"2.0","id":45,"method":"session.subscribe_list"}"#,
        )
        .await
        .unwrap();
        let _ = dispatch_line(
            &state,
            r#"{"jsonrpc":"2.0","id":46,"method":"session.start","params":{"task":"sync me"}}"#,
        )
        .await
        .unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        let mut values = Vec::new();
        while let Ok(line) = rx.try_recv() {
            values.push(serde_json::from_str::<Value>(&line).unwrap());
        }
        let start_response = values
            .iter()
            .find(|value| value["id"] == 46)
            .expect("start response");
        let session_id = start_response["result"]["session_id"].as_str().unwrap();
        assert!(values.iter().any(|value| {
            value["method"] == "session.list_changed"
                && value["params"]["sessions"]
                    .as_array()
                    .map(|sessions| {
                        sessions.iter().any(|session| {
                            session["id"] == session_id && session["running"] == true
                        })
                    })
                    .unwrap_or(false)
        }));
        shutdown_sessions(&state).await;
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn session_command_skill_appends_plan_reminder_context() {
        let root = test_project("skill-command");
        let store = peridot_memory::MemoryStore::new(root.join(".peridot/memory.db"));
        store
            .save_skill(&peridot_memory::StoredSkill {
                name: "auto-fix-parser".into(),
                body: "## Steps\nRun parser tests".into(),
                description: "repair parser tests".into(),
                scope: "auto".into(),
                ..Default::default()
            })
            .unwrap();
        let (tx, mut rx) = mpsc::unbounded_channel::<String>();
        let state = DaemonState::new(
            root.clone(),
            PeridotConfig::default(),
            test_options(None),
            tx,
        );

        let _ = dispatch_line(
            &state,
            r#"{"jsonrpc":"2.0","id":43,"method":"session.command","params":{"session_id":"session-skill","command":"/auto-fix-parser --dry"}}"#,
        )
        .await
        .unwrap();

        let mut values = Vec::new();
        while let Ok(line) = rx.try_recv() {
            values.push(serde_json::from_str::<Value>(&line).unwrap());
        }
        assert!(
            values
                .iter()
                .any(|value| { value["id"] == 43 && value["result"]["kind"] == "skill" })
        );
        let entries = read_context_snapshot(&state, "session-skill").unwrap();
        let last = entries.last().unwrap();
        assert_eq!(last.source, ContextSource::PlanReminder);
        assert!(last.content.contains("[skill:auto-fix-parser]"));
        assert!(last.content.contains("Operator passed args: --dry"));
        assert!(last.content.contains("Run parser tests"));
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn session_command_skills_use_appends_plan_reminder_context() {
        let root = test_project("skills-use-command");
        let store = peridot_memory::MemoryStore::new(root.join(".peridot/memory.db"));
        store
            .save_skill(&peridot_memory::StoredSkill {
                name: "auto-fix-parser".into(),
                body: "## Steps\nRun parser tests".into(),
                description: "repair parser tests".into(),
                scope: "auto".into(),
                ..Default::default()
            })
            .unwrap();
        let (tx, mut rx) = mpsc::unbounded_channel::<String>();
        let state = DaemonState::new(
            root.clone(),
            PeridotConfig::default(),
            test_options(None),
            tx,
        );

        let _ = dispatch_line(
            &state,
            r#"{"jsonrpc":"2.0","id":44,"method":"session.command","params":{"session_id":"session-skill","command":"/skills use auto-fix-parser --dry"}}"#,
        )
        .await
        .unwrap();

        let mut values = Vec::new();
        while let Ok(line) = rx.try_recv() {
            values.push(serde_json::from_str::<Value>(&line).unwrap());
        }
        assert!(
            values
                .iter()
                .any(|value| { value["id"] == 44 && value["result"]["kind"] == "skill" })
        );
        let entries = read_context_snapshot(&state, "session-skill").unwrap();
        let last = entries.last().unwrap();
        assert_eq!(last.source, ContextSource::PlanReminder);
        assert!(last.content.contains("[skill:auto-fix-parser]"));
        assert!(last.content.contains("Operator passed args: --dry"));
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn session_command_attach_appends_file_context() {
        let root = test_project("attach-command");
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(root.join("src/lib.rs"), "pub fn attached() {}\n").unwrap();
        let (tx, mut rx) = mpsc::unbounded_channel::<String>();
        let state = DaemonState::new(
            root.clone(),
            PeridotConfig::default(),
            test_options(None),
            tx,
        );

        let _ = dispatch_line(
            &state,
            r#"{"jsonrpc":"2.0","id":44,"method":"session.command","params":{"session_id":"session-attach","command":"/attach src/lib.rs"}}"#,
        )
        .await
        .unwrap();

        let mut values = Vec::new();
        while let Ok(line) = rx.try_recv() {
            values.push(serde_json::from_str::<Value>(&line).unwrap());
        }
        let response = values
            .iter()
            .find(|value| value["id"] == 44 && value["result"]["kind"] == "attach")
            .expect("attach response");
        assert_eq!(response["result"]["attachment"]["path"], "src/lib.rs");
        assert_eq!(response["result"]["attachment"]["media_type"], "text/plain");
        assert_eq!(response["result"]["attachment"]["inlined"], true);
        assert!(
            response["result"]["attachment"]["content"]
                .as_str()
                .unwrap()
                .contains("pub fn attached()")
        );
        assert_eq!(response["result"]["items"][0]["source"], "attachment");
        assert_eq!(response["result"]["items"][0]["inlined"], true);
        let entries = read_context_snapshot(&state, "session-attach").unwrap();
        let last = entries.last().unwrap();
        assert_eq!(last.source, ContextSource::PlanReminder);
        assert!(last.content.contains("[attachment]"));
        assert!(last.content.contains("path: src/lib.rs"));
        assert!(last.content.contains("pub fn attached()"));

        std::fs::write(root.join("screen.png"), [0x89, b'P', b'N', b'G']).unwrap();
        let _ = dispatch_line(
            &state,
            r#"{"jsonrpc":"2.0","id":45,"method":"session.command","params":{"session_id":"session-attach","command":"/attach screen.png"}}"#,
        )
        .await
        .unwrap();
        let mut image_response = None;
        while let Ok(line) = rx.try_recv() {
            let value: Value = serde_json::from_str(&line).unwrap();
            if value["id"] == 45 {
                image_response = Some(value);
                break;
            }
        }
        let image_response = image_response.expect("image attach response");
        assert_eq!(image_response["result"]["kind"], "attach");
        assert_eq!(
            image_response["result"]["attachment"]["media_type"],
            "image/png"
        );
        assert_eq!(image_response["result"]["attachment"]["inlined"], false);
        assert!(image_response["result"]["attachment"]["content"].is_null());
        assert_eq!(image_response["result"]["items"][0]["inlined"], false);

        let _ = dispatch_line(
            &state,
            r#"{"jsonrpc":"2.0","id":46,"method":"session.command","params":{"session_id":"session-attach","command":"/attachments"}}"#,
        )
        .await
        .unwrap();
        let mut list_response = None;
        while let Ok(line) = rx.try_recv() {
            let value: Value = serde_json::from_str(&line).unwrap();
            if value["id"] == 46 {
                list_response = Some(value);
                break;
            }
        }
        let list_response = list_response.expect("attachments response");
        assert_eq!(list_response["result"]["kind"], "attachments");
        assert_eq!(list_response["result"]["total"], 2);
        assert_eq!(
            list_response["result"]["attachments"][0]["path"],
            "src/lib.rs"
        );
        assert_eq!(
            list_response["result"]["attachments"][1]["media_type"],
            "image/png"
        );

        let _ = dispatch_line(
            &state,
            r#"{"jsonrpc":"2.0","id":47,"method":"session.command","params":{"session_id":"session-attach","command":"/detach ./src/lib.rs"}}"#,
        )
        .await
        .unwrap();
        let mut detach_response = None;
        while let Ok(line) = rx.try_recv() {
            let value: Value = serde_json::from_str(&line).unwrap();
            if value["id"] == 47 {
                detach_response = Some(value);
                break;
            }
        }
        let detach_response = detach_response.expect("detach response");
        assert_eq!(detach_response["result"]["kind"], "detach");
        assert_eq!(detach_response["result"]["removed_count"], 1);
        assert_eq!(detach_response["result"]["remaining_count"], 1);
        assert_eq!(
            detach_response["result"]["removed"][0]["path"],
            "src/lib.rs"
        );
        assert_eq!(
            detach_response["result"]["attachments"][0]["path"],
            "screen.png"
        );
        let entries = read_context_snapshot(&state, "session-attach").unwrap();
        let remaining = crate::commands::attachments_from_context(&entries);
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].path, "screen.png");

        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn session_command_export_writes_session_artifacts() {
        let root = test_project("session-export");
        let session_dir = root.join(".peridot/sessions/session-export");
        std::fs::create_dir_all(&session_dir).unwrap();
        let context = vec![ContextEntry::trusted(
            ContextSource::PlanReminder,
            "[attachment]\npath: src/lib.rs\nbytes: 5\n\n```text\nhello\n```",
        )];
        std::fs::write(
            session_dir.join("context.bin"),
            serde_json::to_vec(&context).unwrap(),
        )
        .unwrap();
        std::fs::write(
            session_dir.join("notes.ndjson"),
            "{\"ts\":1,\"text\":\"remember\"}\n",
        )
        .unwrap();
        std::fs::write(
            session_dir.join("transcript.ndjson"),
            "{\"kind\":\"user\",\"text\":\"task\",\"ts\":1}\n",
        )
        .unwrap();
        let (tx, mut rx) = mpsc::unbounded_channel::<String>();
        let state = DaemonState::new(
            root.clone(),
            PeridotConfig::default(),
            test_options(None),
            tx,
        );

        let _ = dispatch_line(
            &state,
            r#"{"jsonrpc":"2.0","id":48,"method":"session.command","params":{"session_id":"session-export","command":"/export attachments notes timeline"}}"#,
        )
        .await
        .unwrap();
        let mut response = None;
        while let Ok(line) = rx.try_recv() {
            let value: Value = serde_json::from_str(&line).unwrap();
            if value["id"] == 48 {
                response = Some(value);
                break;
            }
        }
        let response = response.expect("export response");
        assert_eq!(response["result"]["kind"], "session_export");
        assert_eq!(
            response["result"]["artifact_classes"],
            serde_json::json!(["attachments", "notes", "timeline"])
        );
        let destination = response["result"]["destination"].as_str().unwrap();
        assert!(Path::new(destination).join("attachments.json").is_file());
        assert!(Path::new(destination).join("notes.ndjson").is_file());
        assert!(Path::new(destination).join("timeline.json").is_file());
        assert_eq!(response["result"]["artifacts"][0]["count"], 1);

        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn echo_method_returns_text_unchanged() {
        let out = dispatch_and_collect(
            r#"{"jsonrpc":"2.0","id":2,"method":"peridot.echo","params":{"text":"hello"}}"#,
        )
        .await;
        assert_eq!(out[0]["id"], 2);
        assert_eq!(out[0]["result"]["echo"], "hello");
    }

    #[tokio::test]
    async fn echo_with_non_object_params_returns_invalid_params_error() {
        let out = dispatch_and_collect(
            r#"{"jsonrpc":"2.0","id":3,"method":"peridot.echo","params":"not-an-object"}"#,
        )
        .await;
        assert_eq!(out[0]["id"], 3);
        assert_eq!(out[0]["error"]["code"], -32602);
    }

    #[tokio::test]
    async fn unknown_method_returns_method_not_found() {
        let out = dispatch_and_collect(r#"{"jsonrpc":"2.0","id":4,"method":"not.real"}"#).await;
        assert_eq!(out[0]["id"], 4);
        assert_eq!(out[0]["error"]["code"], -32601);
    }

    #[tokio::test]
    async fn generate_title_rejects_missing_task() {
        let out = dispatch_and_collect(
            r#"{"jsonrpc":"2.0","id":51,"method":"session.generate_title","params":{}}"#,
        )
        .await;
        assert_eq!(out[0]["id"], 51);
        assert_eq!(out[0]["error"]["code"], -32602);
    }

    #[tokio::test]
    async fn generate_title_rejects_empty_task() {
        let out = dispatch_and_collect(
            r#"{"jsonrpc":"2.0","id":52,"method":"session.generate_title","params":{"task":"   "}}"#,
        )
        .await;
        assert_eq!(out[0]["id"], 52);
        assert_eq!(out[0]["error"]["code"], -32602);
    }

    #[tokio::test]
    async fn generate_title_rejects_non_object_params() {
        let out = dispatch_and_collect(
            r#"{"jsonrpc":"2.0","id":53,"method":"session.generate_title","params":"oops"}"#,
        )
        .await;
        assert_eq!(out[0]["id"], 53);
        assert_eq!(out[0]["error"]["code"], -32602);
    }

    #[tokio::test]
    async fn settings_list_returns_curated_items_with_config_path() {
        let out =
            dispatch_and_collect(r#"{"jsonrpc":"2.0","id":80,"method":"settings.list"}"#).await;
        assert_eq!(out[0]["id"], 80);
        let result = &out[0]["result"];
        let items = result["items"].as_array().expect("items array present");
        assert!(
            items.len() >= 15,
            "expected curated registry to expose 15+ items, got {}",
            items.len()
        );
        // settings_registry must include a stable autonomy toggle so the
        // webview's `Auto-verify` section actually has something to render.
        let auto_verify = items
            .iter()
            .find(|i| i["id"] == "defaults.auto_verify_after_mutation")
            .expect("auto-verify item exposed");
        assert_eq!(auto_verify["value"]["kind"], "Bool");
        assert!(
            result["config_path"]
                .as_str()
                .unwrap_or_default()
                .ends_with("config.toml")
        );
    }

    #[tokio::test]
    async fn settings_save_round_trips_through_list() {
        // Drive a real save+reload through the dispatcher so the on-disk
        // TOML round-trips end to end (encode, write, re-read, decode).
        let (tx, mut rx) = mpsc::unbounded_channel::<String>();
        let root = test_project("settings_round_trip");
        let state = DaemonState::new(
            root.clone(),
            PeridotConfig::default(),
            test_options(None),
            tx,
        );
        // Prime the config file via settings.list.
        dispatch_line(
            &state,
            r#"{"jsonrpc":"2.0","id":1,"method":"settings.list"}"#,
        )
        .await
        .unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(30)).await;
        let mut list_first = Vec::new();
        while let Ok(line) = rx.try_recv() {
            list_first.push(serde_json::from_str::<Value>(&line).unwrap());
        }
        // The handshake notification arrives ahead of the response, hence
        // we pick the entry with our id.
        let list_response = list_first
            .iter()
            .find(|v| v["id"] == 1)
            .expect("settings.list response");
        let mut items: Vec<Value> = list_response["result"]["items"].as_array().unwrap().clone();

        // Flip auto_verify_after_mutation to true.
        for item in items.iter_mut() {
            if item["id"] == "defaults.auto_verify_after_mutation" {
                item["value"]["data"] = Value::Bool(true);
            }
        }

        let save_req = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "settings.save",
            "params": { "items": items },
        });
        dispatch_line(&state, &serde_json::to_string(&save_req).unwrap())
            .await
            .unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(30)).await;
        let mut save_responses = Vec::new();
        while let Ok(line) = rx.try_recv() {
            save_responses.push(serde_json::from_str::<Value>(&line).unwrap());
        }
        let save_response = save_responses
            .iter()
            .find(|v| v["id"] == 2)
            .expect("settings.save response");
        assert_eq!(save_response["result"]["saved"], true);

        // Re-list and confirm the change survived a TOML round trip.
        dispatch_line(
            &state,
            r#"{"jsonrpc":"2.0","id":3,"method":"settings.list"}"#,
        )
        .await
        .unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(30)).await;
        let mut list_second = Vec::new();
        while let Ok(line) = rx.try_recv() {
            list_second.push(serde_json::from_str::<Value>(&line).unwrap());
        }
        let list_response_second = list_second
            .iter()
            .find(|v| v["id"] == 3)
            .expect("second settings.list response");
        let items_second = list_response_second["result"]["items"].as_array().unwrap();
        let saved_item = items_second
            .iter()
            .find(|i| i["id"] == "defaults.auto_verify_after_mutation")
            .unwrap();
        assert_eq!(saved_item["value"]["data"], true);

        shutdown_sessions(&state).await;
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn settings_save_rejects_missing_items_array() {
        let out = dispatch_and_collect(
            r#"{"jsonrpc":"2.0","id":81,"method":"settings.save","params":{}}"#,
        )
        .await;
        let response = out.iter().find(|v| v["id"] == 81).expect("save response");
        assert_eq!(response["error"]["code"], -32602);
    }

    #[tokio::test]
    async fn settings_save_rejects_non_object_params() {
        let out = dispatch_and_collect(
            r#"{"jsonrpc":"2.0","id":82,"method":"settings.save","params":"oops"}"#,
        )
        .await;
        let response = out.iter().find(|v| v["id"] == 82).expect("save response");
        assert_eq!(response["error"]["code"], -32602);
    }

    #[tokio::test]
    async fn handshake_emits_schema_and_daemon_version() {
        let (tx, mut rx) = mpsc::unbounded_channel::<String>();
        let root = test_project("handshake");
        let state = DaemonState::new(
            root.clone(),
            PeridotConfig::default(),
            test_options(None),
            tx,
        );
        emit_handshake(&state).unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        let line = rx.try_recv().unwrap();
        let value: Value = serde_json::from_str(&line).unwrap();
        assert_eq!(value["jsonrpc"], "2.0");
        assert_eq!(value["method"], "peridot.handshake");
        // Should not be a response/request — no id field on a notification.
        assert!(value.get("id").is_none());
        assert_eq!(
            value["params"]["schema_version"],
            peridot_core::AGENT_RUN_EVENT_SCHEMA_VERSION
        );
        assert_eq!(value["params"]["daemon_version"], env!("CARGO_PKG_VERSION"));
        shutdown_sessions(&state).await;
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn session_command_todos_returns_structured_hits() {
        let (tx, mut rx) = mpsc::unbounded_channel::<String>();
        let root = test_project("command-todos");
        let src = root.join("src");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::write(src.join("lib.rs"), "// TODO: wire command rpc\n").unwrap();
        let state = DaemonState::new(
            root.clone(),
            PeridotConfig::default(),
            test_options(None),
            tx,
        );
        let line = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 41,
            "method": "session.command",
            "params": { "command": "/todos" }
        })
        .to_string();

        let _ = dispatch_line(&state, &line).await.unwrap();
        let response: Value = serde_json::from_str(&rx.recv().await.unwrap()).unwrap();

        assert_eq!(response["id"], 41);
        assert_eq!(response["result"]["kind"], "todos");
        assert_eq!(response["result"]["items"][0]["path"], "src/lib.rs");
        assert_eq!(response["result"]["items"][0]["line"], 1);

        shutdown_sessions(&state).await;
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn session_command_codemap_returns_symbols_and_todos() {
        let (tx, mut rx) = mpsc::unbounded_channel::<String>();
        let root = test_project("command-codemap");
        let src = root.join("src");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::write(
            src.join("lib.rs"),
            "pub struct Runner;\n// TODO: finish codemap\nfn use_runner(value: Runner) {}\n",
        )
        .unwrap();
        let state = DaemonState::new(
            root.clone(),
            PeridotConfig::default(),
            test_options(None),
            tx,
        );
        let line = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 42,
            "method": "session.command",
            "params": { "command": "/codemap" }
        })
        .to_string();

        let _ = dispatch_line(&state, &line).await.unwrap();
        let mut response = Value::Null;
        while let Some(line) = rx.recv().await {
            let value: Value = serde_json::from_str(&line).unwrap();
            if value["id"] == 42 {
                response = value;
                break;
            }
        }

        assert_eq!(response["id"], 42);
        assert_eq!(response["result"]["kind"], "codemap");
        assert_eq!(response["result"]["symbol_count"], 1);
        assert_eq!(response["result"]["todo_count"], 1);
        assert_eq!(response["result"]["refreshed"], true);
        assert!(root.join(".peridot/codemap.json").is_file());
        assert!(
            response["result"]["items"]
                .as_array()
                .unwrap()
                .iter()
                .any(|item| item["source"] == "symbol" && item["label"] == "struct Runner")
        );
        assert!(
            response["result"]["items"]
                .as_array()
                .unwrap()
                .iter()
                .any(|item| item["source"] == "todo"
                    && item["detail"].as_str().unwrap().contains("TODO"))
        );

        let status_line = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 421,
            "method": "session.command",
            "params": { "command": "/codemap status" }
        })
        .to_string();
        let _ = dispatch_line(&state, &status_line).await.unwrap();
        let status_response: Value = serde_json::from_str(&rx.recv().await.unwrap()).unwrap();
        assert_eq!(status_response["id"], 421);
        assert_eq!(status_response["result"]["kind"], "codemap_status");
        assert_eq!(status_response["result"]["index_exists"], true);
        assert_eq!(status_response["result"]["symbol_count"], 1);
        assert_eq!(status_response["result"]["todo_count"], 1);
        assert_eq!(status_response["result"]["source_files"], 1);

        let refresh_line = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 43,
            "method": "session.command",
            "params": { "command": "/codemap refresh" }
        })
        .to_string();
        let _ = dispatch_line(&state, &refresh_line).await.unwrap();
        let refresh_response: Value = serde_json::from_str(&rx.recv().await.unwrap()).unwrap();
        assert_eq!(refresh_response["id"], 43);
        assert_eq!(refresh_response["result"]["kind"], "codemap");
        assert_eq!(refresh_response["result"]["refreshed"], true);

        let find_line = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 44,
            "method": "session.command",
            "params": { "command": "/codemap find runner" }
        })
        .to_string();
        let _ = dispatch_line(&state, &find_line).await.unwrap();
        let find_response: Value = serde_json::from_str(&rx.recv().await.unwrap()).unwrap();
        assert_eq!(find_response["id"], 44);
        assert_eq!(find_response["result"]["kind"], "codemap");
        assert_eq!(
            find_response["result"]["title"],
            "Workspace Code Map Search"
        );
        assert_eq!(find_response["result"]["query"], "runner");
        assert_eq!(find_response["result"]["symbol_count"], 1);
        assert_eq!(find_response["result"]["todo_count"], 0);
        assert!(
            find_response["result"]["items"]
                .as_array()
                .unwrap()
                .iter()
                .any(|item| item["source"] == "symbol" && item["label"] == "struct Runner")
        );

        let locate_line = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 45,
            "method": "session.command",
            "params": { "command": "/codemap locate runner" }
        })
        .to_string();
        let _ = dispatch_line(&state, &locate_line).await.unwrap();
        let locate_response: Value = serde_json::from_str(&rx.recv().await.unwrap()).unwrap();
        assert_eq!(locate_response["id"], 45);
        assert_eq!(locate_response["result"]["kind"], "codemap");
        assert_eq!(
            locate_response["result"]["title"],
            "Workspace Symbol Locations"
        );
        assert_eq!(locate_response["result"]["query"], "runner");
        assert_eq!(locate_response["result"]["symbol_count"], 1);
        assert_eq!(locate_response["result"]["todo_count"], 0);
        assert_eq!(locate_response["result"]["items"][0]["path"], "src/lib.rs");
        assert_eq!(locate_response["result"]["items"][0]["line"], 1);

        let outline_line = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 46,
            "method": "session.command",
            "params": { "command": "/codemap outline src/lib.rs" }
        })
        .to_string();
        let _ = dispatch_line(&state, &outline_line).await.unwrap();
        let outline_response: Value = serde_json::from_str(&rx.recv().await.unwrap()).unwrap();
        assert_eq!(outline_response["id"], 46);
        assert_eq!(outline_response["result"]["kind"], "codemap");
        assert_eq!(
            outline_response["result"]["title"],
            "Workspace File Outline"
        );
        assert_eq!(outline_response["result"]["query"], "src/lib.rs");
        assert_eq!(outline_response["result"]["symbol_count"], 1);
        assert_eq!(outline_response["result"]["todo_count"], 0);
        assert!(
            outline_response["result"]["items"]
                .as_array()
                .unwrap()
                .iter()
                .all(|item| item["source"] == "symbol")
        );

        let refs_line = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 47,
            "method": "session.command",
            "params": { "command": "/codemap refs Runner" }
        })
        .to_string();
        let _ = dispatch_line(&state, &refs_line).await.unwrap();
        let refs_response: Value = serde_json::from_str(&rx.recv().await.unwrap()).unwrap();
        assert_eq!(refs_response["id"], 47);
        assert_eq!(refs_response["result"]["kind"], "codemap");
        assert_eq!(
            refs_response["result"]["title"],
            "Workspace Symbol References"
        );
        assert_eq!(refs_response["result"]["query"], "Runner");
        assert_eq!(refs_response["result"]["reference_count"], 1);
        assert_eq!(refs_response["result"]["symbol_count"], 0);
        assert_eq!(refs_response["result"]["todo_count"], 0);
        assert_eq!(refs_response["result"]["items"][0]["source"], "reference");
        assert_eq!(refs_response["result"]["items"][0]["line"], 3);

        shutdown_sessions(&state).await;
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn session_command_branch_returns_picker_result() {
        let (tx, mut rx) = mpsc::unbounded_channel::<String>();
        let root = test_project("command-branch-picker");
        let state = DaemonState::new(
            root.clone(),
            PeridotConfig::default(),
            test_options(None),
            tx,
        );
        let session_id = "session-test-branch";
        let snapshot_path = context_snapshot_path(&state, session_id);
        std::fs::create_dir_all(snapshot_path.parent().unwrap()).unwrap();
        let mut first = ContextEntry::trusted(ContextSource::User, "draft the plan");
        first.turn_id = 1;
        let mut second = ContextEntry::trusted(ContextSource::Assistant, "implemented the plan");
        second.turn_id = 2;
        std::fs::write(
            &snapshot_path,
            serde_json::to_vec(&vec![first, second]).unwrap(),
        )
        .unwrap();
        let line = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 42,
            "method": "session.command",
            "params": { "session_id": session_id, "command": "/branch" }
        })
        .to_string();

        let _ = dispatch_line(&state, &line).await.unwrap();
        let mut response = Value::Null;
        while let Some(line) = rx.recv().await {
            let value: Value = serde_json::from_str(&line).unwrap();
            if value["id"] == 42 {
                response = value;
                break;
            }
        }

        assert_eq!(response["id"], 42);
        assert_eq!(response["result"]["kind"], "branch_picker");
        assert_eq!(response["result"]["items"].as_array().unwrap().len(), 2);
        assert_eq!(response["result"]["items"][0]["turn_id"], 1);
        assert_eq!(response["result"]["items"][0]["source"], "user");
        assert_eq!(response["result"]["items"][1]["turn_id"], 2);

        shutdown_sessions(&state).await;
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn session_command_rewind_updates_context_snapshot() {
        let (tx, _rx) = mpsc::unbounded_channel::<String>();
        let root = test_project("command-rewind");
        let state = DaemonState::new(
            root.clone(),
            PeridotConfig::default(),
            test_options(None),
            tx,
        );
        let session_id = "session-test-rewind";
        let snapshot_path = context_snapshot_path(&state, session_id);
        std::fs::create_dir_all(snapshot_path.parent().unwrap()).unwrap();
        let mut first = ContextEntry::trusted(ContextSource::User, "first prompt");
        first.turn_id = 1;
        let mut first_reply = ContextEntry::trusted(ContextSource::Assistant, "first reply");
        first_reply.turn_id = 1;
        let mut second = ContextEntry::trusted(ContextSource::User, "second prompt");
        second.turn_id = 2;
        let mut second_reply = ContextEntry::trusted(ContextSource::Assistant, "second reply");
        second_reply.turn_id = 2;
        std::fs::write(
            &snapshot_path,
            serde_json::to_vec(&vec![first, first_reply, second, second_reply]).unwrap(),
        )
        .unwrap();
        let result =
            execute_session_command(&state, Some(session_id), "/rewind", SlashCommand::Rewind)
                .await
                .unwrap();

        assert_eq!(result["kind"], "rewind");
        assert_eq!(result["restored_prompt"], "second prompt");
        assert_eq!(result["removed_context_entries"], 2);
        let entries = read_context_snapshot(&state, session_id).unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].content, "first prompt");
        assert_eq!(entries[1].content, "first reply");

        shutdown_sessions(&state).await;
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn session_start_without_task_returns_invalid_params() {
        let out =
            dispatch_and_collect(r#"{"jsonrpc":"2.0","id":5,"method":"session.start"}"#).await;
        assert_eq!(out[0]["id"], 5);
        assert_eq!(out[0]["error"]["code"], -32602);
    }

    #[tokio::test]
    async fn session_start_rejects_invalid_reasoning_effort() {
        let out = dispatch_and_collect(
            r#"{"jsonrpc":"2.0","id":17,"method":"session.start","params":{"task":"finish","reasoning_effort":"huge"}}"#,
        )
        .await;
        assert_eq!(out[0]["id"], 17);
        assert_eq!(out[0]["error"]["code"], -32602);
    }

    #[tokio::test]
    async fn session_start_rejects_invalid_service_tier() {
        let out = dispatch_and_collect(
            r#"{"jsonrpc":"2.0","id":18,"method":"session.start","params":{"task":"finish","service_tier":"expensive"}}"#,
        )
        .await;
        assert_eq!(out[0]["id"], 18);
        assert_eq!(out[0]["error"]["code"], -32602);
    }

    #[tokio::test]
    async fn session_start_with_task_returns_id_and_started_event() {
        let root = test_project("mock");
        let response_file = root.join("responses.jsonl");
        std::fs::write(
            &response_file,
            r#"{"action":"agent_done","parameters":{"summary":"done"}}
"#,
        )
        .unwrap();
        let out = dispatch_and_collect_with_options(
            r#"{"jsonrpc":"2.0","id":6,"method":"session.start","params":{"task":"finish"}}"#,
            test_options(Some(response_file)),
        )
        .await;
        let session_id = out[0]["result"]["session_id"].as_str().unwrap();
        assert!(session_id.starts_with("session-"));
        assert!(out.iter().any(|value| {
            value["method"] == "event"
                && value["params"]["session_id"] == session_id
                && value["params"]["event"]["kind"] == "run_started"
        }));
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn session_start_can_continue_requested_session_id() {
        let root = test_project("mock-continue");
        let response_file = root.join("responses.jsonl");
        std::fs::write(
            &response_file,
            r#"{"action":"agent_done","parameters":{"summary":"done"}}
"#,
        )
        .unwrap();
        let out = dispatch_and_collect_with_options(
            r#"{"jsonrpc":"2.0","id":16,"method":"session.start","params":{"task":"continue","session_id":"session-existing"}}"#,
            test_options(Some(response_file)),
        )
        .await;
        assert_eq!(out[0]["result"]["session_id"], "session-existing");
        assert!(out.iter().any(|value| {
            value["method"] == "event"
                && value["params"]["session_id"] == "session-existing"
                && value["params"]["event"]["kind"] == "run_started"
        }));
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn session_cancel_unknown_id_returns_false() {
        let out = dispatch_and_collect(
            r#"{"jsonrpc":"2.0","id":7,"method":"session.cancel","params":{"session_id":"missing"}}"#,
        )
        .await;
        assert_eq!(out[0]["id"], 7);
        assert_eq!(out[0]["result"]["cancelled"], false);
        assert_eq!(out[0]["result"]["session_id"], "missing");
    }

    #[tokio::test]
    async fn fast_toggle_uses_current_session_tier() {
        let (tx, _rx) = mpsc::unbounded_channel::<String>();
        let root = test_project("fast-toggle");
        let state = DaemonState::new(
            root.clone(),
            PeridotConfig::default(),
            test_options(None),
            tx,
        );
        let session_id = "session-fast";
        state.sessions.lock().await.insert(
            session_id.to_string(),
            SessionEntry {
                cancel: CancelToken::new(),
                compact_request: Arc::new(AtomicBool::new(false)),
                task: None,
                spec: SessionRunSpec {
                    task: "work".to_string(),
                    mode: ExecutionMode::Execute,
                    permission: PermissionMode::Auto,
                    model: None,
                    reasoning_effort: None,
                    service_tier: None,
                    config: PeridotConfig::default(),
                },
                usage: Arc::new(StdMutex::new(LiveSessionUsage::default())),
                plan: Arc::new(StdMutex::new(LiveSessionPlan::default())),
                goal: Arc::new(StdMutex::new(LiveSessionGoal::default())),
                approval_grants: Vec::new(),
                waiting_approval: None,
            },
        );

        let first = execute_session_command(
            &state,
            Some(session_id),
            "/fast toggle",
            SlashCommand::Fast(None),
        )
        .await
        .unwrap();
        assert_eq!(first["message"], "service tier: fast");
        assert_eq!(first["state_delta"]["service_tier"], "fast");
        assert_eq!(
            state
                .sessions
                .lock()
                .await
                .get(session_id)
                .unwrap()
                .spec
                .service_tier,
            Some(Some("fast".to_string()))
        );

        let second = execute_session_command(
            &state,
            Some(session_id),
            "/fast toggle",
            SlashCommand::Fast(None),
        )
        .await
        .unwrap();
        assert_eq!(second["message"], "service tier: standard");
        assert!(second["state_delta"]["service_tier"].is_null());
        assert_eq!(
            state
                .sessions
                .lock()
                .await
                .get(session_id)
                .unwrap()
                .spec
                .service_tier,
            Some(None)
        );

        shutdown_sessions(&state).await;
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn approval_response_parameters_override_snapshot_and_resume_sidecar() {
        let (tx, _rx) = mpsc::unbounded_channel::<String>();
        let root = test_project("approval-override");
        let state = DaemonState::new(
            root.clone(),
            PeridotConfig::default(),
            test_options(None),
            tx,
        );
        let session_id = "session-approval";
        let sidecar = context_snapshot_path(&state, session_id)
            .parent()
            .unwrap()
            .join("pending_resume.bin");
        std::fs::create_dir_all(sidecar.parent().unwrap()).unwrap();
        std::fs::write(
            &sidecar,
            serde_json::to_vec(&ToolCall::new(
                "file_patch",
                serde_json::json!({"path":"src/lib.rs","old_text":"a","new_text":"b"}),
            ))
            .unwrap(),
        )
        .unwrap();
        let snapshot = ApprovalRequestSnapshot {
            tool_name: "file_patch".to_string(),
            reason: "file_patch requires explicit user approval".to_string(),
            parameters: serde_json::json!({"path":"src/lib.rs","old_text":"a","new_text":"b"}),
        };
        let params = serde_json::json!({
            "session_id": session_id,
            "approved": true,
            "scope": "once",
            "tool_name": "file_patch",
            "reason": "file_patch requires explicit user approval",
            "parameters": {"path":"src/lib.rs","old_text":"a","new_text":"partial"}
        });
        let params = params.as_object().unwrap();

        let overridden = approval_snapshot_from_response(&snapshot, params).unwrap();
        assert_eq!(overridden.parameters["new_text"], "partial");
        assert!(rewrite_pending_resume_parameters(
            &state,
            session_id,
            &overridden.parameters
        ));

        let bytes = std::fs::read(sidecar).unwrap();
        let call: ToolCall = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(call.parameters["new_text"], "partial");
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn interaction_respond_unknown_request_returns_not_accepted() {
        let out = dispatch_and_collect(
            r#"{"jsonrpc":"2.0","id":10,"method":"interaction.respond","params":{"request_id":"missing","answer":{"kind":"cancelled"}}}"#,
        )
        .await;
        assert_eq!(out[0]["id"], 10);
        assert_eq!(out[0]["result"]["accepted"], false);
        assert_eq!(out[0]["result"]["request_id"], "missing");
    }

    #[tokio::test]
    async fn daemon_ask_user_port_roundtrips_response() {
        let (tx, mut rx) = mpsc::unbounded_channel::<String>();
        let root = test_project("ask-user");
        let state = DaemonState::new(
            root.clone(),
            PeridotConfig::default(),
            test_options(None),
            tx,
        );
        let port = DaemonAskUserPort {
            state: state.clone(),
            session_id: "session-test".to_string(),
        };

        let ask_task = tokio::spawn(async move {
            port.ask(AskUserRequest::FreeForm {
                question: "Continue?".to_string(),
                hint: None,
                default: None,
            })
            .await
        });

        let line = tokio::time::timeout(std::time::Duration::from_secs(1), rx.recv())
            .await
            .unwrap()
            .unwrap();
        let value: Value = serde_json::from_str(&line).unwrap();
        let request_id = value["params"]["event"]["request_id"].as_str().unwrap();
        let response = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 11,
            "method": "interaction.respond",
            "params": {
                "request_id": request_id,
                "answer": { "kind": "text", "text": "yes" }
            }
        });
        dispatch_line(&state, &response.to_string()).await.unwrap();

        assert_eq!(
            ask_task.await.unwrap(),
            AskUserAnswer::Text("yes".to_string())
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn shutdown_with_id_returns_ack() {
        let out = dispatch_and_collect(r#"{"jsonrpc":"2.0","id":8,"method":"shutdown"}"#).await;
        assert_eq!(out[0]["id"], 8);
        assert_eq!(out[0]["result"]["shutdown"], true);
    }

    #[tokio::test]
    async fn malformed_json_returns_parse_error_with_null_id() {
        let out = dispatch_and_collect("not json at all").await;
        assert!(out[0]["id"].is_null());
        assert_eq!(out[0]["error"]["code"], -32700);
    }
}
