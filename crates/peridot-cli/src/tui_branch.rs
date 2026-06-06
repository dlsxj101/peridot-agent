//! Interactive `/branch` slash-command handlers.
//!
//! Branch/limb operations on the live session's context snapshot: fork the
//! transcript at a turn (`turn`), inspect abandoned limbs (`tree`), swap to a
//! limb (`switch`), and save / restore / list named branch snapshots under
//! `.peridot/branches/`. Split out of `main.rs`; the `apply_session_command`
//! dispatcher calls these. The shared snapshot/journal path helpers and
//! `branch_snapshot_names` stay in `main.rs` (other modules use them).

use std::path::Path;

use peridot_context::{BranchJournal, ContextEntry};
use peridot_tui::TuiState;

use crate::{branch_journal_path, context_snapshot_path};

pub(crate) fn handle_branch_turn(state: &mut TuiState, project_root: &Path, turn_id: u64) {
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
    let entries: Vec<ContextEntry> = match serde_json::from_slice(&bytes) {
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
    let dropped_entries: Vec<ContextEntry> = entries[last_keep + 1..].to_vec();
    let dropped_count = dropped_entries.len();
    if !dropped_entries.is_empty() {
        let journal_path = branch_journal_path(project_root, &session_id);
        let mut journal = BranchJournal::load(&journal_path);
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

pub(crate) fn handle_branch_tree(state: &mut TuiState, project_root: &Path) {
    let session_id = state.current_session_id.clone();
    if session_id.is_empty() {
        state.push_error("branch tree: no active session id".to_string());
        return;
    }
    let journal_path = branch_journal_path(project_root, &session_id);
    let journal = BranchJournal::load(&journal_path);
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

pub(crate) fn handle_branch_switch(state: &mut TuiState, project_root: &Path, index: usize) {
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
    let mut journal = BranchJournal::load(&journal_path);
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
    let current_entries: Vec<ContextEntry> = match serde_json::from_slice(&bytes) {
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
    let current_tail: Vec<ContextEntry> = current_entries[last_keep + 1..].to_vec();
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
pub(crate) fn handle_branch_save(state: &mut TuiState, project_root: &Path, name: &str) {
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
    state.add_branch_suggestion(name);
    state.push_transcript(format!("branch: saved '{name}' from session {session_id}"));
}

/// Overwrites the active session's context snapshot with the named
/// branch's context. The TUI checks `is_agent_busy()` before
/// enqueueing, but we re-validate here so a racy command can't slip
/// past — the agent might still be inside `Finished` cleanup when the
/// queue drains, in which case the rename would race with the loop's
/// own snapshot write.
pub(crate) fn handle_branch_restore(state: &mut TuiState, project_root: &Path, name: &str) {
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
pub(crate) fn handle_branch_list(state: &mut TuiState, project_root: &Path) {
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
