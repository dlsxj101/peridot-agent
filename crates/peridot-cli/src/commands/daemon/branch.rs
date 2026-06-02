//! Branch snapshot and turn-DAG slash command handlers split out of the
//! daemon module. Parent (private) items are reached via `use super::*`.

use peridot_context::{BranchJournal, ContextEntry};
use serde_json::Value;
use std::path::PathBuf;

use super::*;

fn branch_journal_path(state: &DaemonState, session_id: &str) -> PathBuf {
    state
        .project_root
        .join(".peridot")
        .join("sessions")
        .join(session_id)
        .join("branches.json")
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

pub(super) fn handle_command_branch_save(
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

pub(super) fn handle_command_branch_restore(
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

pub(super) fn handle_command_branch_list(
    state: &DaemonState,
    raw_command: &str,
) -> Result<Value, String> {
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

pub(super) fn handle_command_branch_picker(
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

pub(super) fn handle_command_branch_turn(
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

pub(super) fn handle_command_branch_tree(
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

pub(super) fn handle_command_branch_switch(
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
