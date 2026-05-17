//! Append-only audit logging for tool activity.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use peridot_common::{PeriError, PeriResult};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// One audit log entry.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AuditEvent {
    /// Unix timestamp in seconds.
    pub timestamp_unix: u64,
    /// Event category, such as tool.
    pub category: String,
    /// Event action, such as file_write.
    pub action: String,
    /// Whether the action completed successfully.
    pub success: bool,
    /// Human-readable summary.
    pub summary: String,
    /// Structured metadata.
    pub metadata: Value,
}

impl AuditEvent {
    /// Creates an audit event for a tool call.
    pub fn tool_call(
        action: impl Into<String>,
        success: bool,
        summary: impl Into<String>,
        metadata: Value,
    ) -> Self {
        Self {
            timestamp_unix: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|duration| duration.as_secs())
                .unwrap_or_default(),
            category: "tool".to_string(),
            action: action.into(),
            success,
            summary: summary.into(),
            metadata,
        }
    }
}

/// Soft cap on the live `audit.jsonl` size. Once a write would push the
/// file past this threshold we rotate the current file to `audit.jsonl.1`
/// (dropping any older rotation) and start a fresh active log. Picked at
/// 5 MiB so a typical week of busy agent work fits in the active file —
/// big enough that operators don't lose context between sessions, small
/// enough that grepping or tailing the journal stays snappy.
const AUDIT_ROTATE_BYTES: u64 = 5 * 1024 * 1024;

/// Appends one audit event under `.peridot/logs/audit.jsonl`. When the
/// active log grows past `AUDIT_ROTATE_BYTES` the file is rotated to
/// `audit.jsonl.1` (overwriting any previous rotation) and the new
/// entry starts a fresh log — a simple one-generation rotation that
/// keeps the journal bounded without dragging in a logging crate.
pub fn append_audit_event(project_root: &Path, event: &AuditEvent) -> PeriResult<()> {
    let logs_dir = project_root.join(".peridot/logs");
    fs::create_dir_all(&logs_dir)
        .map_err(|err| PeriError::Tool(format!("failed to create audit log dir: {err}")))?;
    let path = logs_dir.join("audit.jsonl");
    if let Ok(meta) = fs::metadata(&path)
        && meta.len() >= AUDIT_ROTATE_BYTES
    {
        let rotated = logs_dir.join("audit.jsonl.1");
        // Best-effort rotation — if the rename fails (e.g. the rotated
        // file is locked on Windows) we fall through and keep appending
        // to the existing file rather than dropping the new entry.
        let _ = fs::remove_file(&rotated);
        let _ = fs::rename(&path, &rotated);
    }
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .map_err(|err| PeriError::Tool(format!("failed to open {}: {err}", path.display())))?;
    let line = serde_json::to_string(event)
        .map_err(|err| PeriError::Parse(format!("failed to serialize audit event: {err}")))?;
    writeln!(file, "{line}")
        .map_err(|err| PeriError::Tool(format!("failed to write {}: {err}", path.display())))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn appends_audit_jsonl() {
        let root = std::env::temp_dir().join(format!("peridot-audit-{}", std::process::id()));
        let event = AuditEvent::tool_call(
            "file_write",
            true,
            "wrote file",
            serde_json::json!({"path": "src/lib.rs"}),
        );

        append_audit_event(&root, &event).unwrap();

        let content = fs::read_to_string(root.join(".peridot/logs/audit.jsonl")).unwrap();
        assert!(content.contains("\"action\":\"file_write\""));
        fs::remove_dir_all(root).unwrap();
    }

    /// Pre-seed `.peridot/logs/audit.jsonl` to just past the rotation
    /// threshold, append a new entry, and verify the old content moves
    /// to `audit.jsonl.1` while the new entry lands in a fresh active
    /// file. The new active file should be much smaller than the
    /// rotated one — that's the smoke test that rotation actually fired.
    #[test]
    fn rotates_audit_log_when_oversized() {
        let root = std::env::temp_dir()
            .join(format!("peridot-audit-rotate-{}", std::process::id()));
        let logs_dir = root.join(".peridot/logs");
        fs::create_dir_all(&logs_dir).unwrap();
        let path = logs_dir.join("audit.jsonl");
        // Pre-fill the live log with a buffer slightly larger than the
        // rotation cap so the very next append triggers a rotation.
        let payload = "x".repeat(AUDIT_ROTATE_BYTES as usize + 64);
        fs::write(&path, &payload).unwrap();

        let event = AuditEvent::tool_call(
            "git_commit",
            true,
            "first commit after rotation",
            serde_json::json!({}),
        );
        append_audit_event(&root, &event).unwrap();

        let rotated = fs::read(logs_dir.join("audit.jsonl.1")).unwrap();
        let active = fs::read_to_string(&path).unwrap();
        assert!(rotated.len() > AUDIT_ROTATE_BYTES as usize);
        assert!(active.contains("\"action\":\"git_commit\""));
        assert!(active.len() < 2_000, "active log should restart small");
        fs::remove_dir_all(root).unwrap();
    }
}
