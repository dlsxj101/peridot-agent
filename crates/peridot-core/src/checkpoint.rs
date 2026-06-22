//! Pre-mutation file checkpoints.
//!
//! Before a mutating file tool (`file_write` / `file_patch`) runs, the harness
//! snapshots the target file's previous content to `.peridot/checkpoints/` so
//! the change can be rendered as a diff and rolled back. Split out of
//! `agent.rs` so the on-disk checkpoint format lives in one place.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use peridot_common::{PeriError, PeriResult};

/// Process-wide monotonic counter that disambiguates checkpoint ids.
///
/// The wall-clock nanos prefix alone is not unique: two checkpoints created
/// back-to-back (e.g. within one multi-tool batch) can read the same
/// `SystemTime::now()` value and collide, overwriting the earlier rollback
/// record on disk. Appending a strictly increasing counter guarantees a
/// distinct id per call for the lifetime of the process.
static CHECKPOINT_SEQ: AtomicU64 = AtomicU64::new(0);

/// Snapshot captured immediately before a mutating file tool runs.
///
/// Carries the previous file content (for diff rendering and rollback)
/// alongside the persisted checkpoint id (for audit-log correlation)
/// and the absolute path the tool will mutate (so the caller can re-read
/// the new content without re-walking `params`).
#[derive(Clone, Debug)]
pub(crate) struct FileCheckpoint {
    pub(crate) id: String,
    pub(crate) relative_path: String,
    pub(crate) absolute_path: PathBuf,
    pub(crate) previous_content: Option<String>,
}

/// Writes a pre-mutation checkpoint for a `file_write` / `file_patch` call and
/// returns it. Returns `Ok(None)` for any other tool, or when the call carries
/// no `path`. The previous content (if the file existed) is both persisted to
/// `.peridot/checkpoints/<id>.json` and returned on the [`FileCheckpoint`].
pub(crate) fn write_file_checkpoint(
    project_root: &std::path::Path,
    tool_name: &str,
    params: &serde_json::Value,
) -> PeriResult<Option<FileCheckpoint>> {
    if !matches!(tool_name, "file_write" | "file_patch") {
        return Ok(None);
    }
    let Some(relative) = params.get("path").and_then(serde_json::Value::as_str) else {
        return Ok(None);
    };
    let path = project_root.join(relative);
    let path = peridot_tools::ensure_within_project(project_root, &path)?;
    let existed = path.exists();
    let previous_content = if existed {
        Some(std::fs::read_to_string(&path).map_err(|err| {
            PeriError::Tool(format!(
                "failed to read checkpoint source {}: {err}",
                path.display()
            ))
        })?)
    } else {
        None
    };
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();
    let seq = CHECKPOINT_SEQ.fetch_add(1, Ordering::Relaxed);
    let id = format!("{nanos}-{seq}-{tool_name}");
    let checkpoints_dir = project_root.join(".peridot/checkpoints");
    std::fs::create_dir_all(&checkpoints_dir).map_err(|err| {
        PeriError::Tool(format!(
            "failed to create checkpoint dir {}: {err}",
            checkpoints_dir.display()
        ))
    })?;
    let checkpoint = serde_json::json!({
        "id": id,
        "tool_name": tool_name,
        "path": relative,
        "existed": existed,
        "previous_content": previous_content,
    });
    let checkpoint_path = checkpoints_dir.join(format!("{id}.json"));
    std::fs::write(
        &checkpoint_path,
        serde_json::to_vec_pretty(&checkpoint)
            .map_err(|err| PeriError::Parse(format!("failed to serialize checkpoint: {err}")))?,
    )
    .map_err(|err| {
        PeriError::Tool(format!(
            "failed to write checkpoint {}: {err}",
            checkpoint_path.display()
        ))
    })?;
    Ok(Some(FileCheckpoint {
        id,
        relative_path: relative.to_string(),
        absolute_path: path,
        previous_content,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn write_file_checkpoint_captures_previous_file_content() {
        let root =
            std::env::temp_dir().join(format!("peridot-file-checkpoint-{}", std::process::id()));
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(root.join("src/lib.rs"), "old").unwrap();

        let checkpoint = write_file_checkpoint(
            &root,
            "file_patch",
            &serde_json::json!({"path": "src/lib.rs"}),
        )
        .unwrap()
        .unwrap();
        assert_eq!(checkpoint.relative_path, "src/lib.rs");
        assert_eq!(checkpoint.previous_content.as_deref(), Some("old"));
        let serialised = std::fs::read_to_string(
            root.join(".peridot/checkpoints")
                .join(format!("{}.json", checkpoint.id)),
        )
        .unwrap();
        let value: serde_json::Value = serde_json::from_str(&serialised).unwrap();

        assert_eq!(value["path"], "src/lib.rs");
        assert_eq!(value["existed"], true);
        assert_eq!(value["previous_content"], "old");
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn back_to_back_checkpoints_have_distinct_ids() {
        let root = std::env::temp_dir().join(format!(
            "peridot-file-checkpoint-distinct-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(root.join("src/lib.rs"), "old").unwrap();

        let params = serde_json::json!({"path": "src/lib.rs"});
        let first = write_file_checkpoint(&root, "file_patch", &params)
            .unwrap()
            .unwrap();
        let second = write_file_checkpoint(&root, "file_patch", &params)
            .unwrap()
            .unwrap();

        // Two checkpoints created back-to-back (potentially sharing a nanos
        // reading) must not collide and overwrite each other's record.
        assert_ne!(first.id, second.id);
        assert!(
            root.join(".peridot/checkpoints")
                .join(format!("{}.json", first.id))
                .exists()
        );
        assert!(
            root.join(".peridot/checkpoints")
                .join(format!("{}.json", second.id))
                .exists()
        );
        std::fs::remove_dir_all(root).unwrap();
    }
}
