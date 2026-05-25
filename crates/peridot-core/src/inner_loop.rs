//! Real subagent runner: spawns a bounded [`HarnessAgent`] in a prepared
//! workspace and runs the delegated task to completion.
//!
//! `peridot-agents::LocalSubAgentRunner` only prepares workspaces — it
//! creates the worktree / fork directory and returns a placeholder
//! `SubAgentResult` without actually executing anything. That keeps the
//! contract crate dependency-free (it cannot reach `peridot-llm` /
//! `peridot-core` without inducing a cycle).
//!
//! `InnerLoopSubAgent` lives in `peridot-core` because that's the first
//! crate that owns both the harness and the LLM provider trait. It
//! delegates workspace preparation to `LocalSubAgentRunner`, then runs a
//! fresh `HarnessAgent` against the prepared root with a small turn /
//! budget cap so a runaway subagent can't burn the parent's headroom.
//!
//! Recursion safety: the inner agent's `ToolContext` is built **without**
//! a runner attached, so any `agent_delegate` call the subagent itself
//! makes falls back to `LocalSubAgentRunner` (prepare-only). This caps
//! delegation depth at one and avoids accidental fork bombs.

use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use peridot_agents::{LocalSubAgentRunner, ModelTier, SubAgent, SubAgentResult, SubAgentTask};
use peridot_common::{
    ExecutionMode, HooksConfig, PeriResult, PermissionMode, ReasoningEffort, SecurityConfig,
};
use peridot_context::ContextManager;
use peridot_llm::LlmProvider;
use peridot_tools::{ToolRegistry, register_builtin_tools};

use crate::agent::HarnessAgent;
use crate::requests::{AgentRunRequest, StopReason};
use crate::state::AgentState;

/// Subagent runner that actually executes the delegated task through a
/// fresh bounded `HarnessAgent`. Plug into [`HarnessAgent::set_subagent_runner`]
/// (or pass through `ToolContext::with_subagent_runner`) to upgrade
/// `agent_delegate` from prepare-only to real execution.
#[derive(Clone)]
pub struct InnerLoopSubAgent {
    provider: Arc<dyn LlmProvider>,
    project_root: PathBuf,
    worktrees_root: PathBuf,
    haiku_model: String,
    main_model: String,
    opus_model: String,
    max_turns: u32,
    max_tokens: u32,
    budget_usd: f64,
    permission: PermissionMode,
    security: SecurityConfig,
    reasoning_effort: ReasoningEffort,
}

impl InnerLoopSubAgent {
    /// Creates a runner anchored at `project_root`. Defaults mirror the
    /// conservative behaviour `LocalSubAgentRunner` used to emit: 8
    /// turns, $0 budget (unbounded — the parent run is already capped),
    /// `Auto` permission, no sandbox change.
    pub fn new(
        provider: Arc<dyn LlmProvider>,
        project_root: impl Into<PathBuf>,
        main_model: impl Into<String>,
    ) -> Self {
        let project_root = project_root.into();
        let worktrees_root = project_root.join(".peridot/worktrees");
        let main = main_model.into();
        Self {
            provider,
            project_root,
            worktrees_root,
            haiku_model: main.clone(),
            main_model: main.clone(),
            opus_model: main,
            max_turns: 8,
            max_tokens: 4096,
            budget_usd: 0.0,
            permission: PermissionMode::Auto,
            security: SecurityConfig::default(),
            reasoning_effort: ReasoningEffort::Off,
        }
    }

    /// Overrides the per-tier model names. Tier mapping:
    /// `Haiku` → cheap/fast, `Main` → default, `Opus` → strongest.
    pub fn with_models(
        mut self,
        haiku: impl Into<String>,
        main: impl Into<String>,
        opus: impl Into<String>,
    ) -> Self {
        self.haiku_model = haiku.into();
        self.main_model = main.into();
        self.opus_model = opus.into();
        self
    }

    /// Overrides the per-run turn cap (default 8).
    pub fn with_max_turns(mut self, max_turns: u32) -> Self {
        self.max_turns = max_turns;
        self
    }

    /// Overrides the per-turn max tokens (default 4096).
    pub fn with_max_tokens(mut self, max_tokens: u32) -> Self {
        self.max_tokens = max_tokens;
        self
    }

    /// Overrides the cost ceiling (default 0.0, unbounded).
    pub fn with_budget_usd(mut self, budget_usd: f64) -> Self {
        self.budget_usd = budget_usd;
        self
    }

    /// Overrides the permission mode the inner agent runs under.
    pub fn with_permission(mut self, permission: PermissionMode) -> Self {
        self.permission = permission;
        self
    }

    /// Overrides the sandbox / security profile.
    pub fn with_security(mut self, security: SecurityConfig) -> Self {
        self.security = security;
        self
    }

    /// Overrides the directory used as the worktree root.
    pub fn with_worktrees_root(mut self, worktrees_root: impl Into<PathBuf>) -> Self {
        self.worktrees_root = worktrees_root.into();
        self
    }

    /// Overrides the reasoning intensity forwarded to the provider.
    pub fn with_reasoning_effort(mut self, effort: ReasoningEffort) -> Self {
        self.reasoning_effort = effort;
        self
    }

    fn resolve_model(&self, tier: Option<&ModelTier>) -> String {
        match tier {
            Some(ModelTier::Haiku) => self.haiku_model.clone(),
            Some(ModelTier::Opus) => self.opus_model.clone(),
            _ => self.main_model.clone(),
        }
    }
}

#[async_trait]
impl SubAgent for InnerLoopSubAgent {
    async fn run(&self, task: SubAgentTask) -> PeriResult<SubAgentResult> {
        let prep = LocalSubAgentRunner::new(&self.project_root, &self.worktrees_root)
            .run(task.clone())
            .await?;
        let workspace = prep
            .workspace
            .clone()
            .unwrap_or_else(|| self.project_root.clone());
        let model = self.resolve_model(task.model_tier.as_ref());

        let mut registry = ToolRegistry::new();
        register_builtin_tools(&mut registry)?;
        let context = ContextManager::new();
        let state = AgentState::new(ExecutionMode::Execute, self.permission);
        let mut agent = HarnessAgent::new(state, context, registry);

        let request = AgentRunRequest {
            task: task.prompt.clone(),
            model,
            goal_checker_model: None,
            max_turns: self.max_turns,
            max_tokens: self.max_tokens,
            reasoning_effort: self.reasoning_effort,
            service_tier: None,
            budget_usd: self.budget_usd,
            budget_warning_pct: 80,
            project_root: workspace.clone(),
            denied_paths: Vec::new(),
            hooks: HooksConfig::default(),
            security: self.security.clone(),
        };
        let summary = agent.run_until_done(&*self.provider, request).await?;
        let success = matches!(summary.stopped_reason, StopReason::Done);
        let summary_text = summary
            .turns
            .last()
            .map(|outcome| outcome.tool_result.summary.clone())
            .filter(|text| !text.is_empty())
            .unwrap_or_else(|| format!("subagent stopped: {:?}", summary.stopped_reason));

        // Capture the workspace diff so the parent harness can fold
        // it into a [sub-agent review] PlanReminder instead of
        // trusting the summary text. Best-effort: `git` may be
        // missing or the workspace may not be a repo.
        let diff = std::process::Command::new("git")
            .args(["diff", "HEAD"])
            .current_dir(&workspace)
            .output()
            .ok()
            .map(|out| String::from_utf8_lossy(&out.stdout).to_string())
            .unwrap_or_default();

        Ok(SubAgentResult {
            success,
            summary: summary_text,
            kind: prep.kind,
            workspace: Some(workspace),
            diff,
            // Evidence-ref protocol (PR 13): future inner-loop work
            // will harvest `EvidenceLedger` entries from the sub-run's
            // context manager here. For now leave empty so the parent's
            // review policy can choose how to treat the result — the
            // captured `diff` already provides the primary review surface.
            evidence_refs: Vec::new(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use peridot_agents::SubAgentKind;
    use peridot_llm::{
        AuthMethod, CompletionRequest, CompletionResponse, PricingTable, ToolInvocation, Usage,
    };
    use std::sync::Mutex;

    struct ScriptedProvider {
        responses: Mutex<Vec<CompletionResponse>>,
    }

    impl ScriptedProvider {
        fn new(responses: Vec<CompletionResponse>) -> Self {
            Self {
                responses: Mutex::new(responses),
            }
        }
    }

    #[async_trait]
    impl LlmProvider for ScriptedProvider {
        async fn complete(&self, _req: CompletionRequest) -> PeriResult<CompletionResponse> {
            Ok(self.responses.lock().unwrap().remove(0))
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
    async fn inner_loop_runs_until_agent_done() {
        let root = std::env::temp_dir().join(format!(
            "peridot-inner-loop-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let provider = Arc::new(ScriptedProvider::new(vec![CompletionResponse {
            text: String::new(),
            tool_calls: vec![ToolInvocation {
                id: "call_1".to_string(),
                name: "agent_done".to_string(),
                arguments: serde_json::json!({ "summary": "fork subagent finished" }),
            }],
            reasoning_content: None,
            usage: Usage::default(),
        }]));
        let runner = InnerLoopSubAgent::new(provider, &root, "test-model").with_max_turns(2);

        let result = runner
            .run(SubAgentTask {
                prompt: "summarize the README".to_string(),
                kind: SubAgentKind::Fork,
                model_tier: Some(ModelTier::Main),
            })
            .await
            .unwrap();

        assert!(result.success, "expected agent_done to mark success");
        assert!(
            result.summary.contains("fork subagent finished"),
            "summary = {}",
            result.summary
        );
        assert_eq!(result.kind, SubAgentKind::Fork);
        std::fs::remove_dir_all(&root).unwrap();
    }
}
