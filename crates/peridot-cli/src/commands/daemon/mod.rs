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
mod branch;
mod codemap;
mod mcp;
mod notes;
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
                *approval_snapshot_for_events.lock().unwrap() = Some(ApprovalRequestSnapshot {
                    tool_name: tool_name.clone(),
                    reason: reason.clone(),
                    parameters: parameters.clone(),
                    risk_class: risk_class.clone(),
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
            let approval = approval_snapshot.lock().unwrap().clone();
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
            handle_command_session_new(state, raw_command, task).await
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
        SlashCommand::Attach(path) => handle_command_attach(state, session_id, raw_command, &path),
        SlashCommand::Attachments => handle_command_attachments(state, session_id, raw_command),
        SlashCommand::Detach(path) => handle_command_detach(state, session_id, raw_command, &path),
        SlashCommand::Export(artifacts) => {
            handle_command_export(state, session_id, raw_command, &artifacts)
        }
        SlashCommand::SessionList => handle_command_session_list(state, raw_command, None).await,
        SlashCommand::SessionListStatus(status) => {
            handle_command_session_list(state, raw_command, Some(&status)).await
        }
        SlashCommand::SessionPrune {
            status,
            older_than_days,
            dry_run,
        } => {
            handle_command_session_prune(
                state,
                raw_command,
                status.as_deref(),
                older_than_days,
                dry_run,
            )
            .await
        }
        SlashCommand::SessionCount => handle_command_session_count(state, raw_command),
        SlashCommand::SessionSearch(query) => {
            handle_command_session_search(state, raw_command, &query)
        }
        SlashCommand::SessionShow(target) => {
            handle_command_session_show(state, raw_command, &target).await
        }
        SlashCommand::SessionLocate(target) => {
            handle_command_session_locate(state, raw_command, &target).await
        }
        SlashCommand::SessionResume(target) => {
            handle_command_session_resume(state, raw_command, &target).await
        }
        SlashCommand::SessionReplay { target, last } => {
            handle_command_session_replay(state, raw_command, &target, last).await
        }
        SlashCommand::SessionExport { target, artifacts } => {
            handle_command_session_export(state, raw_command, &target, &artifacts).await
        }
        SlashCommand::SessionImport { from, id, force } => {
            handle_command_session_import(state, raw_command, &from, id.as_deref(), force).await
        }
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

async fn handle_command_session_new(
    state: &DaemonState,
    raw_command: &str,
    task: Option<String>,
) -> Result<Value, String> {
    let trimmed_task = task
        .as_deref()
        .map(str::trim)
        .filter(|task| !task.is_empty());
    let session_id = state.next_id().await;
    let mut record = SessionRecord::new(&session_id, state.project_root.as_ref().clone());
    record.summary = trimmed_task
        .map(session_title_from_task)
        .unwrap_or_else(|| "new session".to_string());
    record.last_task = trimmed_task.map(str::to_string);
    let store = MemoryStore::new(state.project_root.join(".peridot/memory.db"));
    store
        .save_session_record(&record)
        .map_err(|err| format!("failed to save new session record: {err}"))?;
    let title = record_title(&record);
    store
        .save_session(&SessionSummary {
            id: record.id.clone(),
            summary: title.clone(),
        })
        .map_err(|err| format!("failed to save legacy session summary: {err}"))?;
    emit_session_list_changed(state).await;
    Ok(serde_json::json!({
        "kind": "session_new",
        "title": "New Session",
        "message": if trimmed_task.is_some() {
            "session new: opening and starting task"
        } else {
            "session new: opened"
        },
        "severity": "info",
        "command": raw_command,
        "task": trimmed_task,
        "session_id": session_id,
        "session_title": title,
        "summary": record.summary,
        "status": format!("{:?}", record.status).to_ascii_lowercase(),
        "running": false,
        "updated_at_unix": record.updated_at_unix,
        "total_tokens": record.total_tokens,
        "total_cost_usd": record.total_cost_usd,
        "turns_used": record.turns_used,
        "has_task": trimmed_task.is_some(),
    }))
}

async fn handle_command_session_list(
    state: &DaemonState,
    raw_command: &str,
    status_filter: Option<&str>,
) -> Result<Value, String> {
    let result = session_list_result(state).await;
    let target_status = status_filter.map(|status| status.trim().to_ascii_lowercase());
    let sessions: Vec<Value> = result["sessions"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .filter(|session| {
            target_status.as_ref().is_none_or(|target| {
                session["status"]
                    .as_str()
                    .is_some_and(|status| status == target)
            })
        })
        .collect();
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
        "message": session_list_message(items.len(), target_status.as_deref()),
        "severity": "info",
        "command": raw_command,
        "items": items,
        "sessions": sessions,
        "status_filter": target_status,
        "total": items.len(),
    }))
}

fn session_list_message(total: usize, status_filter: Option<&str>) -> String {
    match (total, status_filter) {
        (0, Some(status)) => format!("sessions ({status}): <none>"),
        (_, Some(status)) => format!("sessions ({status}): {total} total"),
        (0, None) => "sessions: <none>".to_string(),
        (_, None) => format!("sessions: {total} total"),
    }
}

async fn handle_command_session_prune(
    state: &DaemonState,
    raw_command: &str,
    status_filter: Option<&str>,
    older_than_days: Option<u64>,
    dry_run: bool,
) -> Result<Value, String> {
    let store = MemoryStore::new(state.project_root.join(".peridot/memory.db"));
    let result = prune_session_records(
        &store,
        &state.project_root,
        status_filter,
        older_than_days,
        dry_run,
    )
    .map_err(|err| format!("failed to prune sessions: {err}"))?;
    if !result.dry_run && !result.removed.is_empty() {
        let removed: std::collections::BTreeSet<&str> =
            result.removed.iter().map(String::as_str).collect();
        state
            .sessions
            .lock()
            .await
            .retain(|id, _| !removed.contains(id.as_str()));
        emit_session_list_changed(state).await;
    }
    let affected = if result.dry_run {
        result.considered.len()
    } else {
        result.removed.len()
    };
    let message = if result.dry_run {
        format!("session prune (dry-run): {affected} matching session(s)")
    } else {
        format!("session prune: removed {affected} session(s)")
    };
    let items: Vec<Value> = if result.dry_run {
        result
            .considered
            .iter()
            .map(|id| serde_json::json!({"label": id, "detail": "would remove"}))
            .collect()
    } else {
        result
            .removed
            .iter()
            .map(|id| serde_json::json!({"label": id, "detail": "removed"}))
            .collect()
    };
    Ok(serde_json::json!({
        "kind": "session_prune",
        "title": "Session Prune",
        "message": message,
        "severity": "info",
        "command": raw_command,
        "dry_run": result.dry_run,
        "considered": result.considered,
        "removed": result.removed,
        "status_filter": result.status_filter,
        "older_than_days": result.older_than_days,
        "items": items,
        "total": affected,
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

fn handle_command_session_search(
    state: &DaemonState,
    raw_command: &str,
    query: &str,
) -> Result<Value, String> {
    let result = search_session_transcript_hits(&state.project_root, query, None, Some(50))
        .map_err(|err| format!("session search failed: {err}"))?;
    let items: Vec<Value> = result
        .hits
        .iter()
        .map(|hit| {
            serde_json::json!({
                "label": format!("{}[{}] {}", hit.session, hit.index, hit.kind),
                "detail": hit.text,
                "source": hit.kind,
                "session_id": hit.session,
                "text": hit.text,
            })
        })
        .collect();
    Ok(serde_json::json!({
        "kind": "session_search",
        "title": "Session Search",
        "message": if items.is_empty() {
            format!("session search: no matches for '{}'", result.query)
        } else {
            format!("session search: {} match(es) for '{}'", result.total, result.query)
        },
        "severity": "info",
        "command": raw_command,
        "query": result.query,
        "items": items,
        "hits": result.hits,
        "total": result.total,
        "truncated": result.truncated,
    }))
}

async fn handle_command_session_show(
    state: &DaemonState,
    raw_command: &str,
    target: &str,
) -> Result<Value, String> {
    let session_id = resolve_session_target_id(state, target)
        .await?
        .unwrap_or_else(|| target.to_string());
    let summary = match session_show_summary(&state.project_root, &session_id) {
        Ok(summary) => summary,
        Err(err) => {
            return Ok(serde_json::json!({
                "kind": "session_show",
                "title": "Session Show",
                "message": format!("session show: {target} not found ({err})"),
                "severity": "error",
                "command": raw_command,
                "target": target,
                "session_id": session_id,
                "found": false,
            }));
        }
    };
    let session_title = summary
        .session
        .as_ref()
        .map(|session| session.summary.as_str())
        .or_else(|| {
            summary.record.as_ref().and_then(|record| {
                (!record.summary.trim().is_empty())
                    .then_some(record.summary.as_str())
                    .or(record.last_task.as_deref())
            })
        })
        .unwrap_or(summary.id.as_str())
        .to_string();
    let status = summary
        .record
        .as_ref()
        .map(|record| format!("{:?}", record.status).to_ascii_lowercase())
        .unwrap_or_else(|| "idle".to_string());
    let workspace = summary
        .record
        .as_ref()
        .map(|record| record.workspace_root.display().to_string());
    let total_tokens = summary
        .record
        .as_ref()
        .map(|record| record.total_tokens)
        .unwrap_or_default();
    let total_cost_usd = summary
        .record
        .as_ref()
        .map(|record| record.total_cost_usd)
        .unwrap_or_default();
    let turns_used = summary
        .record
        .as_ref()
        .map(|record| record.turns_used)
        .unwrap_or_default();
    let last_task = summary
        .record
        .as_ref()
        .and_then(|record| record.last_task.clone());
    let worktree_branch = summary
        .record
        .as_ref()
        .and_then(|record| record.worktree_branch.clone());
    let id = summary.id.clone();
    let session = summary.session.clone();
    let record = summary.record.clone();
    let summary_text = session_title.clone();
    let workspace_detail = workspace.clone().unwrap_or_else(|| "<unknown>".to_string());
    let attachment_count = summary.attachment_paths.len();
    let attachment_paths = summary.attachment_paths.clone();
    let mut items = vec![
        serde_json::json!({ "label": "session", "detail": id.clone() }),
        serde_json::json!({ "label": "title", "detail": session_title.clone() }),
        serde_json::json!({ "label": "status", "detail": status.clone() }),
        serde_json::json!({ "label": "workspace", "detail": workspace_detail }),
        serde_json::json!({ "label": "tokens", "detail": total_tokens.to_string() }),
        serde_json::json!({ "label": "cost", "detail": format!("${:.4}", total_cost_usd) }),
        serde_json::json!({ "label": "turns", "detail": turns_used.to_string() }),
        serde_json::json!({ "label": "notes", "detail": summary.notes_count.to_string() }),
        serde_json::json!({ "label": "attachments", "detail": attachment_count.to_string() }),
    ];
    if let Some(note) = summary.last_note.as_deref() {
        items.push(serde_json::json!({
            "label": "latest note",
            "detail": note,
            "text": note,
            "source": "note",
        }));
    }
    for path in &attachment_paths {
        items.push(serde_json::json!({
            "label": path,
            "path": path,
            "source": "attachment",
        }));
    }
    Ok(serde_json::json!({
        "kind": "session_show",
        "title": "Session Show",
        "message": format!("session show: {id} ({status})"),
        "severity": "info",
        "command": raw_command,
        "target": target,
        "session_id": id.clone(),
        "session_title": session_title.clone(),
        "summary": summary_text,
        "session": session,
        "record": record,
        "status": status.clone(),
        "workspace": workspace.clone(),
        "last_task": last_task,
        "worktree_branch": worktree_branch,
        "total_tokens": total_tokens,
        "total_cost_usd": total_cost_usd,
        "turns_used": turns_used,
        "notes_count": summary.notes_count,
        "last_note": summary.last_note,
        "attachment_count": attachment_count,
        "attachment_paths": attachment_paths,
        "found": true,
        "items": items,
    }))
}

async fn handle_command_session_locate(
    state: &DaemonState,
    raw_command: &str,
    target: &str,
) -> Result<Value, String> {
    let session_id = resolve_session_target_id(state, target)
        .await?
        .unwrap_or_else(|| target.to_string());
    let located = session_locate(&state.project_root, &session_id);
    Ok(serde_json::json!({
        "kind": "session_locate",
        "title": "Session Locate",
        "message": if located.exists {
            format!("session locate: {}", located.path)
        } else {
            format!("session locate: {} (not present)", located.path)
        },
        "severity": if located.exists { "info" } else { "error" },
        "command": raw_command,
        "target": target,
        "session_id": located.id,
        "path": located.path,
        "exists": located.exists,
        "items": [
            { "label": "session", "detail": located.id },
            { "label": "directory", "path": located.path, "detail": if located.exists { "present" } else { "not present" } },
        ],
    }))
}

async fn handle_command_session_resume(
    state: &DaemonState,
    raw_command: &str,
    target: &str,
) -> Result<Value, String> {
    let session_id = resolve_session_target_id(state, target)
        .await?
        .unwrap_or_else(|| target.to_string());
    let resume = session_resume_summary(&state.project_root, &session_id)
        .map_err(|err| format!("session resume: {target} not found ({err})"))?;
    Ok(serde_json::json!({
        "kind": "start_task",
        "title": "Session Resume",
        "message": format!("session resume: starting {}", resume.id),
        "severity": "info",
        "command": raw_command,
        "target": target,
        "session_id": resume.id,
        "summary": resume.summary,
        "task": resume.resume_task,
        "label": "session resume",
        "items": [
            { "label": "session", "detail": resume.id },
            { "label": "summary", "detail": resume.summary },
        ],
    }))
}

async fn handle_command_session_replay(
    state: &DaemonState,
    raw_command: &str,
    target: &str,
    last: Option<usize>,
) -> Result<Value, String> {
    let session_id = resolve_session_target_id(state, target)
        .await?
        .unwrap_or_else(|| target.to_string());
    let replay =
        match crate::commands::session_replay_summary(&state.project_root, &session_id, last) {
            Ok(replay) => replay,
            Err(err) => {
                return Ok(serde_json::json!({
                    "kind": "session_replay",
                    "title": "Session Replay",
                    "message": format!("session replay: {target} not found ({err})"),
                    "severity": "error",
                    "command": raw_command,
                    "target": target,
                    "session_id": session_id,
                    "found": false,
                }));
            }
        };
    let items: Vec<Value> = replay
        .timeline
        .iter()
        .map(|entry| {
            serde_json::json!({
                "label": entry.marker,
                "source": entry.source,
                "detail": entry.text,
                "ts": entry.ts,
            })
        })
        .collect();
    let suffix = if replay.truncated {
        format!(
            " (showing {} of {} timeline entries)",
            replay.timeline.len(),
            replay.timeline_total
        )
    } else {
        String::new()
    };
    Ok(serde_json::json!({
        "kind": "session_replay",
        "title": "Session Replay",
        "message": format!("session replay: {} timeline entr{}{}", replay.timeline.len(), if replay.timeline.len() == 1 { "y" } else { "ies" }, suffix),
        "severity": "info",
        "command": raw_command,
        "target": target,
        "session_id": replay.id,
        "entries": replay.entries,
        "timeline": replay.timeline,
        "items": items,
        "total": replay.total,
        "timeline_total": replay.timeline_total,
        "committee_total": replay.committee_total,
        "truncated": replay.truncated,
        "found": true,
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
    let summary = session
        .and_then(|item| item["summary"].as_str())
        .unwrap_or(title);
    let updated_at_unix = session
        .and_then(|item| item["updated_at_unix"].as_u64())
        .unwrap_or(0);
    let total_tokens = session
        .and_then(|item| item["total_tokens"].as_u64())
        .unwrap_or(0);
    let total_cost_usd = session
        .and_then(|item| item["total_cost_usd"].as_f64())
        .unwrap_or(0.0);
    let turns_used = session
        .and_then(|item| item["turns_used"].as_u64())
        .unwrap_or(0);
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
        "summary": summary,
        "updated_at_unix": updated_at_unix,
        "total_tokens": total_tokens,
        "total_cost_usd": total_cost_usd,
        "turns_used": turns_used,
        "switched": true,
        "items": [
            { "label": "session", "detail": session_id },
            { "label": "title", "detail": title },
            { "label": "status", "detail": status },
            { "label": "tokens", "detail": total_tokens.to_string() },
            { "label": "cost", "detail": format!("${:.4}", total_cost_usd) },
            { "label": "turns", "detail": turns_used.to_string() },
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
    let sessions = if renamed {
        Some(session_list_result(state).await)
    } else {
        None
    };
    let session = sessions
        .as_ref()
        .and_then(|list| list["sessions"].as_array())
        .and_then(|items| items.iter().find(|item| item["id"] == session_id));
    let summary = session
        .and_then(|item| item["summary"].as_str())
        .unwrap_or(title);
    let status = session
        .and_then(|item| item["status"].as_str())
        .unwrap_or("idle");
    let running = session
        .and_then(|item| item["running"].as_bool())
        .unwrap_or(false);
    let updated_at_unix = session
        .and_then(|item| item["updated_at_unix"].as_u64())
        .unwrap_or(0);
    let total_tokens = session
        .and_then(|item| item["total_tokens"].as_u64())
        .unwrap_or(0);
    let total_cost_usd = session
        .and_then(|item| item["total_cost_usd"].as_f64())
        .unwrap_or(0.0);
    let turns_used = session
        .and_then(|item| item["turns_used"].as_u64())
        .unwrap_or(0);
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
        "summary": summary,
        "status": status,
        "running": running,
        "updated_at_unix": updated_at_unix,
        "total_tokens": total_tokens,
        "total_cost_usd": total_cost_usd,
        "turns_used": turns_used,
        "renamed": renamed,
        "items": [
            { "label": "session", "detail": session_id },
            { "label": "title", "detail": title },
            { "label": "renamed", "detail": renamed.to_string() },
            { "label": "tokens", "detail": total_tokens.to_string() },
            { "label": "cost", "detail": format!("${:.4}", total_cost_usd) },
            { "label": "turns", "detail": turns_used.to_string() },
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
        approval::clear_pending_ask_user_for_session(state, session_id);
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
    let (notes_count, last_note) =
        crate::commands::read_notes_summary(&state.project_root, session_id);
    let attachment_paths = session_attachment_paths(state, session_id);
    let attachment_count = attachment_paths.len();
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
        "notes_count": notes_count,
        "last_note": last_note,
        "attachment_count": attachment_count,
        "attachment_paths": attachment_paths,
        "items": [
            { "label": "session", "detail": session_id },
            { "label": "status", "detail": format!("{:?}", record.status).to_ascii_lowercase() },
            { "label": "tokens", "detail": record.total_tokens.to_string() },
            { "label": "cost", "detail": format!("${:.4}", record.total_cost_usd) },
            { "label": "notes", "detail": notes_count.to_string() },
            { "label": "attachments", "detail": attachment_count.to_string() },
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
    export_session_id(state, &session_id, raw_command, artifacts, None)
}

async fn handle_command_session_export(
    state: &DaemonState,
    raw_command: &str,
    target: &str,
    artifacts: &[ExportArtifact],
) -> Result<Value, String> {
    let session_id = resolve_session_target_id(state, target)
        .await?
        .unwrap_or_else(|| target.to_string());
    export_session_id(state, &session_id, raw_command, artifacts, Some(target))
}

async fn handle_command_session_import(
    state: &DaemonState,
    raw_command: &str,
    from: &str,
    id: Option<&str>,
    force: bool,
) -> Result<Value, String> {
    let store = MemoryStore::new(state.project_root.join(".peridot/memory.db"));
    let result = crate::commands::import_session_artifacts(
        &store,
        &state.project_root,
        &PathBuf::from(from),
        id,
        force,
    )
    .map_err(|err| err.to_string())?;
    emit_session_list_changed(state).await;
    let items: Vec<Value> = result
        .files
        .iter()
        .map(|file| {
            serde_json::json!({
                "source": "file",
                "label": file,
            })
        })
        .collect();
    let (notes_count, last_note) =
        crate::commands::read_notes_summary(&state.project_root, &result.id);
    let attachment_paths = session_attachment_paths(state, &result.id);
    let attachment_count = attachment_paths.len();
    Ok(serde_json::json!({
        "kind": "session_import",
        "title": "Session Import",
        "message": format!("session import: imported {} from {} into {}", result.id, result.source, result.destination),
        "severity": "info",
        "command": raw_command,
        "id": result.id,
        "session_id": result.id,
        "source": result.source,
        "destination": result.destination,
        "files": result.files,
        "notes_count": notes_count,
        "last_note": last_note,
        "attachment_count": attachment_count,
        "attachment_paths": attachment_paths,
        "items": items,
        "total": items.len(),
    }))
}

fn export_session_id(
    state: &DaemonState,
    session_id: &str,
    raw_command: &str,
    artifacts: &[ExportArtifact],
    target: Option<&str>,
) -> Result<Value, String> {
    let selected = map_export_artifacts(artifacts);
    let out_dir = default_session_export_dir(&state.project_root, session_id);
    let report = crate::commands::export_session_artifacts(
        &state.project_root,
        session_id,
        &out_dir,
        &selected,
        false,
    )
    .map_err(|err| err.to_string())?;
    let mut items: Vec<Value> = report
        .files
        .iter()
        .map(|file| {
            serde_json::json!({
                "source": "full_copy",
                "label": file,
                "detail": "full copy",
            })
        })
        .collect();
    items.extend(report.artifacts.iter().map(|artifact| {
        serde_json::json!({
            "source": "artifact",
            "label": artifact.path,
            "detail": format!("{} entries · {}", artifact.count, artifact.class),
        })
    }));
    Ok(serde_json::json!({
        "kind": "session_export",
        "title": "Session Artifact Export",
        "message": format!("export: wrote {} artifact file(s) to {}", report.artifacts.len(), report.destination),
        "severity": "info",
        "command": raw_command,
        "target": target,
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
