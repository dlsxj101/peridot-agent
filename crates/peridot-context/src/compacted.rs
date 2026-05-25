//! Structured context-compaction schema.
//!
//! Today's compaction produces a prose summary string. That's fine for
//! humans skimming a transcript but loses structure — downstream tools
//! can't tell "files read so far" from "current task" from "approvals
//! granted" without re-parsing the prose. [`CompactedContext`] makes
//! every category a discrete field so TUI/VS Code/grader can render
//! them directly.
//!
//! Only the [`CompactedContext::narrative`] field is intended to be
//! LLM-generated. Every other field is a mechanical projection from
//! the [`crate::EvidenceLedger`] / audit log / plan state, so it's
//! deterministic and verifiable.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::{ContextEntry, ContextSource};

/// Structured snapshot of "what has happened so far" that replaces the
/// pure-prose compaction output.
///
/// Wire-format consumers (TUI side panel, VS Code "context overview",
/// grader prompt builder) should prefer the structured fields over
/// `narrative`. `narrative` exists as a fallback for any consumer that
/// truly just wants a paragraph — e.g., the system prompt's "recap of
/// the conversation so far" preface.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct CompactedContext {
    /// Notable decisions the model committed to (architectural choices,
    /// approach pivots, etc.). Order: chronological.
    #[serde(default)]
    pub decisions: Vec<Decision>,
    /// Files the model has read this run. Sourced from the
    /// [`EvidenceLedger`] of `file` references — line ranges and
    /// digests are preserved so a follow-up turn can re-validate
    /// quickly.
    #[serde(default)]
    pub files_read: Vec<FileRef>,
    /// Files the model has changed (via `file_write`, `file_patch`,
    /// or a delegated subagent's diff). Sourced from the audit log
    /// of mutating tool invocations.
    #[serde(default)]
    pub files_changed: Vec<FileChange>,
    /// Verification commands the model has run and their pass/fail
    /// results.
    #[serde(default)]
    pub verifications: Vec<VerificationRecord>,
    /// `todo.json` items still pending or in_progress.
    #[serde(default)]
    pub open_todos: Vec<TodoItem>,
    /// Approval grants the user has issued this run.
    #[serde(default)]
    pub approvals: Vec<ApprovalRecord>,
    /// External / untrusted inputs the model consumed (web fetch
    /// results, MCP responses, pasted content). Surfaced so grader
    /// and reviewer can challenge claims sourced from them.
    #[serde(default)]
    pub untrusted_inputs: Vec<UntrustedInput>,
    /// Short LLM-generated paragraph summarising the conversation
    /// state. The *only* field that requires an LLM call; everything
    /// else is mechanical projection.
    #[serde(default)]
    pub narrative: String,
}

/// A specific decision the model committed to during the run.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Decision {
    /// Short imperative phrase, e.g. `"use position-delta scroll tracking"`.
    pub summary: String,
    /// Optional turn index where the decision was first stated.
    #[serde(default)]
    pub turn_id: Option<u64>,
}

/// Pointer to a file region the model has read.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FileRef {
    /// Path relative to the project root.
    pub path: PathBuf,
    /// Optional inclusive line range (1-based).
    #[serde(default)]
    pub line_range: Option<(u32, u32)>,
    /// Optional content digest of the bytes the model saw, so a follow-up
    /// turn can detect that the file changed underneath.
    #[serde(default)]
    pub digest: Option<String>,
}

/// Record of a file the model mutated.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FileChange {
    /// Path relative to the project root.
    pub path: PathBuf,
    /// Digest of file content before the change.
    #[serde(default)]
    pub before_digest: Option<String>,
    /// Digest of file content after the change.
    #[serde(default)]
    pub after_digest: Option<String>,
    /// Tool that performed the change (e.g., `"file_patch"`).
    pub tool: String,
}

/// One verification command result.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct VerificationRecord {
    /// Verification kind: `"build"`, `"test"`, `"lint"`, `"format"`.
    pub kind: String,
    /// Whether the command succeeded.
    pub passed: bool,
    /// Short summary of the command's output.
    #[serde(default)]
    pub summary: Option<String>,
}

/// Plan / todo item.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TodoItem {
    /// One-based ordinal in the plan.
    pub id: u32,
    /// Short imperative description.
    pub text: String,
    /// Status — `"pending"`, `"in_progress"`, `"done"`.
    pub status: String,
}

/// Approval grant issued by the user.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ApprovalRecord {
    /// Tool name the grant covers.
    pub tool: String,
    /// Scope — `"once"`, `"command"`, `"path"`, `"session"`.
    pub scope: String,
    /// Optional descriptor (the command line, the path) the scope
    /// applies to.
    #[serde(default)]
    pub detail: Option<String>,
}

/// External / untrusted input the model has been exposed to.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct UntrustedInput {
    /// Source kind: `"web_fetch"`, `"mcp"`, `"paste"`, etc.
    pub kind: String,
    /// Short label — URL, MCP server name, etc.
    pub label: String,
}

impl CompactedContext {
    /// Build a [`CompactedContext`] by mechanical projection from a
    /// slice of context entries.
    ///
    /// Does NOT call an LLM — `narrative` is left empty. Callers that
    /// want a prose summary should call the existing
    /// `compact_with_llm_inner` and copy its result into the
    /// `narrative` field.
    ///
    /// Each entry's `evidence_refs` are scanned for `file` kinds; those
    /// become [`FileRef`] entries in `files_read`. External / untrusted
    /// entries surface as [`UntrustedInput`]s so the grader/preflight
    /// can challenge claims sourced from them.
    ///
    /// Future work (PR 14b): a dedicated `from_audit_log` path that
    /// populates `files_changed`/`verifications` from the run's audit
    /// trail. Today those fields stay empty when this constructor is
    /// used standalone.
    pub fn from_entries(entries: &[ContextEntry]) -> Self {
        let mut compacted = Self::default();
        let mut seen_files: std::collections::HashSet<String> = Default::default();
        for entry in entries {
            for evidence in &entry.evidence_refs {
                if evidence.kind == "file" {
                    // Dedup repeated reads of the same evidence ref —
                    // models tend to re-read files across turns.
                    if !seen_files.insert(evidence.id.clone()) {
                        continue;
                    }
                    // `EvidenceRef::path` is the on-disk storage location
                    // (`.peridot/evidence/...`); the actual source file
                    // identifier is in `id`. Project the compacted view
                    // off `id` so consumers see the file the model
                    // actually read, not the harness's storage path.
                    compacted.files_read.push(FileRef {
                        path: PathBuf::from(&evidence.id),
                        line_range: None,
                        digest: Some(evidence.digest.clone()),
                    });
                }
            }
            if entry.source == ContextSource::External || entry.untrusted {
                let label = entry
                    .content
                    .lines()
                    .next()
                    .unwrap_or("(untitled)")
                    .chars()
                    .take(120)
                    .collect();
                compacted.untrusted_inputs.push(UntrustedInput {
                    kind: "external".to_string(),
                    label,
                });
            }
        }
        compacted
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_entries_records_external_inputs() {
        let mut entries = Vec::new();
        let mut e = ContextEntry::trusted(ContextSource::User, "do the thing");
        e.turn_id = 1;
        entries.push(e);
        let mut ext = ContextEntry::untrusted(ContextSource::External, "scraped page content");
        ext.turn_id = 2;
        entries.push(ext);
        let compacted = CompactedContext::from_entries(&entries);
        assert_eq!(compacted.untrusted_inputs.len(), 1);
        assert_eq!(compacted.untrusted_inputs[0].kind, "external");
        assert!(compacted.untrusted_inputs[0].label.contains("scraped"));
    }

    #[test]
    fn empty_input_produces_empty_compacted() {
        let compacted = CompactedContext::from_entries(&[]);
        assert!(compacted.decisions.is_empty());
        assert!(compacted.files_read.is_empty());
        assert!(compacted.files_changed.is_empty());
        assert!(compacted.untrusted_inputs.is_empty());
        assert!(compacted.narrative.is_empty());
    }

    #[test]
    fn dedupes_repeated_file_reads() {
        use crate::EvidenceRef;
        let mut entries = Vec::new();
        for _ in 0..3 {
            let mut e = ContextEntry::trusted(ContextSource::Tool, "read src/lib.rs");
            e.evidence_refs.push(EvidenceRef {
                id: "src/lib.rs".to_string(),
                kind: "file".to_string(),
                summary: "L1-10".to_string(),
                bytes: 0,
                digest: "abc123".to_string(),
                path: ".peridot/evidence/e1.json".to_string(),
            });
            entries.push(e);
        }
        let compacted = CompactedContext::from_entries(&entries);
        assert_eq!(
            compacted.files_read.len(),
            1,
            "three reads of the same evidence ref should dedupe to one entry"
        );
        assert_eq!(compacted.files_read[0].path, PathBuf::from("src/lib.rs"));
    }
}
