//! Task complexity classifier.
//!
//! Replaces the brittle "task is short, skip the planner" heuristic
//! with a single classification round-trip to the main model that
//! returns one of four labels. The committee preflight only fires for
//! `Complex` and `Architectural`; `Chat` and `Simple` go straight to
//! the executor.
//!
//! Bounded to one round trip with max 64 output tokens and reasoning
//! off so the gate stays a fixed cost regardless of which model is
//! configured. The classifier is OFF by default — operators opt in
//! via `committee.use_llm_complexity_gate`, with the legacy
//! `committee.min_task_chars` length gate left in place as a free
//! pre-filter (we skip the LLM entirely for sub-threshold tasks).

use peridot_common::{PeriResult, ReasoningEffort};
use peridot_llm::{CompletionRequest, LlmMessage, LlmProvider, MessageRole, ToolChoice};
use serde::{Deserialize, Serialize};

/// Coarse-grained complexity label produced by [`classify_task_complexity`].
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskComplexity {
    /// Conversational message, greeting, or question that needs no edit.
    Chat,
    /// One- or two-step concrete change with an obvious target file.
    Simple,
    /// Multi-step coding work — multiple files, refactor, feature add.
    Complex,
    /// Cross-cutting design / architecture / large refactor / migration.
    Architectural,
}

impl TaskComplexity {
    /// Returns `true` when the committee planner preflight should
    /// fire for a task with this complexity.
    pub fn warrants_planner(self) -> bool {
        matches!(
            self,
            TaskComplexity::Complex | TaskComplexity::Architectural
        )
    }

    /// Parses a single label string. Unknown values fall through to
    /// `Simple` so a flaky classifier never silently skips the
    /// planner on a genuinely complex task — better one extra
    /// preflight than one missed.
    pub fn parse(input: &str) -> Self {
        match input.trim().to_ascii_lowercase().as_str() {
            "chat" | "smalltalk" | "greeting" | "question" => TaskComplexity::Chat,
            "simple" | "trivial" | "small" | "easy" => TaskComplexity::Simple,
            "complex" | "multistep" | "multi-step" | "medium" | "hard" => TaskComplexity::Complex,
            "architectural" | "architecture" | "design" | "refactor" | "migration" => {
                TaskComplexity::Architectural
            }
            _ => TaskComplexity::Simple,
        }
    }
}

/// Asks `provider` to classify `task`. Returns the parsed verdict.
/// On any provider error or unparseable response the function returns
/// `TaskComplexity::Complex` so the planner DOES run — a missed
/// planner is worse than an extra one. Cost is tiny: 64 output tokens
/// with reasoning off.
pub async fn classify_task_complexity<P>(
    provider: &P,
    model: &str,
    task: &str,
) -> PeriResult<TaskComplexity>
where
    P: LlmProvider + ?Sized,
{
    let system = "You are Peridot's task complexity classifier. Read the operator's task \
        and respond with exactly one word from this list — no punctuation, no quotes, no extra prose: \
        chat | simple | complex | architectural. \
        Use `chat` for greetings and questions that need no edit. \
        Use `simple` for a one- or two-step concrete change with an obvious target file. \
        Use `complex` for multi-step coding work touching multiple files. \
        Use `architectural` for cross-cutting design, refactors, or migrations.";
    let request = CompletionRequest {
        model: model.to_string(),
        system: Some(system.to_string()),
        messages: vec![LlmMessage::new(MessageRole::User, task.to_string())],
        max_tokens: Some(64),
        thinking: false,
        reasoning_effort: ReasoningEffort::Off,
        service_tier: None,
        tools: Vec::new(),
        tool_choice: ToolChoice::None,
    };
    match provider.complete(request).await {
        Ok(response) => Ok(TaskComplexity::parse(&response.text)),
        Err(_) => Ok(TaskComplexity::Complex),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use peridot_llm::{AuthMethod, CompletionResponse, PricingTable, Usage};
    use std::sync::Mutex;

    struct StaticProvider {
        responses: Mutex<Vec<String>>,
    }

    impl StaticProvider {
        fn new(text: &str) -> Self {
            Self {
                responses: Mutex::new(vec![text.to_string()]),
            }
        }
    }

    #[async_trait]
    impl LlmProvider for StaticProvider {
        async fn complete(&self, _req: CompletionRequest) -> PeriResult<CompletionResponse> {
            let text = self.responses.lock().unwrap().pop().unwrap_or_default();
            Ok(CompletionResponse {
                text,
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

    #[test]
    fn parse_handles_known_labels() {
        assert_eq!(TaskComplexity::parse("chat"), TaskComplexity::Chat);
        assert_eq!(TaskComplexity::parse("CHAT"), TaskComplexity::Chat);
        assert_eq!(TaskComplexity::parse("simple"), TaskComplexity::Simple);
        assert_eq!(TaskComplexity::parse("complex"), TaskComplexity::Complex);
        assert_eq!(
            TaskComplexity::parse("architectural"),
            TaskComplexity::Architectural
        );
        assert_eq!(
            TaskComplexity::parse("design"),
            TaskComplexity::Architectural
        );
    }

    #[test]
    fn unknown_label_defaults_to_simple() {
        assert_eq!(TaskComplexity::parse("???"), TaskComplexity::Simple);
        assert_eq!(TaskComplexity::parse(""), TaskComplexity::Simple);
    }

    #[test]
    fn warrants_planner_only_for_complex_or_architectural() {
        assert!(!TaskComplexity::Chat.warrants_planner());
        assert!(!TaskComplexity::Simple.warrants_planner());
        assert!(TaskComplexity::Complex.warrants_planner());
        assert!(TaskComplexity::Architectural.warrants_planner());
    }

    #[tokio::test]
    async fn classifies_chat_response() {
        let provider = StaticProvider::new("chat");
        let result = classify_task_complexity(&provider, "test-model", "hi how are you")
            .await
            .unwrap();
        assert_eq!(result, TaskComplexity::Chat);
    }

    #[tokio::test]
    async fn classifies_architectural_response() {
        let provider = StaticProvider::new("architectural");
        let result = classify_task_complexity(
            &provider,
            "test-model",
            "redesign the storage layer to use SQLite",
        )
        .await
        .unwrap();
        assert_eq!(result, TaskComplexity::Architectural);
    }
}
