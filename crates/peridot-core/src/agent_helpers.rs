//! Stateless helpers extracted from the giant `agent.rs` so the agent
//! loop reads top-to-bottom without scrolling through pure utility
//! code. The split is intentionally conservative — only functions that
//! (a) take simple inputs, (b) return plain values, and (c) have no
//! observable interaction with `HarnessAgent` internals move here.
//! The agent's stateful methods (turn execution, recovery, message
//! drain, etc.) stay in `agent.rs` so a reader who is reasoning about
//! "what happens on this turn" doesn't have to jump between files.

use peridot_common::PeriError;
use peridot_context::{ContextManager, ContextSource};

/// Returns true when the harness produced this error because a tool
/// required explicit user approval. Used by the loop to distinguish a
/// recoverable halt (pause + persist pending tool call) from a fatal
/// error.
pub(crate) fn approval_required_error(err: &PeriError) -> bool {
    match err {
        PeriError::PermissionDenied(reason) => reason.contains("requires explicit user approval"),
        _ => false,
    }
}

/// Returns true when the named tool mutates the workspace. Matches the
/// hardcoded list the committee loop uses (`run_committee_loop_with_events`)
/// so auto-verify and reviewer triggering stay aligned.
pub(crate) fn is_mutating_tool_name(name: &str) -> bool {
    matches!(name, "file_write" | "file_patch" | "shell_exec")
}

/// Truncates a `&str` to at most `max_chars` code points, appending `...`
/// when the input was longer. Used for compact log lines so a giant
/// stacktrace doesn't blow the transcript width.
pub(crate) fn truncate_chars(value: &str, max_chars: usize) -> String {
    let mut chars = value.chars();
    let truncated = chars.by_ref().take(max_chars).collect::<String>();
    if chars.next().is_some() {
        format!("{truncated}...")
    } else {
        truncated
    }
}

/// Scans the most recent context entries for a `verify_*` tool result
/// or `[auto-verify]` PlanReminder note. Used as the third grader
/// input so the LLM gates on actual check output, not just the diff.
pub(crate) fn recent_verify_summary(context: &ContextManager) -> Option<String> {
    for entry in context.entries().iter().rev().take(20) {
        let content = entry.content.trim();
        if content.starts_with("[auto-verify]") {
            return Some(content.to_string());
        }
        if entry.source == ContextSource::Tool && content.contains("verify_") {
            return Some(content.to_string());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn approval_required_error_only_matches_user_approval_messages() {
        let yes = PeriError::PermissionDenied(
            "dependency installation requires explicit user approval".to_string(),
        );
        assert!(approval_required_error(&yes));
        let no_other_denial = PeriError::PermissionDenied("path outside project".to_string());
        assert!(!approval_required_error(&no_other_denial));
        let no_unrelated = PeriError::Tool("oops".to_string());
        assert!(!approval_required_error(&no_unrelated));
    }

    #[test]
    fn is_mutating_tool_name_covers_the_canonical_list() {
        assert!(is_mutating_tool_name("file_write"));
        assert!(is_mutating_tool_name("file_patch"));
        assert!(is_mutating_tool_name("shell_exec"));
        assert!(!is_mutating_tool_name("verify_build"));
        assert!(!is_mutating_tool_name("file_read"));
        assert!(!is_mutating_tool_name("agent_done"));
    }

    #[test]
    fn truncate_chars_appends_ellipsis_only_when_truncated() {
        assert_eq!(truncate_chars("short", 10), "short");
        assert_eq!(truncate_chars("12345678", 4), "1234...");
        // Multibyte safe: each Korean syllable is one char.
        assert_eq!(truncate_chars("안녕하세요", 3), "안녕하...");
    }

    #[test]
    fn recent_verify_summary_prefers_auto_verify_planreminder() {
        use peridot_context::ContextEntry;
        let mut ctx = ContextManager::new();
        ctx.append(ContextEntry::trusted(ContextSource::User, "hi"));
        ctx.append(ContextEntry::trusted(
            ContextSource::PlanReminder,
            "[auto-verify] verify_build passed: 0 errors",
        ));
        let summary = recent_verify_summary(&ctx).expect("auto-verify note retrievable");
        assert!(summary.contains("verify_build passed"));
    }

    #[test]
    fn recent_verify_summary_falls_back_to_tool_entry_with_verify_substring() {
        use peridot_context::ContextEntry;
        let mut ctx = ContextManager::new();
        ctx.append(ContextEntry::trusted(
            ContextSource::Tool,
            "{\"summary\":\"verify_test failed: 1/12\"}",
        ));
        let summary = recent_verify_summary(&ctx).expect("tool entry retrievable");
        assert!(summary.contains("verify_test"));
    }
}
