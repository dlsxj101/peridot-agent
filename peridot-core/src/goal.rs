use peridot_common::PeriResult;
use peridot_llm::{CompletionRequest, LlmMessage, LlmProvider, MessageRole, Usage};
use serde::{Deserialize, Serialize};

use crate::requests::AgentTurnOutcome;
use crate::slash::SlashCommand;

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct GoalCheckVerdict {
    pub(crate) satisfied: bool,
    pub(crate) reason: String,
    pub(crate) usage: Usage,
}

pub(crate) async fn check_goal_satisfied<P>(
    provider: &P,
    model: &str,
    objective: &str,
    outcomes: &[AgentTurnOutcome],
) -> PeriResult<GoalCheckVerdict>
where
    P: LlmProvider + ?Sized,
{
    let response = provider
        .complete(CompletionRequest {
            model: model.to_string(),
            system: Some(
                "You are Peridot's independent goal checker. Decide whether the objective is fully satisfied. Respond as JSON: {\"satisfied\":true|false,\"reason\":\"short reason\"}."
                    .to_string(),
            ),
            messages: vec![LlmMessage::new(
                MessageRole::User,
                goal_checker_prompt(objective, outcomes),
            )],
            max_tokens: Some(512),
            thinking: false,
        })
        .await?;
    let (satisfied, reason) = parse_goal_checker_response(&response.text);
    Ok(GoalCheckVerdict {
        satisfied,
        reason,
        usage: response.usage,
    })
}

fn goal_checker_prompt(objective: &str, outcomes: &[AgentTurnOutcome]) -> String {
    let recent = outcomes
        .iter()
        .rev()
        .take(5)
        .rev()
        .map(|outcome| {
            format!(
                "- tool={} success={} summary={}",
                outcome.tool_name, outcome.tool_result.success, outcome.tool_result.summary
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    format!("Objective:\n{objective}\n\nRecent tool outcomes:\n{recent}")
}

fn parse_goal_checker_response(text: &str) -> (bool, String) {
    if let Ok(value) = serde_json::from_str::<serde_json::Value>(text) {
        let satisfied = value
            .get("satisfied")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false);
        let reason = value
            .get("reason")
            .and_then(serde_json::Value::as_str)
            .unwrap_or(if satisfied {
                "satisfied"
            } else {
                "not satisfied"
            });
        return (satisfied, reason.to_string());
    }
    let normalized = text.trim().to_ascii_lowercase();
    let satisfied = matches!(normalized.as_str(), "true" | "yes" | "satisfied");
    (satisfied, text.trim().to_string())
}

/// Goal execution status.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum GoalStatus {
    /// Goal is actively running.
    Running,
    /// Goal is paused.
    Paused,
    /// Goal completed.
    Done,
    /// Goal was cleared.
    Cleared,
}

/// Runtime guardrails for a goal-mode task.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct GoalController {
    /// Durable objective text.
    pub objective: String,
    /// Current goal status.
    pub status: GoalStatus,
    /// Maximum turn count.
    pub max_turns: u32,
    /// Current turn count.
    pub turns_used: u32,
    /// Budget cap in USD.
    pub budget_usd: f64,
    /// Current cost in USD.
    pub cost_usd: f64,
}

impl GoalController {
    /// Creates a running goal controller.
    pub fn new(objective: impl Into<String>, max_turns: u32, budget_usd: f64) -> Self {
        Self {
            objective: objective.into(),
            status: GoalStatus::Running,
            max_turns,
            turns_used: 0,
            budget_usd,
            cost_usd: 0.0,
        }
    }

    /// Applies a goal-specific slash command.
    pub fn apply(&mut self, command: &SlashCommand) {
        match command {
            SlashCommand::GoalPause => self.status = GoalStatus::Paused,
            SlashCommand::GoalResume => self.status = GoalStatus::Running,
            SlashCommand::GoalClear => self.status = GoalStatus::Cleared,
            _ => {}
        }
    }

    /// Records one completed turn and added cost.
    pub fn record_turn(&mut self, cost_usd: f64) {
        self.turns_used += 1;
        self.cost_usd += cost_usd;
    }

    /// Returns true when a guardrail requires stopping.
    pub fn should_stop(&self) -> bool {
        matches!(
            self.status,
            GoalStatus::Paused | GoalStatus::Done | GoalStatus::Cleared
        ) || self.turns_used >= self.max_turns
            || self.cost_usd >= self.budget_usd
    }
}
