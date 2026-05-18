//! Intent clarification preflight.
//!
//! Sits between the user's raw request and the main executor loop. A
//! single classification round-trip to the main model inspects the
//! task plus lightweight project context (AGENTS.md head, project
//! profile summary) and decides whether the request is concrete
//! enough to act on or whether the operator should be asked to pin
//! down their intent first.
//!
//! When the verdict is `NeedsClarification` and the harness exposes an
//! `AskUserPort`, the preflight dispatches a `SingleSelect` question
//! with the model's candidate interpretations and returns the
//! operator's pick so the executor's first turn starts from a clear
//! goal rather than from "fix the bug".
//!
//! Model selection follows the same convention as the rest of the
//! harness: the call uses `options.model` (the operator's chosen main
//! model). Output is capped at 256 tokens with reasoning off so the
//! preflight stays a fixed-cost step regardless of which model is
//! configured. On provider error or unparseable output the helper
//! returns `Clear` so the executor still gets a chance to ask on its
//! own — a missed clarification is worse than a missed preflight.

use peridot_common::{PeriResult, ReasoningEffort};
use peridot_llm::{CompletionRequest, LlmMessage, LlmProvider, MessageRole, ToolChoice};
use serde::{Deserialize, Serialize};

/// Verdict returned by [`analyze_intent_clarification`].
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum IntentClarification {
    /// The task is concrete enough to start executing.
    Clear,
    /// The task is ambiguous; show the operator the candidate
    /// interpretations and let them pick (or type their own answer via
    /// the panel's `[o] Other`).
    NeedsClarification {
        /// Question shown to the operator.
        question: String,
        /// Candidate interpretations grounded in project context.
        candidates: Vec<String>,
    },
}

/// Asks `provider` whether `task` needs an up-front clarification. The
/// `agents_md_excerpt` and `project_summary` give the model just enough
/// context to ground candidate interpretations without spending tokens
/// on a full repo scan. On any error the function returns `Clear` so
/// the executor loop still runs.
pub async fn analyze_intent_clarification<P>(
    provider: &P,
    model: &str,
    task: &str,
    agents_md_excerpt: &str,
    project_summary: &str,
) -> PeriResult<IntentClarification>
where
    P: LlmProvider + ?Sized,
{
    let system = "You are Peridot's intent clarification gate. Decide whether the operator's \
        task is concrete enough to start coding, or whether you need to ask them to pick \
        between candidate interpretations first.\n\
        Respond with a single JSON object on one line, no prose, no code fences:\n\
          - When the task is concrete: {\"verdict\":\"clear\"}\n\
          - When the task is ambiguous: {\"verdict\":\"needs_clarification\",\
            \"question\":\"<one-line question>\",\
            \"candidates\":[\"<concrete candidate 1>\",\"<concrete candidate 2>\",...]}\n\
        Guidelines:\n\
          - Treat vague verbs (improve, refactor, fix, better, clean up, optimize, make it work, \
            doesn't work, broken) and vague references (the bug, this feature, this part, it, that) \
            as ambiguous unless an existing todo.md or AGENTS.md rule names the target.\n\
          - Provide 2-4 candidates. Each must be a concrete, actionable interpretation that names \
            a file, symbol, or scope from the project context.\n\
          - When the task is concrete enough (specific file path, named function, or unambiguous \
            user-visible change), return clear.";
    let user = format!(
        "Task:\n{task}\n\nAGENTS.md excerpt:\n{agents}\n\nProject summary:\n{project}",
        task = task,
        agents = if agents_md_excerpt.trim().is_empty() {
            "(none)"
        } else {
            agents_md_excerpt
        },
        project = if project_summary.trim().is_empty() {
            "(none)"
        } else {
            project_summary
        },
    );
    let request = CompletionRequest {
        model: model.to_string(),
        system: Some(system.to_string()),
        messages: vec![LlmMessage::new(MessageRole::User, user)],
        max_tokens: Some(256),
        thinking: false,
        reasoning_effort: ReasoningEffort::Off,
        service_tier: None,
        tools: Vec::new(),
        tool_choice: ToolChoice::None,
    };
    match provider.complete(request).await {
        Ok(response) => Ok(parse_verdict(&response.text)),
        Err(_) => Ok(IntentClarification::Clear),
    }
}

fn parse_verdict(text: &str) -> IntentClarification {
    let trimmed = extract_json_object(text).unwrap_or_default();
    let Ok(value) = serde_json::from_str::<serde_json::Value>(&trimmed) else {
        return IntentClarification::Clear;
    };
    let verdict = value
        .get("verdict")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase();
    if verdict != "needs_clarification" && verdict != "ambiguous" {
        return IntentClarification::Clear;
    }
    let question = value
        .get("question")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("Which interpretation of your request should I implement?")
        .to_string();
    let candidates = value
        .get("candidates")
        .and_then(serde_json::Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(serde_json::Value::as_str)
                .map(str::to_string)
                .filter(|candidate| !candidate.trim().is_empty())
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    if candidates.len() < 2 {
        // A clarification with fewer than two candidates is just a
        // narrow guess. Skip the prompt and let the executor proceed
        // — its own ask_user can fire later if it still feels stuck.
        return IntentClarification::Clear;
    }
    IntentClarification::NeedsClarification {
        question,
        candidates,
    }
}

/// Best-effort extraction of the first balanced top-level JSON object
/// in `text`. Mirrors the tolerance the executor's response parser
/// applies to model output that wraps JSON in prose.
fn extract_json_object(text: &str) -> Option<String> {
    let bytes = text.as_bytes();
    let start = bytes.iter().position(|&b| b == b'{')?;
    let mut depth = 0_i32;
    let mut in_string = false;
    let mut escaped = false;
    for (idx, &byte) in bytes.iter().enumerate().skip(start) {
        if in_string {
            if escaped {
                escaped = false;
            } else if byte == b'\\' {
                escaped = true;
            } else if byte == b'"' {
                in_string = false;
            }
            continue;
        }
        match byte {
            b'"' => in_string = true,
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(text[start..=idx].to_string());
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
    use async_trait::async_trait;
    use peridot_common::PeriResult;
    use peridot_llm::{AuthMethod, CompletionResponse, PricingTable, Usage};
    use std::sync::Mutex;

    struct StaticProvider {
        text: Mutex<String>,
        error: bool,
    }

    impl StaticProvider {
        fn new(text: &str) -> Self {
            Self {
                text: Mutex::new(text.to_string()),
                error: false,
            }
        }
        fn failing() -> Self {
            Self {
                text: Mutex::new(String::new()),
                error: true,
            }
        }
    }

    #[async_trait]
    impl LlmProvider for StaticProvider {
        async fn complete(&self, _req: CompletionRequest) -> PeriResult<CompletionResponse> {
            if self.error {
                return Err(peridot_common::PeriError::Provider("boom".to_string()));
            }
            Ok(CompletionResponse {
                text: self.text.lock().unwrap().clone(),
                tool_calls: Vec::new(),
                reasoning_content: None,
                usage: Usage::default(),
            })
        }
        fn supports_cache(&self) -> bool {
            false
        }
        fn supports_prefill(&self) -> bool {
            false
        }
        fn supports_thinking(&self) -> bool {
            false
        }
        fn pricing(&self) -> PricingTable {
            PricingTable::default()
        }
        fn auth_method(&self) -> AuthMethod {
            AuthMethod::ApiKey
        }
    }

    #[tokio::test]
    async fn returns_clear_for_concrete_task() {
        let provider = StaticProvider::new(r#"{"verdict":"clear"}"#);
        let verdict =
            analyze_intent_clarification(&provider, "m", "rename foo to bar in src/lib.rs", "", "")
                .await
                .unwrap();
        assert_eq!(verdict, IntentClarification::Clear);
    }

    #[tokio::test]
    async fn returns_candidates_for_vague_task() {
        let provider = StaticProvider::new(
            r#"{"verdict":"needs_clarification","question":"Which bug?","candidates":["A","B","C"]}"#,
        );
        let verdict = analyze_intent_clarification(&provider, "m", "fix the bug", "", "")
            .await
            .unwrap();
        match verdict {
            IntentClarification::NeedsClarification {
                question,
                candidates,
            } => {
                assert_eq!(question, "Which bug?");
                assert_eq!(candidates, vec!["A", "B", "C"]);
            }
            _ => panic!("expected NeedsClarification"),
        }
    }

    #[tokio::test]
    async fn single_candidate_falls_back_to_clear() {
        let provider = StaticProvider::new(
            r#"{"verdict":"needs_clarification","question":"Q","candidates":["only one"]}"#,
        );
        let verdict = analyze_intent_clarification(&provider, "m", "task", "", "")
            .await
            .unwrap();
        assert_eq!(verdict, IntentClarification::Clear);
    }

    #[tokio::test]
    async fn provider_error_returns_clear() {
        let provider = StaticProvider::failing();
        let verdict = analyze_intent_clarification(&provider, "m", "task", "", "")
            .await
            .unwrap();
        assert_eq!(verdict, IntentClarification::Clear);
    }

    #[test]
    fn parses_json_wrapped_in_prose() {
        let raw = "Sure — here's my call:\n{\"verdict\":\"clear\"}\nThanks!";
        assert_eq!(parse_verdict(raw), IntentClarification::Clear);
    }

    #[test]
    fn parses_json_in_code_fence() {
        let raw = "```json\n{\"verdict\":\"needs_clarification\",\"question\":\"Which one?\",\"candidates\":[\"a\",\"b\"]}\n```";
        match parse_verdict(raw) {
            IntentClarification::NeedsClarification {
                question,
                candidates,
            } => {
                assert_eq!(question, "Which one?");
                assert_eq!(candidates, vec!["a", "b"]);
            }
            _ => panic!("expected NeedsClarification"),
        }
    }
}
