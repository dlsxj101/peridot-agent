//! Subagent orchestration contracts.

use std::path::{Path, PathBuf};

use async_trait::async_trait;
use peridot_common::{PeriError, PeriResult};
use peridot_git::GitManager;
use serde::{Deserialize, Serialize};

const MAX_WORKTREE_SLUG_CHARS: usize = 48;
const WORKTREE_SLUG_HASH_CHARS: usize = 12;

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
    /// Optional model tier for the subagent.
    #[serde(default)]
    pub model_tier: Option<ModelTier>,
}

/// Lightweight reference to a piece of evidence the subagent produced
/// or consumed — a file path with optional line range or a digest of an
/// external response. Parents are expected to *re-verify* these
/// references (read the file at the cited lines, re-hash the response)
/// before treating the subagent's claims as ground truth.
///
/// Wire-compatible with the `EvidenceRef` type in `peridot-context` —
/// we re-declare a tiny serialisable shape here to avoid a runtime
/// dependency cycle (peridot-agents -> peridot-context -> peridot-agents).
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SubAgentEvidenceRef {
    /// Stable category for the evidence, e.g. `"file"`, `"url"`,
    /// `"command_output"`.
    pub kind: String,
    /// Free-text identifier — file path, URL, command line, etc. The
    /// parent uses this to look the evidence up locally.
    pub id: String,
    /// Optional line range or digest summary. Parents can match these
    /// against their own [`peridot_context::EvidenceLedger`] to confirm
    /// they actually inspected the cited region.
    #[serde(default)]
    pub summary: Option<String>,
}

/// Result returned by a subagent.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SubAgentResult {
    /// Whether the task succeeded.
    pub success: bool,
    /// Summary of work completed.
    pub summary: String,
    /// Subagent kind that handled the task.
    pub kind: SubAgentKind,
    /// Optional isolated workspace path.
    #[serde(default)]
    pub workspace: Option<PathBuf>,
    /// Captured `git diff` of the subagent's workspace at exit. Empty
    /// when the runner only prepared the workspace (no inner agent
    /// execution) or when `git` was unavailable. The parent harness
    /// folds this into a `[sub-agent review]` PlanReminder so the
    /// caller actually inspects the change instead of trusting the
    /// summary text.
    #[serde(default)]
    pub diff: String,
    /// References to evidence the subagent inspected or produced. The
    /// parent is expected to re-read at least one entry before trusting
    /// the subagent's summary; the [`crate::SubAgentReviewPolicy`] in
    /// peridot-core downgrades the result to an untrusted summary entry
    /// when this is empty, instead of injecting it as a trusted plan
    /// reminder.
    ///
    /// Optional for wire compatibility with older runners that did not
    /// populate it; empty (`[]`) deserialises cleanly.
    #[serde(default)]
    pub evidence_refs: Vec<SubAgentEvidenceRef>,
}

/// Trait implemented by subagent runners.
#[async_trait]
pub trait SubAgent: Send + Sync {
    /// Runs a subagent task.
    async fn run(&self, task: SubAgentTask) -> PeriResult<SubAgentResult>;
}

/// Local subagent runner for fork/worktree orchestration.
#[derive(Clone, Debug)]
pub struct LocalSubAgentRunner {
    project_root: PathBuf,
    worktrees_root: PathBuf,
}

impl LocalSubAgentRunner {
    /// Creates a local subagent runner.
    pub fn new(project_root: impl Into<PathBuf>, worktrees_root: impl Into<PathBuf>) -> Self {
        Self {
            project_root: project_root.into(),
            worktrees_root: worktrees_root.into(),
        }
    }
}

#[async_trait]
impl SubAgent for LocalSubAgentRunner {
    /// Prepares the workspace for a subagent task without executing
    /// it. The actual LLM loop is the harness's job (`InnerLoopSubAgent`
    /// in `peridot-core`) — this crate stays LLM-free so the
    /// dependency graph keeps `peridot-tools` → `peridot-agents`
    /// without a cycle back through `peridot-core`/`peridot-llm`.
    ///
    /// Per-kind preparation:
    /// * `Fork` — reuses the parent workspace as-is. The execution
    ///   step gets a fresh `HarnessAgent` but works against the same
    ///   files. Returns the parent root as `workspace`.
    /// * `Worktree` — materialises a real `git worktree` under
    ///   `<worktrees_root>/<slug>` on branch
    ///   `codex/subagent-<slug>`. Caller is responsible for tearing
    ///   it down via `GitManager::remove_worktree` when the session
    ///   ends.
    /// * `Teammate` — same physical isolation as `Worktree`. The
    ///   distinction is bookkeeping: teammates carry parent↔child
    ///   message channels (`agent_message`) and have a longer
    ///   intended lifetime, but the file isolation needs are identical
    ///   so we share the worktree machinery instead of running unisolated.
    async fn run(&self, task: SubAgentTask) -> PeriResult<SubAgentResult> {
        match task.kind {
            SubAgentKind::Fork => Ok(SubAgentResult {
                success: true,
                summary: format!(
                    "fork workspace prepared (shared with parent) for task: {}",
                    task.prompt
                ),
                kind: SubAgentKind::Fork,
                workspace: Some(self.project_root.clone()),
                diff: String::new(),
                // Workspace-prep paths don't produce evidence — that
                // happens later, inside the inner-agent run if one is
                // scheduled. Leave empty so the parent's review policy
                // can decide how to treat the orientation-only output.
                evidence_refs: Vec::new(),
            }),
            SubAgentKind::Worktree => {
                let plan =
                    WorktreePlan::new(&self.project_root, &self.worktrees_root, &task.prompt)?;
                std::fs::create_dir_all(&self.worktrees_root).map_err(|err| {
                    PeriError::Tool(format!(
                        "failed to create worktrees root {}: {err}",
                        self.worktrees_root.display()
                    ))
                })?;
                GitManager::new(&self.project_root).add_worktree(&plan.path, &plan.branch)?;
                Ok(SubAgentResult {
                    success: true,
                    summary: format!("worktree subagent prepared on {}", plan.branch),
                    kind: SubAgentKind::Worktree,
                    workspace: Some(plan.path),
                    diff: String::new(),
                    evidence_refs: Vec::new(),
                })
            }
            SubAgentKind::Teammate => {
                // Teammates inherit the same physical isolation as worktree
                // subagents — the difference is only in lifecycle
                // (long-running) and routing (parent↔child message bus).
                // Sharing the worktree path avoids the two-tier
                // "Fork still string-only / Worktree real" inconsistency
                // that v0.5.x carried.
                let plan =
                    WorktreePlan::new(&self.project_root, &self.worktrees_root, &task.prompt)?;
                std::fs::create_dir_all(&self.worktrees_root).map_err(|err| {
                    PeriError::Tool(format!(
                        "failed to create worktrees root {}: {err}",
                        self.worktrees_root.display()
                    ))
                })?;
                GitManager::new(&self.project_root).add_worktree(&plan.path, &plan.branch)?;
                Ok(SubAgentResult {
                    success: true,
                    summary: format!("teammate subagent prepared on {}", plan.branch),
                    kind: SubAgentKind::Teammate,
                    workspace: Some(plan.path),
                    diff: String::new(),
                    evidence_refs: Vec::new(),
                })
            }
        }
    }
}

/// Model tier selected for a subagent.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelTier {
    /// Cheap/fast model for straightforward checks.
    Haiku,
    /// Main model for normal coding work.
    Main,
    /// Strongest model for architecture and audit work.
    Opus,
}

/// Deterministic subagent selection policy.
#[derive(Clone, Debug, Default)]
pub struct SubAgentPolicy;

impl SubAgentPolicy {
    /// Selects a subagent kind and model tier from task text.
    pub fn select(&self, prompt: &str) -> (SubAgentKind, ModelTier) {
        let lower = prompt.to_lowercase();
        if contains_any(&lower, &["test", "format", "lint", "doc", "search"]) {
            return (SubAgentKind::Fork, ModelTier::Main);
        }
        if contains_any(
            &lower,
            &["architecture", "security", "audit", "performance", "design"],
        ) {
            return (SubAgentKind::Teammate, ModelTier::Main);
        }
        if contains_any(
            &lower,
            &[
                "refactor",
                "many files",
                "large change",
                "parallel",
                "worktree",
            ],
        ) {
            return (SubAgentKind::Worktree, ModelTier::Main);
        }
        (SubAgentKind::Fork, ModelTier::Main)
    }
}

/// Planned git worktree isolation for a subagent.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct WorktreePlan {
    /// Branch name to create.
    pub branch: String,
    /// Worktree path.
    pub path: PathBuf,
    /// Non-interactive command arguments for `git worktree add`.
    pub add_args: Vec<String>,
}

impl WorktreePlan {
    /// Creates a deterministic worktree plan.
    pub fn new(
        project_root: impl AsRef<Path>,
        worktrees_root: impl AsRef<Path>,
        task_id: &str,
    ) -> PeriResult<Self> {
        let task_slug = slug(task_id)?;
        let branch = format!("codex/subagent-{task_slug}");
        let path = worktrees_root.as_ref().join(&task_slug);
        let add_args = vec![
            "worktree".to_string(),
            "add".to_string(),
            "-b".to_string(),
            branch.clone(),
            path.display().to_string(),
            "HEAD".to_string(),
        ];
        let project_root = project_root.as_ref();
        if project_root == path {
            return Err(PeriError::Config(
                "worktree path must differ from project root".to_string(),
            ));
        }
        Ok(Self {
            branch,
            path,
            add_args,
        })
    }
}

fn contains_any(text: &str, needles: &[&str]) -> bool {
    needles.iter().any(|needle| text.contains(needle))
}

fn slug(value: &str) -> PeriResult<String> {
    let raw_slug = value
        .chars()
        .filter_map(|ch| {
            if ch.is_ascii_alphanumeric() {
                Some(ch.to_ascii_lowercase())
            } else if ch == '-' || ch == '_' || ch.is_whitespace() {
                Some('-')
            } else {
                None
            }
        })
        .collect::<String>()
        .split('-')
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("-");
    if raw_slug.is_empty() {
        return Err(PeriError::Config("subagent task id is empty".to_string()));
    }
    let slug = if raw_slug.chars().count() > MAX_WORKTREE_SLUG_CHARS {
        let prefix_len = MAX_WORKTREE_SLUG_CHARS.saturating_sub(WORKTREE_SLUG_HASH_CHARS + 1);
        let prefix = raw_slug
            .chars()
            .take(prefix_len)
            .collect::<String>()
            .trim_matches('-')
            .to_string();
        format!("{prefix}-{}", stable_slug_hash(value))
    } else {
        raw_slug
    };
    Ok(slug)
}

fn stable_slug_hash(value: &str) -> String {
    let mut hash = 0xcbf29ce484222325_u64;
    for byte in value.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    format!("{hash:016x}")
        .chars()
        .take(WORKTREE_SLUG_HASH_CHARS)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selects_fork_for_tests_on_main_model() {
        let policy = SubAgentPolicy;

        assert_eq!(
            policy.select("write tests for parser"),
            (SubAgentKind::Fork, ModelTier::Main)
        );
    }

    #[test]
    fn selects_teammate_for_security_audit() {
        let policy = SubAgentPolicy;

        assert_eq!(
            policy.select("security audit the permission model"),
            (SubAgentKind::Teammate, ModelTier::Main)
        );
    }

    #[test]
    fn creates_worktree_plan() {
        let plan = WorktreePlan::new("/repo", "/tmp/peridot-worktrees", "Large Refactor").unwrap();

        assert_eq!(plan.branch, "codex/subagent-large-refactor");
        assert!(plan.path.ends_with("large-refactor"));
        assert_eq!(plan.add_args[0], "worktree");
        assert_eq!(plan.add_args[2], "-b");
    }

    #[test]
    fn worktree_plan_truncates_long_task_slug_with_stable_hash() {
        let long_task =
            "Parent context packet: use this as intent and evidence index, not as proof. "
                .repeat(20);
        let plan = WorktreePlan::new("/repo", "/tmp/peridot-worktrees", &long_task).unwrap();
        let slug = plan
            .branch
            .strip_prefix("codex/subagent-")
            .expect("subagent branch prefix");

        assert!(slug.chars().count() <= MAX_WORKTREE_SLUG_CHARS);
        assert!(plan.branch.chars().count() < 100);
        assert!(plan.path.file_name().unwrap().to_string_lossy().len() < 100);
        assert_eq!(
            WorktreePlan::new("/repo", "/tmp/peridot-worktrees", &long_task)
                .unwrap()
                .branch,
            plan.branch
        );
    }

    #[tokio::test]
    async fn local_runner_creates_worktree_for_task() {
        let root = std::env::temp_dir().join(format!("peridot-agents-{}", std::process::id()));
        let worktrees =
            std::env::temp_dir().join(format!("peridot-agents-worktrees-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        std::process::Command::new("git")
            .arg("init")
            .current_dir(&root)
            .output()
            .unwrap();
        std::process::Command::new("git")
            .args(["config", "user.email", "peridot@example.com"])
            .current_dir(&root)
            .output()
            .unwrap();
        std::process::Command::new("git")
            .args(["config", "user.name", "Peridot Test"])
            .current_dir(&root)
            .output()
            .unwrap();
        std::fs::write(root.join("README.md"), "hello\n").unwrap();
        GitManager::new(&root).commit_all("chore: initial").unwrap();
        let runner = LocalSubAgentRunner::new(&root, &worktrees);

        let result = runner
            .run(SubAgentTask {
                prompt: "large refactor".to_string(),
                kind: SubAgentKind::Worktree,
                model_tier: Some(ModelTier::Main),
            })
            .await
            .unwrap();

        let workspace = result.workspace.unwrap();
        assert!(workspace.join("README.md").exists());
        GitManager::new(&root).remove_worktree(&workspace).unwrap();
        std::fs::remove_dir_all(root).unwrap();
    }
}
