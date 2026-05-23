//! `peridot daemon` -- JSON-RPC over stdio server.
//!
//! Speaks line-delimited JSON-RPC 2.0 (`\n` framed) so VS Code and other
//! editor clients can drive Peridot bidirectionally. Responses and
//! notifications are serialized onto a single stdout writer task so concurrent
//! session tasks cannot interleave JSON frames.

use std::collections::{BTreeMap, HashMap};
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
    AgentRunEvent, AutoFixAction, SlashCommand, SlashStateDelta, StopReason, SubagentModelChange,
    parse_slash_command, slash_state_delta,
};
use peridot_git::GitManager;
use peridot_tools::AskUserPort;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::io::AsyncWriteExt;
use tokio::sync::{Mutex, mpsc, oneshot};

use crate::checkpoints::restore_latest_checkpoint;
use crate::commands::{
    AuthProvider, read_managed_env_var, read_stored_api_key, read_stored_openai_oauth_credentials,
};
use crate::run_loop::{AgentTaskOptions, MessageBusHookup, run_task_with_events};
use crate::session_router::{RouterMessageBus, SessionHandle, SessionRouter, WorkspaceIsolation};

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
    approval_grants: Vec<ApprovalGrant>,
    waiting_approval: Option<ApprovalRequestSnapshot>,
}

#[derive(Clone)]
struct SessionRunSpec {
    task: String,
    mode: ExecutionMode,
    permission: PermissionMode,
    model: Option<String>,
    reasoning_effort: Option<ReasoningEffort>,
    service_tier: Option<Option<String>>,
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
    for (_, entry) in sessions.drain() {
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
                slash_command_catalog_result(),
            )?;
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
        "session.command" => {
            handle_session_command(state, request.id.unwrap_or(Value::Null), request.params)
                .await?;
        }
        "session.cancel" => {
            handle_session_cancel(state, request.id.unwrap_or(Value::Null), request.params).await?;
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

fn slash_command_catalog_result() -> Value {
    let commands: Vec<Value> = peridot_tui::slash_command_catalog()
        .iter()
        .map(|spec| {
            serde_json::json!({
                "name": spec.name,
                "description": spec.description,
                "arg_hint": spec.arg_hint,
                "category": spec.category,
            })
        })
        .collect();
    serde_json::json!({ "commands": commands })
}

async fn handle_status(state: &DaemonState, id: Value) -> Result<()> {
    let config = state.run_config.as_ref();
    let auth = auth_status(config).await;
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
        None => state.run_config.defaults.mode,
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
    };
    let spec_for_task = spec.clone();
    let (start_tx, start_rx) = oneshot::channel::<()>();

    let handle = tokio::spawn(async move {
        let _ = start_rx.await;
        run_session_task(
            state_for_task,
            session_id_for_task,
            spec_for_task,
            cancel_for_task,
            compact_request_for_task,
        )
        .await;
    });

    state.sessions.lock().await.insert(
        session_id.clone(),
        SessionEntry {
            cancel,
            compact_request,
            task: Some(handle),
            spec,
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
    emit_response(state, id, serde_json::json!({ "session_id": session_id }))?;
    let _ = start_tx.send(());
    Ok(())
}

async fn run_session_task(
    state: DaemonState,
    session_id: String,
    spec: SessionRunSpec,
    cancel: CancelToken,
    compact_request: Arc<AtomicBool>,
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
    let mut config = (*state.run_config).clone();
    apply_session_approval_grants(&state, &session_id, &mut config).await;

    let ask_user_port = Arc::new(DaemonAskUserPort {
        state: state.clone(),
        session_id: session_id.clone(),
    });

    let context_snapshot_path = Some(context_snapshot_path(&state, &session_id));
    let approval_snapshot: Arc<std::sync::Mutex<Option<ApprovalRequestSnapshot>>> =
        Arc::new(std::sync::Mutex::new(None));
    let approval_snapshot_for_events = approval_snapshot.clone();
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
    )
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

    match execute_session_command(state, session_id.as_deref(), &command_text, command).await {
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
        SlashCommand::Note(note) => Ok(command_result(
            "note",
            "Note",
            &format!("note: {note}"),
            "info",
        )),
        SlashCommand::Lang(locale) => Ok(command_result_with_state_delta(
            "setting",
            "Language",
            &format!("language: {locale:?}"),
            "info",
            &state_delta,
        )),
        SlashCommand::Help => Ok(serde_json::json!({
            "kind": "help",
            "title": "Slash Commands",
            "message": "Use the extension picker to select a command. Commands that touch project state are executed through the Peridot daemon.",
            "severity": "info",
            "command": raw_command,
        })),
        SlashCommand::Clear => Ok(serde_json::json!({
            "kind": "client_action",
            "action": "clear",
            "title": "Clear",
            "message": "clear: transcript + context should be cleared by the client",
            "severity": "info",
            "command": raw_command,
        })),
        SlashCommand::Cost
        | SlashCommand::PlanShow
        | SlashCommand::Info
        | SlashCommand::SessionSave
        | SlashCommand::SidepanelToggle
        | SlashCommand::Collapse
        | SlashCommand::Rewind
        | SlashCommand::SessionNew(_)
        | SlashCommand::SessionSwitch(_)
        | SlashCommand::SessionClose(_)
        | SlashCommand::SessionList
        | SlashCommand::GoalStart(_)
        | SlashCommand::GoalPause
        | SlashCommand::GoalResume
        | SlashCommand::GoalClear
        | SlashCommand::GoalStatus => Ok(with_state_delta(
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
        SlashCommand::Diff => handle_command_diff(state, raw_command),
        SlashCommand::Undo => handle_command_undo(state, raw_command),
        SlashCommand::Todos => handle_command_todos(state, raw_command),
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

    let (spec, cancel_for_task, compact_request, parameters_overridden) = {
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
        (spec, cancel, compact_request, parameters_overridden)
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

fn source_label(source: &ContextSource) -> &'static str {
    match source {
        ContextSource::User => "user",
        ContextSource::Assistant => "assistant",
        ContextSource::Tool => "tool",
        ContextSource::PlanReminder => "plan",
        ContextSource::ReviewerComment => "review",
        ContextSource::External => "external",
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
            assert_eq!(
                actual["arg_hint"].as_str().unwrap_or(""),
                expected.arg_hint.unwrap_or("")
            );
        }
        assert!(commands.iter().any(|entry| entry["name"] == "/plan"));
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
                },
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
