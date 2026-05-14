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

/// Appends one audit event under `.peridot/logs/audit.jsonl`.
pub fn append_audit_event(project_root: &Path, event: &AuditEvent) -> PeriResult<()> {
    let logs_dir = project_root.join(".peridot/logs");
    fs::create_dir_all(&logs_dir)
        .map_err(|err| PeriError::Tool(format!("failed to create audit log dir: {err}")))?;
    let path = logs_dir.join("audit.jsonl");
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
}
