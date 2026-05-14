//! Subagent orchestration contracts.

use async_trait::async_trait;
use peridot_common::PeriResult;
use serde::{Deserialize, Serialize};

/// Subagent type.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SubAgentKind {
    /// Same workspace, isolated context.
    Fork,
    /// Separate git worktree.
    Worktree,
    /// Long-running teammate agent.
    Teammate,
}

/// Request to run a subagent.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SubAgentTask {
    /// Task prompt.
    pub prompt: String,
    /// Desired subagent kind.
    pub kind: SubAgentKind,
}

/// Result returned by a subagent.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SubAgentResult {
    /// Whether the task succeeded.
    pub success: bool,
    /// Summary of work completed.
    pub summary: String,
}

/// Trait implemented by subagent runners.
#[async_trait]
pub trait SubAgent: Send + Sync {
    /// Runs a subagent task.
    async fn run(&self, task: SubAgentTask) -> PeriResult<SubAgentResult>;
}
