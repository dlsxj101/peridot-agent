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
    PermissionMode, ReasoningEffort, ToolCall, configured_model_suggestions,
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

use crate::branch_snapshot_names;
use crate::checkpoints::restore_latest_checkpoint;
use crate::commands::{
    AuthProvider, append_session_note, clear_session_notes, move_auto_skill_to_archive,
    prune_session_records, read_managed_env_var, read_session_notes, read_stored_api_key,
    read_stored_openai_oauth_credentials, restore_archived_skill, search_session_transcript_hits,
    session_count_summary, session_locate, session_resume_summary, session_show_summary,
};
use crate::run_loop::{AgentTaskOptions, MessageBusHookup, run_task_with_events};
use crate::session_router::{RouterMessageBus, SessionHandle, SessionRouter, WorkspaceIsolation};
use crate::worktree_cleanup::reconcile_stale_worktrees;

mod approval;
mod attach;
mod branch;
mod codemap;
mod inspect;
mod mcp;
mod notes;
mod session_cmd;
mod skills;

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
    /// Filesystem watcher that invalidates the symbol-cache on file changes
    /// (feature F1). Kept alive for the daemon's lifetime via the shared Arc;
    /// `None` when a watcher couldn't be started (the cache still works,
    /// falling back to query-time staleness checks).
    _symbol_cache_watcher: Option<Arc<peridot_tools::SymbolCacheWatcher>>,
}

impl DaemonState {
    fn new(
        project_root: PathBuf,
        run_config: PeridotConfig,
        run_template: AgentTaskOptions,
        out: mpsc::UnboundedSender<String>,
    ) -> Self {
        let symbol_cache_watcher =
            peridot_tools::SymbolCacheWatcher::new(&project_root).map(Arc::new);
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
            _symbol_cache_watcher: symbol_cache_watcher,
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    risk_class: Option<String>,
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
                skills_list_result(state, skills_list_include_archived(request.params.as_ref())),
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
            approval::handle_interaction_respond(
                state,
                request.id.unwrap_or(Value::Null),
                request.params,
            )
            .await?;
        }
        "approval.respond" => {
            approval::handle_approval_respond(
                state,
                request.id.unwrap_or(Value::Null),
                request.params,
            )
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

fn skills_list_include_archived(params: Option<&Value>) -> bool {
    params
        .and_then(|params| params.get("include_archived"))
        .and_then(Value::as_bool)
        .unwrap_or(false)
}

fn skills_list_result(state: &DaemonState, include_archived: bool) -> Value {
    let store = peridot_memory::MemoryStore::new(state.project_root.join(".peridot/memory.db"));
    let skills = if include_archived {
        store
            .list_skill_records()
            .map(|records| records.into_iter().map(|record| record.skill).collect())
            .unwrap_or_default()
    } else {
        store.list_skills().unwrap_or_default()
    };
    let skills: Vec<Value> = skills
        .into_iter()
        .filter(|skill| skill.scope == "auto")
        .map(|skill| {
            serde_json::json!({
                "name": skill.name,
                "description": skill_description(&skill),
                "scope": skill.scope,
                "archived": skill.archived_at_unix > 0,
                "archived_at_unix": skill.archived_at_unix,
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
        let (notes_count, last_note) =
            crate::commands::read_notes_summary(state.project_root.as_ref(), &session.id);
        let attachment_paths = session_attachment_paths(state, &session.id);
        let attachment_count = attachment_paths.len();
        rows.insert(
            session.id.clone(),
            serde_json::json!({
                "id": session.id,
                "title": session.summary,
                "summary": session.summary,
                "status": "idle",
                "running": false,
                "updated_at_unix": 0,
                "notes_count": notes_count,
                "last_note": last_note,
                "attachment_count": attachment_count,
                "attachment_paths": attachment_paths,
            }),
        );
    }
    for record in records {
        let running =
            running_sessions.contains_key(&record.id) || record.status == SessionLifecycle::Running;
        let (notes_count, last_note) =
            crate::commands::read_notes_summary(state.project_root.as_ref(), &record.id);
        let attachment_paths = session_attachment_paths(state, &record.id);
        let attachment_count = attachment_paths.len();
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
                "notes_count": notes_count,
                "last_note": last_note,
                "attachment_count": attachment_count,
                "attachment_paths": attachment_paths,
            }),
        );
    }
    for (id, entry) in running_sessions.iter() {
        rows.entry(id.clone()).or_insert_with(|| {
            let (notes_count, last_note) =
                crate::commands::read_notes_summary(state.project_root.as_ref(), id);
            let attachment_paths = session_attachment_paths(state, id);
            let attachment_count = attachment_paths.len();
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
                "notes_count": notes_count,
                "last_note": last_note,
                "attachment_count": attachment_count,
                "attachment_paths": attachment_paths,
            })
        });
    }
    serde_json::json!({
        "sessions": rows.into_values().collect::<Vec<_>>(),
    })
}

fn session_attachment_paths(state: &DaemonState, session_id: &str) -> Vec<String> {
    read_context_snapshot(state, session_id)
        .map(|entries| {
            let mut paths = crate::commands::attachments_from_context(&entries)
                .into_iter()
                .map(|attachment| attachment.path)
                .filter(|path| !path.trim().is_empty())
                .collect::<Vec<_>>();
            paths.sort_by_key(|path| path.to_ascii_lowercase());
            paths.dedup_by(|left, right| left.eq_ignore_ascii_case(right));
            paths
        })
        .unwrap_or_default()
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
    let model_suggestions =
        configured_model_suggestions(config, Some(state.run_template.model.as_str()));
    let branch_snapshots = branch_snapshot_names(state.project_root.as_ref());
    let code_map = crate::commands::code_map_status(state.project_root.as_ref())
        .ok()
        .map(|status| codemap::code_map_status_summary(&status));
    let mcp: Vec<Value> = config
        .mcp
        .iter()
        .map(|entry| {
            serde_json::json!({
                "name": entry.name,
                "transport": entry.transport.to_string(),
            })
        })
        .collect();
    emit_response(
        state,
        id,
        serde_json::json!({
            "version": env!("CARGO_PKG_VERSION"),
            "project_root": state.project_root.as_ref(),
            "provider": config.auth.primary,
            "model": config.models.main,
            "model_suggestions": model_suggestions,
            "branch_snapshots": branch_snapshots,
            "reasoning_effort": format!("{:?}", config.models.reasoning_effort),
            "mode": format!("{:?}", config.defaults.mode),
            "permission": format!("{:?}", state.run_template.permission),
            "committee_mode": config.committee.mode.to_string(),
            "auth": auth,
            "mcp": mcp,
            "code_map": code_map,
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
    state.sessions.lock().await.get(session_id).map(|entry| {
        entry
            .usage
            .lock()
            .expect("daemon mutex (usage) poisoned")
            .clone()
    })
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
    approval::apply_session_approval_grants(&state, &session_id, &mut config).await;

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
                risk_class,
            } = &event
            {
                *approval_snapshot_for_events
                    .lock()
                    .expect("daemon mutex (approval_snapshot_for_events) poisoned") =
                    Some(ApprovalRequestSnapshot {
                        tool_name: tool_name.clone(),
                        reason: reason.clone(),
                        parameters: parameters.clone(),
                        risk_class: risk_class.clone(),
                    });
            }
            usage_for_events
                .lock()
                .expect("daemon mutex (usage_for_events) poisoned")
                .record_event(&event);
            if let AgentRunEvent::PlanUpdated { steps, current } = &event {
                *plan_for_events
                    .lock()
                    .expect("daemon mutex (plan_for_events) poisoned") = LiveSessionPlan {
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
                let approval = approval_snapshot
                    .lock()
                    .expect("daemon mutex (approval_snapshot) poisoned")
                    .clone();
                approval::mark_session_waiting_approval(&state, &session_id, approval.clone())
                    .await;
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
            let approval = approval_snapshot
                .lock()
                .expect("daemon mutex (approval_snapshot) poisoned")
                .clone();
            if approval.is_some() && approval::is_approval_required_error(&err) {
                approval::mark_session_waiting_approval(&state, &session_id, approval.clone())
                    .await;
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
    approval::clear_pending_ask_user_for_session(&state, &session_id);
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
            let mut pending = self
                .state
                .ask_user_pending
                .lock()
                .expect("daemon mutex (ask_user_pending) poisoned");
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
        approval::clear_pending_ask_user_for_session(state, session_id);
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
            let refresh_session_list = command_result_refreshes_session_list(&result);
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
            if refresh_session_list {
                emit_session_list_changed(state).await;
            }
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

fn command_result_refreshes_session_list(result: &Value) -> bool {
    match result.get("kind").and_then(Value::as_str) {
        Some("note" | "attach") => true,
        Some("notes_clear") => result
            .get("cleared")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        Some("detach") => {
            result
                .get("removed_count")
                .and_then(Value::as_u64)
                .unwrap_or_default()
                > 0
        }
        _ => false,
    }
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
        SlashCommand::GoalMode => Ok(command_result_with_state_delta(
            "setting",
            "Mode",
            "mode: goal",
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
            &format!("committee: {mode}"),
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
        SlashCommand::Note(note) => {
            notes::handle_command_note(state, session_id, raw_command, &note)
        }
        SlashCommand::Notes(last) => {
            notes::handle_command_notes(state, session_id, raw_command, last)
        }
        SlashCommand::NotesClear => {
            notes::handle_command_notes_clear(state, session_id, raw_command)
        }
        SlashCommand::Lang(locale) => Ok(command_result_with_state_delta(
            "setting",
            "Language",
            &format!("language: {locale:?}"),
            "info",
            &state_delta,
        )),
        SlashCommand::Help => Ok(handle_command_help(raw_command, None)),
        SlashCommand::SkillList => skills::handle_command_skill_list(state, raw_command),
        SlashCommand::SkillShow(name) => {
            skills::handle_command_skill_show(state, raw_command, &name)
        }
        SlashCommand::SkillSearch(query) => {
            skills::handle_command_skill_search(state, raw_command, &query)
        }
        SlashCommand::SkillArchived(query) => {
            skills::handle_command_skill_archived(state, raw_command, &query)
        }
        SlashCommand::SkillPin(name) => {
            skills::handle_command_skill_pin(state, raw_command, &name, true)
        }
        SlashCommand::SkillUnpin(name) => {
            skills::handle_command_skill_pin(state, raw_command, &name, false)
        }
        SlashCommand::SkillArchive(name) => {
            skills::handle_command_skill_archive(state, raw_command, &name)
        }
        SlashCommand::SkillRestore(name) => {
            skills::handle_command_skill_restore(state, raw_command, &name)
        }
        SlashCommand::Cost => inspect::handle_command_cost(state, session_id, raw_command).await,
        SlashCommand::Info => inspect::handle_command_info(state, session_id, raw_command).await,
        SlashCommand::PlanShow => {
            inspect::handle_command_plan_show(state, session_id, raw_command).await
        }
        SlashCommand::SessionSave => {
            session_cmd::handle_command_session_save(state, session_id, raw_command).await
        }
        SlashCommand::GoalPause
        | SlashCommand::GoalResume
        | SlashCommand::GoalClear
        | SlashCommand::GoalStatus => {
            inspect::handle_command_goal_control(state, session_id, raw_command, &command).await
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
                    Vec::new(),
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
        SlashCommand::Clear => {
            session_cmd::handle_command_clear(state, session_id, raw_command).await
        }
        SlashCommand::SidepanelToggle | SlashCommand::Collapse => Ok(with_state_delta(
            serde_json::json!({
                "kind": "client_action",
                "action": "local",
                "title": "Handled by Extension",
                "message": format!("{raw_command}: handled by the extension UI"),
                "severity": "info",
                "command": raw_command,
            }),
            &state_delta,
        )),
        SlashCommand::SessionNew(task) => {
            session_cmd::handle_command_session_new(state, raw_command, task).await
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
        SlashCommand::Todos => codemap::handle_command_todos(state, raw_command),
        SlashCommand::CodeMap => codemap::handle_command_codemap(state, raw_command, false),
        SlashCommand::CodeMapStatus => codemap::handle_command_codemap_status(state, raw_command),
        SlashCommand::CodeMapRefresh => codemap::handle_command_codemap(state, raw_command, true),
        SlashCommand::CodeMapFind(query) => {
            codemap::handle_command_codemap_find(state, raw_command, &query)
        }
        SlashCommand::CodeMapLocate(query) => {
            codemap::handle_command_codemap_locate(state, raw_command, &query)
        }
        SlashCommand::CodeMapOutline(path) => {
            codemap::handle_command_codemap_outline(state, raw_command, &path)
        }
        SlashCommand::CodeMapRefs(query) => {
            codemap::handle_command_codemap_refs(state, raw_command, &query)
        }
        SlashCommand::Attach(path) => {
            attach::handle_command_attach(state, session_id, raw_command, &path)
        }
        SlashCommand::Attachments => {
            attach::handle_command_attachments(state, session_id, raw_command)
        }
        SlashCommand::Detach(path) => {
            attach::handle_command_detach(state, session_id, raw_command, &path)
        }
        SlashCommand::Export(artifacts) => {
            attach::handle_command_export(state, session_id, raw_command, &artifacts)
        }
        SlashCommand::SessionList => {
            session_cmd::handle_command_session_list(state, raw_command, None).await
        }
        SlashCommand::SessionListStatus(status) => {
            session_cmd::handle_command_session_list(state, raw_command, Some(&status)).await
        }
        SlashCommand::SessionPrune {
            status,
            older_than_days,
            dry_run,
        } => {
            session_cmd::handle_command_session_prune(
                state,
                raw_command,
                status.as_deref(),
                older_than_days,
                dry_run,
            )
            .await
        }
        SlashCommand::SessionCount => session_cmd::handle_command_session_count(state, raw_command),
        SlashCommand::SessionSearch(query) => {
            session_cmd::handle_command_session_search(state, raw_command, &query)
        }
        SlashCommand::SessionShow(target) => {
            session_cmd::handle_command_session_show(state, raw_command, &target).await
        }
        SlashCommand::SessionLocate(target) => {
            session_cmd::handle_command_session_locate(state, raw_command, &target).await
        }
        SlashCommand::SessionResume(target) => {
            session_cmd::handle_command_session_resume(state, raw_command, &target).await
        }
        SlashCommand::SessionReplay { target, last } => {
            session_cmd::handle_command_session_replay(state, raw_command, &target, last).await
        }
        SlashCommand::SessionExport { target, artifacts } => {
            attach::handle_command_session_export(state, raw_command, &target, &artifacts).await
        }
        SlashCommand::SessionImport { from, id, force } => {
            attach::handle_command_session_import(state, raw_command, &from, id.as_deref(), force)
                .await
        }
        SlashCommand::SessionDelete(target) => {
            session_cmd::handle_command_session_delete(state, raw_command, &target).await
        }
        SlashCommand::SessionSwitch(target) => {
            session_cmd::handle_command_session_switch(state, raw_command, &target).await
        }
        SlashCommand::SessionClose(target) => {
            session_cmd::handle_command_session_close(state, raw_command, &target).await
        }
        SlashCommand::SessionRename { target, title } => {
            session_cmd::handle_command_session_rename(state, raw_command, &target, &title).await
        }
        SlashCommand::McpList => mcp::handle_command_mcp_list(state, raw_command),
        SlashCommand::McpAdd {
            name,
            transport,
            target,
        } => mcp::handle_command_mcp_add(state, raw_command, &name, &transport, &target),
        SlashCommand::McpRemove(name) => mcp::handle_command_mcp_remove(state, raw_command, &name),
        SlashCommand::McpTest(name) => {
            mcp::handle_command_mcp_test(state, raw_command, &name).await
        }
        SlashCommand::BranchSave(name) => {
            branch::handle_command_branch_save(state, session_id, raw_command, &name)
        }
        SlashCommand::BranchRestore(name) => {
            branch::handle_command_branch_restore(state, session_id, raw_command, &name)
        }
        SlashCommand::BranchList => branch::handle_command_branch_list(state, raw_command),
        SlashCommand::BranchPicker => {
            branch::handle_command_branch_picker(state, session_id, raw_command)
        }
        SlashCommand::BranchTurn(turn_id) => {
            branch::handle_command_branch_turn(state, session_id, raw_command, turn_id)
        }
        SlashCommand::BranchTree => {
            branch::handle_command_branch_tree(state, session_id, raw_command)
        }
        SlashCommand::BranchSwitch(index) => {
            branch::handle_command_branch_switch(state, session_id, raw_command, index)
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

fn context_snapshot_path(state: &DaemonState, session_id: &str) -> PathBuf {
    state
        .project_root
        .join(".peridot")
        .join("sessions")
        .join(session_id)
        .join("context.bin")
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

fn append_plan_reminder_to_context(
    state: &DaemonState,
    session_id: &str,
    content: String,
    images: Vec<peridot_llm::ImageContent>,
) -> Result<(), String> {
    let mut entries = read_context_snapshot(state, session_id).unwrap_or_default();
    entries.push(ContextEntry::trusted(ContextSource::PlanReminder, content).with_images(images));
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
mod tests;
