//! Interactive `/session export` (and import-result rendering) handlers.
//!
//! Writes the active session's artifacts (full copy / attachments / notes /
//! timeline) to `.peridot/exports/<session>-<ts>/` and renders the export /
//! import result for the transcript. Split out of `main.rs`; the
//! `apply_session_command` dispatcher and the session-import path call these.

use std::path::{Path, PathBuf};

use peridot_core::ExportArtifact;
use peridot_tui::TuiState;

use crate::commands::{self, SessionExportArtifact, export_session_artifacts};
use crate::run_state::unix_timestamp;

pub(crate) fn handle_session_export(
    state: &mut TuiState,
    project_root: &Path,
    artifacts: &[ExportArtifact],
) {
    if state.current_session_id.is_empty() {
        state.push_error("export: no active session".to_string());
        return;
    }
    let session_id = state.current_session_id.clone();
    handle_session_export_for_id(state, project_root, &session_id, artifacts);
}

pub(crate) fn handle_session_export_for_id(
    state: &mut TuiState,
    project_root: &Path,
    session_id: &str,
    artifacts: &[ExportArtifact],
) {
    let selected = map_export_artifacts(artifacts);
    let out_dir = default_session_export_dir(project_root, session_id);
    match export_session_artifacts(project_root, session_id, &out_dir, &selected, false) {
        Ok(report) => state.push_transcript(render_session_export_text(&report)),
        Err(err) => state.push_error(format!("export: failed: {err}")),
    }
}

fn default_session_export_dir(project_root: &Path, session_id: &str) -> PathBuf {
    project_root.join(".peridot").join("exports").join(format!(
        "{}-{}",
        sanitize_export_segment(session_id),
        unix_timestamp()
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

fn map_export_artifacts(artifacts: &[ExportArtifact]) -> Vec<SessionExportArtifact> {
    artifacts
        .iter()
        .map(|artifact| match artifact {
            ExportArtifact::Full => SessionExportArtifact::Full,
            ExportArtifact::Attachments => SessionExportArtifact::Attachments,
            ExportArtifact::Notes => SessionExportArtifact::Notes,
            ExportArtifact::Timeline => SessionExportArtifact::Timeline,
        })
        .collect()
}

pub(crate) fn render_session_export_text(report: &commands::SessionExportReport) -> String {
    let mut body = format!(
        "export: wrote {} artifact file(s) to {}",
        report.artifacts.len(),
        report.destination
    );
    if !report.files.is_empty() {
        body.push_str(&format!("\nfull copy entries: {}", report.files.len()));
        for file in &report.files {
            body.push_str(&format!("\n  - {file}"));
        }
    }
    for artifact in &report.artifacts {
        body.push_str(&format!(
            "\n{}  {} entries  {}",
            artifact.path, artifact.count, artifact.class
        ));
    }
    body
}

pub(crate) fn render_session_import_text(
    result: &commands::SessionImportResult,
    notes_count: usize,
    last_note: Option<&str>,
    attachment_paths: &[String],
) -> String {
    let mut body = format!(
        "session import: imported {} from {} into {} ({} entries)",
        result.id,
        result.source,
        result.destination,
        result.files.len()
    );
    for file in &result.files {
        body.push_str(&format!("\n  - {file}"));
    }
    if notes_count > 0 {
        let suffix = last_note
            .map(|note| format!("  ({note})"))
            .unwrap_or_default();
        body.push_str(&format!("\nnotes: {notes_count}{suffix}"));
    }
    if !attachment_paths.is_empty() {
        body.push_str(&format!("\nattachments: {}", attachment_paths.len()));
        for path in attachment_paths {
            body.push_str(&format!("\n  - {path}"));
        }
    }
    body
}
