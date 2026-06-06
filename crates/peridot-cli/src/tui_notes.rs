//! Interactive `/notes` slash-command handlers.
//!
//! Lists or clears the active session's notes (persisted by the `note_*`
//! tools) and keeps the Status side panel's note summary in sync. Split out of
//! `main.rs`; the `apply_session_command` dispatcher calls these.

use std::path::Path;

use peridot_tui::TuiState;

use crate::commands::{self, read_session_notes};

pub(crate) fn handle_notes_list(state: &mut TuiState, project_root: &Path, last: Option<usize>) {
    let session_id = state.current_session_id.clone();
    if session_id.is_empty() {
        state.push_error("notes: no active session".to_string());
        return;
    }
    let (notes, total) = match read_session_notes(project_root, &session_id, last) {
        Ok(result) => result,
        Err(err) => {
            state.push_error(format!("notes: failed to read session notes: {err}"));
            return;
        }
    };
    let latest = notes
        .last()
        .and_then(|note| note["text"].as_str())
        .map(ToString::to_string);
    state.set_note_summary(total, latest);
    if notes.is_empty() {
        state.push_transcript(format!("notes: none for {session_id}"));
        return;
    }
    let mut lines = vec![format!(
        "notes: {} of {} for {session_id}",
        notes.len(),
        total
    )];
    for note in notes {
        let ts = note["ts"].as_u64().unwrap_or_default();
        let text = note["text"].as_str().unwrap_or("");
        lines.push(format!("  [{ts}] {text}"));
    }
    state.push_transcript(lines.join("\n"));
}

pub(crate) fn handle_notes_clear(state: &mut TuiState, project_root: &Path) {
    let session_id = state.current_session_id.clone();
    if session_id.is_empty() {
        state.push_error("notes: no active session".to_string());
        return;
    }
    match commands::clear_session_notes(project_root, &session_id) {
        Ok(true) => {
            state.clear_note_summary();
            state.push_transcript(format!("notes: cleared for {session_id}"));
        }
        Ok(false) => {
            state.clear_note_summary();
            state.push_transcript(format!("notes: none for {session_id}"));
        }
        Err(err) => state.push_error(format!("notes: failed to clear session notes: {err}")),
    }
}
