//! Verifier-failure tracking and auto-fix directive synthesis.
//!
//! When an auto-verify tool (build / test / lint) fails, the harness records a
//! [`VerifyFailureState`] — the failing tool, a stable signature of its output,
//! how many consecutive attempts have hit the same signature, and parsed
//! `path:line` hints — and turns it into the auto-fix directive injected back
//! into the loop. Split out of `agent.rs` so the (pure, well-tested) failure
//! parsing lives next to the state it builds.

use peridot_common::ToolResult;

use crate::agent_helpers::truncate_chars;
use crate::requests::AgentTurnOutcome;

/// Tracks consecutive verifier failures so the auto-fix loop can escalate its
/// guidance (and stop repeating an ineffective patch) when the same failure
/// signature recurs.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct VerifyFailureState {
    pub(crate) tool_name: String,
    pub(crate) signature: String,
    pub(crate) attempts: u32,
    /// Parsed `path[:line]` hints extracted from the verifier output.
    /// Helps the model jump straight to the culprit instead of grepping
    /// the failing log line by line. Empty when no recognisable
    /// `path:line` token is present.
    pub(crate) hints: Vec<String>,
}

/// Folds `outcome` into the running verifier-failure state and returns the
/// updated state. The attempt counter increments only while the same tool keeps
/// failing with the same signature; any change resets it to 1.
pub(crate) fn update_verify_failure_state<'a>(
    state: &'a mut Option<VerifyFailureState>,
    outcome: &AgentTurnOutcome,
) -> &'a VerifyFailureState {
    let signature = verify_failure_signature(&outcome.tool_result);
    let hints = extract_verify_failure_hints(&outcome.tool_result);
    let attempts = match state.as_ref() {
        Some(previous)
            if previous.tool_name == outcome.tool_name && previous.signature == signature =>
        {
            previous.attempts.saturating_add(1)
        }
        _ => 1,
    };
    *state = Some(VerifyFailureState {
        tool_name: outcome.tool_name.clone(),
        signature,
        attempts,
        hints,
    });
    state.as_ref().expect("verify failure state just written")
}

/// Scans the verifier output for `file.ext:line[:col]` tokens and
/// returns up to 5 unique hits in order of appearance. Recognises
/// Rust (`src/foo.rs:12:5`), TypeScript / JS (`src/foo.ts:12:5`),
/// Python (`File "src/foo.py", line 12`), and bare line markers
/// (`foo.go:12`). The list is deduplicated so a single failing file
/// only appears once even when its name is repeated all over the
/// traceback.
fn extract_verify_failure_hints(result: &ToolResult) -> Vec<String> {
    let mut buffer = String::new();
    buffer.push_str(&result.summary);
    if !result.output.is_null()
        && let Ok(rendered) = serde_json::to_string(&result.output)
    {
        buffer.push('\n');
        buffer.push_str(&rendered);
    }
    let mut hints: Vec<String> = Vec::new();
    // Python frame: File "...", line N.
    for chunk in buffer.split("File \"").skip(1) {
        if let Some(end) = chunk.find('"')
            && let Some(rest) = chunk.get(end..)
            && let Some(line_idx) = rest.find("line ")
        {
            let path = &chunk[..end];
            let after = &rest[line_idx + 5..];
            let line: String = after.chars().take_while(|c| c.is_ascii_digit()).collect();
            if !line.is_empty() {
                let hint = format!("{path}:{line}");
                if !hints.iter().any(|h| h == &hint) {
                    hints.push(hint);
                }
            }
        }
        if hints.len() >= 5 {
            break;
        }
    }
    // Generic `path/with.ext:NN[:MM]` — capture the longest contiguous
    // non-whitespace run that ends in a digit after a `:`. Cheap
    // scanner; avoids pulling in a regex crate for the harness.
    for token in buffer.split(|c: char| c.is_whitespace() || c == '(' || c == ')') {
        if hints.len() >= 5 {
            break;
        }
        let trimmed = token.trim_matches(|c: char| matches!(c, ',' | '.' | ':' | '\'' | '"' | '`'));
        if !looks_like_path_with_line(trimmed) {
            continue;
        }
        if !hints.iter().any(|h| h == trimmed) {
            hints.push(trimmed.to_string());
        }
    }
    hints
}

fn looks_like_path_with_line(token: &str) -> bool {
    // Require at least one '/' or '.' so plain words don't match, plus
    // a `:digit` suffix. Reject obvious noise (urls, time stamps).
    if token.starts_with("http://") || token.starts_with("https://") {
        return false;
    }
    let Some(colon) = token.find(':') else {
        return false;
    };
    let (path_part, rest) = token.split_at(colon);
    if !(path_part.contains('.') || path_part.contains('/')) {
        return false;
    }
    if path_part.contains(' ') {
        return false;
    }
    let after_colon = &rest[1..];
    let line_str: String = after_colon
        .chars()
        .take_while(|c| c.is_ascii_digit())
        .collect();
    !line_str.is_empty()
}

fn verify_failure_signature(result: &ToolResult) -> String {
    let mut material = String::new();
    material.push_str(result.summary.trim());
    if !result.output.is_null()
        && let Ok(output) = serde_json::to_string(&result.output)
    {
        material.push('\n');
        material.push_str(&output);
    }
    let normalized = material
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .take(12)
        .collect::<Vec<_>>()
        .join(" | ");
    truncate_chars(&normalized, 180)
}

pub(crate) fn verify_failure_directive(failure: &VerifyFailureState, cap: u32) -> String {
    let repeat_note = if failure.attempts > 1 {
        " This is the same verifier failure signature as the previous attempt; change strategy before editing again."
    } else {
        ""
    };
    // Surface the parsed file:line hints so the model knows where to
    // open. Empty when the verifier output didn't include any
    // recognisable location markers — in that case the directive
    // simply omits the hint sentence.
    let hint_note = if failure.hints.is_empty() {
        String::new()
    } else {
        format!(
            " Likely culprit(s) from the failing output: {}. Read those files first before editing anything else.",
            failure.hints.join(", ")
        )
    };
    format!(
        "Auto-fix directive ({}/{}): {} failed with signature `{}`.{} STOP all new work. Diagnose that verifier output, make the smallest targeted change, and re-run `{}`.{} If it fails again with the same signature, do not repeat the same patch pattern; explain the blocker or ask the operator.",
        failure.attempts,
        cap,
        failure.tool_name,
        failure.signature,
        hint_note,
        failure.tool_name,
        repeat_note,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use peridot_llm::Usage;

    #[test]
    fn verify_failure_state_repeats_same_signature_and_resets_on_new_one() {
        let mut state = None;
        let first = AgentTurnOutcome {
            tool_name: "verify_build".to_string(),
            tool_result: ToolResult::failure("error[E0425]: cannot find value `x`"),
            usage: Usage::default(),
            done: false,
        };
        let repeated = update_verify_failure_state(&mut state, &first).clone();
        assert_eq!(repeated.attempts, 1);
        let repeated = update_verify_failure_state(&mut state, &first).clone();
        assert_eq!(repeated.attempts, 2);

        let changed = AgentTurnOutcome {
            tool_name: "verify_build".to_string(),
            tool_result: ToolResult::failure("error[E0308]: mismatched types"),
            usage: Usage::default(),
            done: false,
        };
        let changed = update_verify_failure_state(&mut state, &changed).clone();
        assert_eq!(changed.attempts, 1);
        assert!(changed.signature.contains("E0308"));
    }

    #[test]
    fn verify_failure_directive_mentions_repeated_signature() {
        let failure = VerifyFailureState {
            tool_name: "verify_test".to_string(),
            signature: "test foo failed".to_string(),
            attempts: 2,
            hints: Vec::new(),
        };
        let directive = verify_failure_directive(&failure, 3);
        assert!(directive.contains("2/3"));
        assert!(directive.contains("same verifier failure signature"));
        assert!(directive.contains("re-run `verify_test`"));
    }

    #[test]
    fn verify_failure_directive_includes_path_line_hints_when_present() {
        let failure = VerifyFailureState {
            tool_name: "verify_test".to_string(),
            signature: "assertion failed".to_string(),
            attempts: 1,
            hints: vec!["src/lib.rs:42".to_string(), "src/util.rs:7".to_string()],
        };
        let directive = verify_failure_directive(&failure, 3);
        assert!(
            directive.contains("Likely culprit"),
            "directive must announce hint section: {directive}"
        );
        assert!(directive.contains("src/lib.rs:42"));
        assert!(directive.contains("src/util.rs:7"));
    }

    #[test]
    fn extract_verify_failure_hints_parses_rust_and_python_locations() {
        let result = ToolResult::failure(
            "error[E0425]: cannot find value `x` in this scope\n  --> src/lib.rs:42:5\n   |\n   File \"tests/test_one.py\", line 7\n",
        );
        let hints = extract_verify_failure_hints(&result);
        assert!(
            hints.iter().any(|h| h.starts_with("src/lib.rs:42")),
            "expected a src/lib.rs:42[:col] hint in {hints:?}"
        );
        assert!(
            hints.iter().any(|h| h == "tests/test_one.py:7"),
            "expected tests/test_one.py:7 in {hints:?}"
        );
    }

    #[test]
    fn extract_verify_failure_hints_skips_urls() {
        let result = ToolResult::failure(
            "see https://docs.rs/foo:5 for details and src/main.rs:12 for the actual problem",
        );
        let hints = extract_verify_failure_hints(&result);
        assert!(
            hints.iter().any(|h| h == "src/main.rs:12"),
            "expected src/main.rs:12 in {hints:?}"
        );
        assert!(
            !hints.iter().any(|h| h.contains("https")),
            "must not return URLs as hints: {hints:?}"
        );
    }
}
