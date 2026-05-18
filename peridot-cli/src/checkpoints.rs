//! Checkpoint restore helpers for TUI rollback commands.

use std::path::Path;

/// Restores the most recent file checkpoint under `.peridot/checkpoints`.
///
/// Checkpoints are consumed after a successful restore so `/undo` behaves as a
/// single-step rollback instead of repeatedly applying the same state.
pub(crate) fn restore_latest_checkpoint(project_root: &Path) -> Result<String, String> {
    let checkpoints_dir = project_root.join(".peridot/checkpoints");
    let entries =
        std::fs::read_dir(&checkpoints_dir).map_err(|_| "no checkpoints found".to_string())?;
    let mut files = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.extension().and_then(|value| value.to_str()) == Some("json"))
        .collect::<Vec<_>>();
    files.sort();
    let checkpoint_path = files
        .pop()
        .ok_or_else(|| "no checkpoints found".to_string())?;
    let bytes = std::fs::read(&checkpoint_path)
        .map_err(|err| format!("read {}: {err}", checkpoint_path.display()))?;
    let value: serde_json::Value =
        serde_json::from_slice(&bytes).map_err(|err| format!("parse checkpoint: {err}"))?;
    let relative = value
        .get("path")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| "checkpoint missing path".to_string())?;
    let path = project_root.join(relative);
    let path =
        peridot_tools::ensure_within_project(project_root, &path).map_err(|err| err.to_string())?;
    let existed = value
        .get("existed")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);
    if existed {
        let content = value
            .get("previous_content")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("");
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|err| format!("create {}: {err}", parent.display()))?;
        }
        std::fs::write(&path, content).map_err(|err| format!("write {}: {err}", path.display()))?;
    } else if path.exists() {
        std::fs::remove_file(&path).map_err(|err| format!("remove {}: {err}", path.display()))?;
    }
    let _ = std::fs::remove_file(&checkpoint_path);
    let id = value
        .get("id")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("latest");
    Ok(format!("undo: restored checkpoint {id} for {relative}"))
}
