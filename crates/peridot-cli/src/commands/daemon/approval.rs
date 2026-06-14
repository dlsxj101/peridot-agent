//! Interaction (`interaction.respond`) and approval (`approval.respond`)
//! RPC handlers plus the approval-grant scope/config plumbing, split out
//! of the daemon module. Shared helpers (`context_snapshot_path`,
//! `emit_event`, `read/write_context_snapshot`) stay in the parent and
//! are reached via `use super::*`.

use peridot_common::{AskUserAnswer, CancelToken, PeridotConfig};
use serde_json::Value;
use std::path::Path;

use super::*;

pub(super) async fn handle_interaction_respond(
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
        let sender = state
            .ask_user_pending
            .lock()
            .expect("daemon mutex (ask_user_pending) poisoned")
            .remove(request_id);
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

pub(super) async fn handle_approval_respond(
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
    if !peridot_common::is_valid_session_id(session_id) {
        emit_error(
            state,
            id,
            -32602,
            format!("invalid session_id: {session_id:?}"),
        )?;
        return Ok(());
    }
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

pub(super) fn clear_pending_ask_user_for_session(state: &DaemonState, session_id: &str) {
    let prefix = format!("{session_id}:");
    let mut pending = state
        .ask_user_pending
        .lock()
        .expect("daemon mutex (ask_user_pending) poisoned");
    pending.retain(|request_id, _| !request_id.starts_with(&prefix));
}

pub(super) async fn mark_session_waiting_approval(
    state: &DaemonState,
    session_id: &str,
    approval: Option<ApprovalRequestSnapshot>,
) {
    if let Some(entry) = state.sessions.lock().await.get_mut(session_id) {
        entry.task = None;
        entry.waiting_approval = approval;
    }
}

pub(super) async fn apply_session_approval_grants(
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

pub(super) fn approval_snapshot_from_response(
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

pub(super) fn rewrite_pending_resume_parameters(
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

pub(super) fn is_approval_required_error(err: &anyhow::Error) -> bool {
    err.to_string().contains("requires explicit user approval")
}
