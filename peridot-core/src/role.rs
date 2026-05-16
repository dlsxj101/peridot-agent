//! Multi-LLM committee role. Each `HarnessAgent` carries one role and the
//! system prompt picks up role-specific guidance on top of the mode-specific
//! prompt. `Executor` is the legacy single-agent role; new committee roles
//! `Planner` and `Reviewer` exist alongside it.

use serde::{Deserialize, Serialize};

/// One of the three roles a `HarnessAgent` can play inside the committee.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentRole {
    /// Read-only Planner that produces a task plan before the executor runs.
    Planner,
    /// Single-turn Reviewer that inspects each mutating turn's diff.
    Reviewer,
    /// Default role: the agent loop that actually executes tools and writes
    /// files. Matches the legacy single-agent behaviour when committee mode
    /// is `Off`.
    #[default]
    Executor,
}

impl AgentRole {
    /// Returns the role-specific guidance appended to the system prompt.
    /// Empty for `Executor` so the legacy prompt is unchanged when the
    /// committee is disabled.
    pub fn system_prompt_suffix(self) -> &'static str {
        match self {
            AgentRole::Planner => {
                "\n\nROLE: Planner\n\
                 - You are the planner in a three-role committee.\n\
                 - Produce a concise, structured plan for the requested task.\n\
                 - Use read-only tools only (file_read, project_scan, plan_show).\n\
                 - Do NOT call file_write, file_patch, shell_exec, or any mutating tool.\n\
                 - Stop after emitting the plan as your final assistant message.\n\
                 - The plan is consumed verbatim by the Executor agent.\n"
            }
            AgentRole::Reviewer => {
                "\n\nROLE: Reviewer\n\
                 - You review one diff produced by the Executor agent.\n\
                 - Respond with a single JSON object: \
                   {\"verdict\":\"approve\"|\"request_changes\"|\"block\",\
                   \"comments\":\"...\"}.\n\
                 - approve: the diff is correct and may land.\n\
                 - request_changes: the diff has fixable issues described in `comments`.\n\
                 - block: the diff has a fundamental problem requiring operator override.\n\
                 - Be terse and actionable in `comments`. Do not write code blocks.\n\
                 - Do NOT call any tool.\n"
            }
            AgentRole::Executor => "",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn executor_is_the_default() {
        assert_eq!(AgentRole::default(), AgentRole::Executor);
    }

    #[test]
    fn executor_prompt_suffix_is_empty_for_backwards_compatibility() {
        assert!(AgentRole::Executor.system_prompt_suffix().is_empty());
    }

    #[test]
    fn planner_and_reviewer_prompts_carry_role_marker() {
        assert!(
            AgentRole::Planner
                .system_prompt_suffix()
                .contains("ROLE: Planner")
        );
        assert!(
            AgentRole::Reviewer
                .system_prompt_suffix()
                .contains("ROLE: Reviewer")
        );
    }
}
