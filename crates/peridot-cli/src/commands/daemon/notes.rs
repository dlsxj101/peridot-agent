//! Session-notes slash command handlers (`/note`, `/notes`,
//! `/notes clear`) split out of the daemon module. The persistence
//! helpers live in `crate::commands::session`; parent (private) items
//! are reached via `use super::*`.

use crate::commands::{append_session_note, clear_session_notes, read_session_notes};
use serde_json::Value;

use super::*;

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

pub(super) fn handle_command_note(
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

pub(super) fn handle_command_notes(
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

pub(super) fn handle_command_notes_clear(
    state: &DaemonState,
    session_id: Option<&str>,
    raw_command: &str,
) -> Result<Value, String> {
    let session_id = require_session_id(session_id, "notes")?;
    let cleared = clear_session_notes(&state.project_root, &session_id)
        .map_err(|err| format!("notes: failed to clear session notes: {err}"))?;
    Ok(serde_json::json!({
        "kind": "notes_clear",
        "title": "Session Notes",
        "message": if cleared {
            format!("notes: cleared for {session_id}")
        } else {
            format!("notes: none for {session_id}")
        },
        "severity": "info",
        "command": raw_command,
        "session_id": session_id,
        "cleared": cleared,
        "total": 0,
        "items": [
            { "label": "session", "detail": session_id },
            { "label": "cleared", "detail": cleared.to_string() },
        ],
    }))
}
