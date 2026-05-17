//! Subagent orchestration contracts.

use std::path::{Path, PathBuf};

use async_trait::async_trait;
use peridot_common::{PeriError, PeriResult};
use peridot_git::GitManager;
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
    /// Optional model tier for the subagent.
    #[serde(default)]
    pub model_tier: Option<ModelTier>,
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
    async fn run(&self, task: SubAgentTask) -> PeriResult<SubAgentResult> {
        match task.kind {
            SubAgentKind::Fork => Ok(SubAgentResult {
                success: true,
                summary: format!("fork subagent prepared: {}", task.prompt),
                kind: SubAgentKind::Fork,
                workspace: Some(self.project_root.clone()),
                diff: String::new(),
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
                })
            }
            SubAgentKind::Teammate => Ok(SubAgentResult {
                success: true,
                summary: format!("teammate subagent queued: {}", task.prompt),
                kind: SubAgentKind::Teammate,
                workspace: Some(self.project_root.clone()),
                diff: String::new(),
            }),
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
            return (SubAgentKind::Fork, ModelTier::Haiku);
        }
        if contains_any(
            &lower,
            &["architecture", "security", "audit", "performance", "design"],
        ) {
            return (SubAgentKind::Teammate, ModelTier::Opus);
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
    let slug = value
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
    if slug.is_empty() {
        return Err(PeriError::Config("subagent task id is empty".to_string()));
    }
    Ok(slug)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selects_lightweight_fork_for_tests() {
        let policy = SubAgentPolicy;

        assert_eq!(
            policy.select("write tests for parser"),
            (SubAgentKind::Fork, ModelTier::Haiku)
        );
    }

    #[test]
    fn selects_teammate_for_security_audit() {
        let policy = SubAgentPolicy;

        assert_eq!(
            policy.select("security audit the permission model"),
            (SubAgentKind::Teammate, ModelTier::Opus)
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
