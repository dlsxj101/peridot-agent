//! File-attachment and session-artifact export/import slash command
//! handlers (`/attach`, `/attachments`, `/detach`, `/export`,
//! `/session export|import`) plus the export path/id helpers, split out
//! of the daemon module. Parent (private) items and sibling submodules
//! (e.g. `session_cmd::resolve_session_target_id`) are reached via
//! `use super::*`.

use serde_json::Value;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use super::*;

pub(super) fn handle_command_attach(
    state: &DaemonState,
    session_id: Option<&str>,
    raw_command: &str,
    path: &str,
) -> Result<Value, String> {
    const MAX_ATTACHMENT_BYTES: usize = 64 * 1024;
    let session_id = require_session_id(session_id, "attach")?;
    let attachment =
        crate::commands::load_text_attachment(&state.project_root, path, MAX_ATTACHMENT_BYTES)?;
    let inlined = attachment.content.is_some();
    let path = attachment.path.clone();
    let bytes = attachment.bytes;
    let media_type = attachment
        .media_type
        .clone()
        .unwrap_or_else(|| "text/plain".to_string());
    let content = attachment.content.clone();
    let detail = if inlined {
        format!("{bytes} bytes · inlined")
    } else {
        format!("{bytes} bytes · {media_type} placeholder")
    };
    append_plan_reminder_to_context(
        state,
        &session_id,
        crate::commands::attachment_plan_reminder(&attachment),
    )?;
    Ok(serde_json::json!({
        "kind": "attach",
        "title": "Attachment",
        "message": format!("attach: added {path} ({bytes} bytes) to session context"),
        "severity": "info",
        "command": raw_command,
        "attachment": {
            "path": path,
            "bytes": bytes,
            "media_type": media_type,
            "inlined": inlined,
            "content": content,
        },
        "items": [{
            "source": "attachment",
            "label": path,
            "path": path,
            "detail": detail,
            "bytes": bytes,
            "media_type": media_type,
            "inlined": inlined,
        }],
    }))
}

pub(super) fn handle_command_attachments(
    state: &DaemonState,
    session_id: Option<&str>,
    raw_command: &str,
) -> Result<Value, String> {
    let session_id = require_session_id(session_id, "attachments")?;
    let entries = read_context_snapshot(state, &session_id)?;
    let attachments = crate::commands::attachments_from_context(&entries);
    let items: Vec<Value> = attachments
        .iter()
        .map(|attachment| {
            let mode = if attachment.inlined {
                "inlined"
            } else {
                "placeholder"
            };
            serde_json::json!({
                "source": "attachment",
                "label": attachment.path,
                "path": attachment.path,
                "detail": format!("{} bytes · {} · {}", attachment.bytes, attachment.media_type, mode),
                "bytes": attachment.bytes,
                "media_type": attachment.media_type,
                "inlined": attachment.inlined,
            })
        })
        .collect();
    Ok(serde_json::json!({
        "kind": "attachments",
        "title": "Session Attachments",
        "message": format!("attachments: {} file(s) in session context", attachments.len()),
        "severity": "info",
        "command": raw_command,
        "attachments": attachments,
        "items": items,
        "total": items.len(),
    }))
}

pub(super) fn handle_command_detach(
    state: &DaemonState,
    session_id: Option<&str>,
    raw_command: &str,
    path: &str,
) -> Result<Value, String> {
    let session_id = require_session_id(session_id, "detach")?;
    let entries = read_context_snapshot(state, &session_id)?;
    let (kept, removed) = crate::commands::detach_attachments_from_context(entries, path);
    if removed.is_empty() {
        return Ok(serde_json::json!({
            "kind": "detach",
            "title": "Detach Attachment",
            "message": format!("detach: no attachment matched {path}"),
            "severity": "info",
            "command": raw_command,
            "removed_count": 0,
            "items": [],
        }));
    }
    write_context_snapshot(state, &session_id, &kept)?;
    let remaining = crate::commands::attachments_from_context(&kept);
    let items: Vec<Value> = removed
        .iter()
        .map(|attachment| {
            serde_json::json!({
                "source": "attachment",
                "label": attachment.path,
                "path": attachment.path,
                "detail": format!("{} bytes · removed", attachment.bytes),
                "bytes": attachment.bytes,
                "media_type": attachment.media_type,
                "inlined": attachment.inlined,
            })
        })
        .collect();
    Ok(serde_json::json!({
        "kind": "detach",
        "title": "Detach Attachment",
        "message": format!("detach: removed {} attachment(s) matching {path}", removed.len()),
        "severity": "info",
        "command": raw_command,
        "removed_count": removed.len(),
        "remaining_count": remaining.len(),
        "removed": removed,
        "attachments": remaining,
        "items": items,
    }))
}

pub(super) fn handle_command_export(
    state: &DaemonState,
    session_id: Option<&str>,
    raw_command: &str,
    artifacts: &[ExportArtifact],
) -> Result<Value, String> {
    let session_id = require_session_id(session_id, "export")?;
    export_session_id(state, &session_id, raw_command, artifacts, None)
}

pub(super) async fn handle_command_session_export(
    state: &DaemonState,
    raw_command: &str,
    target: &str,
    artifacts: &[ExportArtifact],
) -> Result<Value, String> {
    let session_id = session_cmd::resolve_session_target_id(state, target)
        .await?
        .unwrap_or_else(|| target.to_string());
    export_session_id(state, &session_id, raw_command, artifacts, Some(target))
}

pub(super) async fn handle_command_session_import(
    state: &DaemonState,
    raw_command: &str,
    from: &str,
    id: Option<&str>,
    force: bool,
) -> Result<Value, String> {
    let store = MemoryStore::new(state.project_root.join(".peridot/memory.db"));
    let result = crate::commands::import_session_artifacts(
        &store,
        &state.project_root,
        &PathBuf::from(from),
        id,
        force,
    )
    .map_err(|err| err.to_string())?;
    emit_session_list_changed(state).await;
    let items: Vec<Value> = result
        .files
        .iter()
        .map(|file| {
            serde_json::json!({
                "source": "file",
                "label": file,
            })
        })
        .collect();
    let (notes_count, last_note) =
        crate::commands::read_notes_summary(&state.project_root, &result.id);
    let attachment_paths = session_attachment_paths(state, &result.id);
    let attachment_count = attachment_paths.len();
    Ok(serde_json::json!({
        "kind": "session_import",
        "title": "Session Import",
        "message": format!("session import: imported {} from {} into {}", result.id, result.source, result.destination),
        "severity": "info",
        "command": raw_command,
        "id": result.id,
        "session_id": result.id,
        "source": result.source,
        "destination": result.destination,
        "files": result.files,
        "notes_count": notes_count,
        "last_note": last_note,
        "attachment_count": attachment_count,
        "attachment_paths": attachment_paths,
        "items": items,
        "total": items.len(),
    }))
}

fn export_session_id(
    state: &DaemonState,
    session_id: &str,
    raw_command: &str,
    artifacts: &[ExportArtifact],
    target: Option<&str>,
) -> Result<Value, String> {
    let selected = map_export_artifacts(artifacts);
    let out_dir = default_session_export_dir(&state.project_root, session_id);
    let report = crate::commands::export_session_artifacts(
        &state.project_root,
        session_id,
        &out_dir,
        &selected,
        false,
    )
    .map_err(|err| err.to_string())?;
    let mut items: Vec<Value> = report
        .files
        .iter()
        .map(|file| {
            serde_json::json!({
                "source": "full_copy",
                "label": file,
                "detail": "full copy",
            })
        })
        .collect();
    items.extend(report.artifacts.iter().map(|artifact| {
        serde_json::json!({
            "source": "artifact",
            "label": artifact.path,
            "detail": format!("{} entries · {}", artifact.count, artifact.class),
        })
    }));
    Ok(serde_json::json!({
        "kind": "session_export",
        "title": "Session Artifact Export",
        "message": format!("export: wrote {} artifact file(s) to {}", report.artifacts.len(), report.destination),
        "severity": "info",
        "command": raw_command,
        "target": target,
        "id": report.id,
        "source": report.source,
        "destination": report.destination,
        "artifact_classes": report.artifact_classes,
        "files": report.files,
        "artifacts": report.artifacts,
        "items": items,
        "total": items.len(),
    }))
}

fn default_session_export_dir(project_root: &Path, session_id: &str) -> PathBuf {
    project_root.join(".peridot").join("exports").join(format!(
        "{}-{}",
        sanitize_export_segment(session_id),
        current_unix_secs()
    ))
}

fn sanitize_export_segment(value: &str) -> String {
    let sanitized: String = value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.') {
                ch
            } else {
                '-'
            }
        })
        .collect();
    let trimmed = sanitized.trim_matches('-');
    if trimmed.is_empty() {
        "session".to_string()
    } else {
        trimmed.to_string()
    }
}

fn current_unix_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or_default()
}

fn map_export_artifacts(
    artifacts: &[ExportArtifact],
) -> Vec<crate::commands::SessionExportArtifact> {
    artifacts
        .iter()
        .map(|artifact| match artifact {
            ExportArtifact::Full => crate::commands::SessionExportArtifact::Full,
            ExportArtifact::Attachments => crate::commands::SessionExportArtifact::Attachments,
            ExportArtifact::Notes => crate::commands::SessionExportArtifact::Notes,
            ExportArtifact::Timeline => crate::commands::SessionExportArtifact::Timeline,
        })
        .collect()
}
