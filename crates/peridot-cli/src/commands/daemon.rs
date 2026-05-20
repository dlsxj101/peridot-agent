//! `peridot daemon` -- JSON-RPC over stdio server.
//!
//! Speaks line-delimited JSON-RPC 2.0 (`\n` framed) so VS Code and other
//! editor clients can drive Peridot bidirectionally. Responses and
//! notifications are serialized onto a single stdout writer task so concurrent
//! session tasks cannot interleave JSON frames.

use std::collections::HashMap;
use std::io::BufRead;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::Result;
use peridot_common::{CancelToken, ExecutionMode, PeridotConfig, PermissionMode};
use peridot_core::AgentRunEvent;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::io::AsyncWriteExt;
use tokio::sync::{Mutex, mpsc, oneshot};

use crate::commands::{
    AuthProvider, read_managed_env_var, read_stored_api_key, read_stored_openai_oauth_credentials,
};
use crate::run_loop::{AgentTaskOptions, MessageBusHookup, run_task_with_events};

/// Shared daemon state cloned into per-session tasks.
#[derive(Clone)]
struct DaemonState {
    sessions: Arc<Mutex<HashMap<String, SessionEntry>>>,
    next_session_id: Arc<Mutex<u64>>,
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
    task: tokio::task::JoinHandle<()>,
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
        entry.task.abort();
    }
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
        "session.cancel" => {
            handle_session_cancel(state, request.id.unwrap_or(Value::Null), request.params).await?;
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

    let session_id = state.next_id().await;
    let cancel = CancelToken::new();
    let cancel_for_task = cancel.clone();
    let state_for_task = state.clone();
    let session_id_for_task = session_id.clone();
    let (start_tx, start_rx) = oneshot::channel::<()>();

    let handle = tokio::spawn(async move {
        let _ = start_rx.await;
        run_session_task(
            state_for_task,
            session_id_for_task,
            task,
            mode,
            permission,
            model,
            cancel_for_task,
        )
        .await;
    });

    state.sessions.lock().await.insert(
        session_id.clone(),
        SessionEntry {
            cancel,
            task: handle,
        },
    );
    emit_response(state, id, serde_json::json!({ "session_id": session_id }))?;
    let _ = start_tx.send(());
    Ok(())
}

async fn run_session_task(
    state: DaemonState,
    session_id: String,
    task: String,
    mode: ExecutionMode,
    permission: PermissionMode,
    model: Option<String>,
    cancel: CancelToken,
) {
    emit_event(
        &state,
        &session_id,
        serde_json::json!({
            "kind": "started",
            "task": task,
        }),
    );

    let mut options = (*state.run_template).clone();
    options.permission = permission;
    if let Some(model) = model {
        options.model = model;
    }

    let session_id_inner = session_id.clone();
    let state_inner = state.clone();
    let result = run_task_with_events(
        task,
        mode,
        options,
        (*state.run_config).clone(),
        state.project_root.as_ref().clone(),
        Some(cancel),
        None,
        None,
        None,
        MessageBusHookup::None,
        move |event: AgentRunEvent| {
            let value = serde_json::to_value(&event).unwrap_or(Value::Null);
            emit_event(&state_inner, &session_id_inner, value);
        },
    )
    .await;

    match result {
        Ok(summary) => emit_event(
            &state,
            &session_id,
            serde_json::json!({
                "kind": "finished",
                "stopped_reason": format!("{:?}", summary.stopped_reason),
                "turns": summary.turns.len(),
                "duration_ms": summary.duration_ms,
            }),
        ),
        Err(err) => emit_event(
            &state,
            &session_id,
            serde_json::json!({
                "kind": "error",
                "message": err.to_string(),
            }),
        ),
    }

    state.sessions.lock().await.remove(&session_id);
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

    let cancelled = if let Some(entry) = state.sessions.lock().await.get(session_id) {
        entry.cancel.cancel();
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
        let root =
            std::env::temp_dir().join(format!("peridot-daemon-test-{name}-{}", std::process::id()));
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
    async fn session_start_without_task_returns_invalid_params() {
        let out =
            dispatch_and_collect(r#"{"jsonrpc":"2.0","id":5,"method":"session.start"}"#).await;
        assert_eq!(out[0]["id"], 5);
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
                && value["params"]["event"]["kind"] == "started"
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
