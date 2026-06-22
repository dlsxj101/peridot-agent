//! Session-lifecycle slash command handlers (`/session new|list|prune|
//! count|search|show|locate|resume|replay|delete|close|switch|remove|
//! rename|save`, `/clear`) and their session-target resolution /
//! persistence helpers, split out of the daemon module. Shared helpers
//! (command_result family, update_session_spec, apply_session_state_delta,
//! context-snapshot helpers, emit_*) stay in the parent and are reached
//! via `use super::*`.

use peridot_memory::{SessionLifecycle, SessionRecord, SessionSummary};
use serde_json::Value;

use super::*;

pub(super) async fn handle_command_session_new(
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

pub(super) async fn handle_command_session_list(
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

pub(super) async fn handle_command_session_prune(
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

pub(super) fn handle_command_session_count(
    state: &DaemonState,
    raw_command: &str,
) -> Result<Value, String> {
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

pub(super) async fn handle_command_clear(
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

pub(super) fn handle_command_session_search(
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

pub(super) async fn handle_command_session_show(
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

pub(super) async fn handle_command_session_locate(
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

pub(super) async fn handle_command_session_resume(
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

pub(super) async fn handle_command_session_replay(
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

pub(super) async fn handle_command_session_delete(
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

pub(super) async fn handle_command_session_close(
    state: &DaemonState,
    raw_command: &str,
    target: &str,
) -> Result<Value, String> {
    handle_command_session_remove(state, raw_command, target, "session_close", "Session Close")
        .await
}

pub(super) async fn handle_command_session_switch(
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

pub(super) async fn handle_command_session_remove(
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

pub(super) async fn handle_command_session_rename(
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

pub(super) async fn resolve_session_target_id(
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
            .unwrap_or_else(std::sync::PoisonError::into_inner)
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

pub(super) async fn handle_command_session_save(
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
