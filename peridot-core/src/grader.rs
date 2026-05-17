//! LLM-based work grader (Verify pipeline Stage 5).
//!
//! Stages 1–4 in `peridot-verify` are deterministic (build / test / lint /
//! diff review). Stage 5 asks an LLM to look at the task, the work that
//! landed, and whatever signal the deterministic stages produced, then
//! returns a pass/fail verdict + recommendations. This is the qualitative
//! "is the change actually good?" gate that the deterministic checks
//! cannot answer on their own.
//!
//! Callers supply the provider + model. The grader pulls its model from
//! `ModelsConfig::goal_checker()`, which always mirrors `models.main` —
//! there is intentionally no separate `models.goal_checker` knob so a
//! single switch reroutes both the main loop and the grader together
//! (avoids the failure mode where one is updated and the other is
//! quietly left on a stale model). The grader is a plain async function
//! rather than a tool so it never enters the agent's tool catalog (we
//! don't want the model triggering its own grading via tool_call).

use peridot_common::{PeriError, PeriResult, ReasoningEffort};
use peridot_llm::{CompletionRequest, LlmMessage, LlmProvider, MessageRole, ToolChoice, Usage};
use serde::{Deserialize, Serialize};

/// Verdict returned by [`grade_work`]. Mirrors the shape `peridot-verify`
/// expects for a Grader stage result so the caller can hand it straight
/// into `VerifyStageResult`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct GraderVerdict {
    /// Whether the grader believes the change is acceptable.
    pub passed: bool,
    /// One-line summary suitable for `VerifyStageResult.summary`.
    pub summary: String,
    /// Optional follow-up suggestions; never blocks pass=true on its own.
    pub recommendations: Vec<String>,
    /// Token / cost usage the grader call consumed. Counted against the
    /// session budget the same as any other LLM call.
    pub usage: Usage,
}

/// Runs the grader against a single (task, diff, verify_summary) tuple.
/// Returns a verdict; the caller decides whether to halt the agent loop
/// on a failing verdict or just record it.
pub async fn grade_work<P>(
    provider: &P,
    model: &str,
    task: &str,
    diff: &str,
    verify_summary: &str,
) -> PeriResult<GraderVerdict>
where
    P: LlmProvider + ?Sized,
{
    let system = "You are Peridot's verify grader. The agent has finished a coding task. \
        Your job is to read the task, the diff, and the deterministic verify summary, \
        then decide whether the change is acceptable to ship. \
        Answer in this exact JSON shape on a single line: \
        {\"passed\": <bool>, \"summary\": <one short sentence>, \"recommendations\": [<strings>]}. \
        Pass when the change addresses the task and the verify summary shows no failures. \
        Fail when the diff is wrong, incomplete, off-task, or the verify checks reported errors. \
        Recommendations are optional follow-ups; an empty array is fine.";
    let body = format!(
        "Task:\n{task}\n\nDiff:\n{diff}\n\nVerify summary:\n{verify_summary}\n\nReturn ONLY the JSON line — no prose, no markdown fences."
    );
    let response = provider
        .complete(CompletionRequest {
            model: model.to_string(),
            system: Some(system.to_string()),
            messages: vec![LlmMessage::new(MessageRole::User, body)],
            max_tokens: Some(512),
            thinking: false,
            reasoning_effort: ReasoningEffort::Off,
            tools: Vec::new(),
            tool_choice: ToolChoice::None,
        })
        .await?;
    let text = response.text.trim();
    let verdict = parse_verdict_payload(text).ok_or_else(|| {
        PeriError::Parse(format!(
            "grader returned unparseable text (expected JSON object): {text}"
        ))
    })?;
    Ok(GraderVerdict {
        passed: verdict.passed,
        summary: verdict.summary,
        recommendations: verdict.recommendations,
        usage: response.usage,
    })
}

#[derive(Deserialize)]
struct ParsedVerdict {
    passed: bool,
    summary: String,
    #[serde(default)]
    recommendations: Vec<String>,
}

/// Best-effort parser: accepts strict JSON, JSON inside a `` ```json ... ``` ``
/// fence, or JSON inside a paragraph of prose. Returns `None` if no
/// recognisable JSON object is found.
fn parse_verdict_payload(text: &str) -> Option<ParsedVerdict> {
    if let Ok(direct) = serde_json::from_str::<ParsedVerdict>(text) {
        return Some(direct);
    }
    // Strip a leading ```json or ``` fence and trailing ``` then retry.
    let stripped = text
        .trim_start_matches("```json")
        .trim_start_matches("```")
        .trim_end_matches("```")
        .trim();
    if let Ok(fenced) = serde_json::from_str::<ParsedVerdict>(stripped) {
        return Some(fenced);
    }
    // Scan for the first balanced `{...}` substring as a last resort.
    let start = text.find('{')?;
    let mut depth = 0u32;
    let bytes = text.as_bytes();
    for (offset, byte) in bytes[start..].iter().enumerate() {
        match byte {
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    let end = start + offset + 1;
                    return serde_json::from_str::<ParsedVerdict>(&text[start..end]).ok();
                }
            }
            _ => {}
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_strict_json_payload() {
        let v = parse_verdict_payload(
            r#"{"passed": true, "summary": "looks good", "recommendations": []}"#,
        )
        .expect("parse");
        assert!(v.passed);
        assert_eq!(v.summary, "looks good");
        assert!(v.recommendations.is_empty());
    }

    #[test]
    fn parses_payload_with_recommendations() {
        let v = parse_verdict_payload(
            r#"{"passed": false, "summary": "missing tests", "recommendations": ["add unit tests", "update docs"]}"#,
        )
        .expect("parse");
        assert!(!v.passed);
        assert_eq!(v.recommendations.len(), 2);
    }

    #[test]
    fn parses_payload_inside_fence() {
        let v = parse_verdict_payload(
            "```json\n{\"passed\": true, \"summary\": \"fenced\", \"recommendations\": []}\n```",
        )
        .expect("parse");
        assert!(v.passed);
        assert_eq!(v.summary, "fenced");
    }

    #[test]
    fn parses_payload_inside_prose() {
        let v = parse_verdict_payload(
            "Verdict follows: {\"passed\": false, \"summary\": \"needs work\"} — recommendations omitted.",
        )
        .expect("parse");
        assert!(!v.passed);
    }

    #[test]
    fn rejects_garbage() {
        assert!(parse_verdict_payload("nope no json here").is_none());
    }
}
