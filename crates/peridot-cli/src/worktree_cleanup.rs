//! Startup reconciliation for Peridot-managed git worktrees.

use std::path::{Path, PathBuf};

use peridot_git::GitManager;
use peridot_memory::{MemoryStore, SessionLifecycle, SessionRecord};
use serde::Serialize;

/// Summary of stale session/worktree reconciliation.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize)]
pub(crate) struct WorktreeCleanupReport {
    /// Session ids transitioned from `running` to `suspended`.
    pub suspended_sessions: Vec<String>,
    /// Clean Peridot worktrees removed from git and disk.
    pub removed_worktrees: Vec<WorktreeCleanupItem>,
    /// Worktrees left in place because removing them could lose work.
    pub preserved_worktrees: Vec<WorktreeCleanupItem>,
    /// Records that pointed at an already-missing worktree path.
    pub missing_worktrees: Vec<WorktreeCleanupItem>,
    /// Non-fatal cleanup errors.
    pub errors: Vec<WorktreeCleanupError>,
}

impl WorktreeCleanupReport {
    /// Returns true when reconciliation found nothing to report.
    pub(crate) fn is_empty(&self) -> bool {
        self.suspended_sessions.is_empty()
            && self.removed_worktrees.is_empty()
            && self.preserved_worktrees.is_empty()
            && self.missing_worktrees.is_empty()
            && self.errors.is_empty()
    }

    /// Human-facing one-line summary for TUI / editor status surfaces.
    pub(crate) fn summary(&self) -> Option<String> {
        if self.is_empty() {
            return None;
        }
        let mut parts = Vec::new();
        if !self.suspended_sessions.is_empty() {
            parts.push(format!(
                "{} stale session(s) suspended",
                self.suspended_sessions.len()
            ));
        }
        if !self.removed_worktrees.is_empty() {
            parts.push(format!(
                "{} clean worktree(s) removed",
                self.removed_worktrees.len()
            ));
        }
        if !self.preserved_worktrees.is_empty() {
            parts.push(format!(
                "{} dirty worktree(s) preserved",
                self.preserved_worktrees.len()
            ));
        }
        if !self.missing_worktrees.is_empty() {
            parts.push(format!(
                "{} missing worktree record(s) reconciled",
                self.missing_worktrees.len()
            ));
        }
        if !self.errors.is_empty() {
            parts.push(format!("{} cleanup error(s)", self.errors.len()));
        }
        Some(parts.join("; "))
    }
}

/// One worktree cleanup decision.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub(crate) struct WorktreeCleanupItem {
    /// Session id that owned the worktree.
    pub session_id: String,
    /// Path recorded for the session's isolated worktree.
    pub path: PathBuf,
    /// Git branch recorded for the worktree, when present.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    /// Reason for the decision.
    pub reason: String,
    /// Number of changed files, for preserved dirty worktrees.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub changed_files: Option<usize>,
}

/// Non-fatal worktree cleanup error.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub(crate) struct WorktreeCleanupError {
    /// Session id being reconciled.
    pub session_id: String,
    /// Worktree path involved in the failure.
    pub path: PathBuf,
    /// Error text.
    pub message: String,
}

/// Reconciles sessions left `running` by an unclean shutdown.
///
/// Only Peridot-managed worktrees under `<project>/.peridot/worktrees/` are
/// candidates for automatic removal. Clean worktrees are removed; dirty ones
/// are kept so operator changes are never discarded.
pub(crate) fn reconcile_stale_worktrees(project_root: &Path) -> WorktreeCleanupReport {
    let store = MemoryStore::new(project_root.join(".peridot/memory.db"));
    let Ok(records) = store.list_session_records() else {
        return WorktreeCleanupReport::default();
    };
    let now = crate::run_state::unix_timestamp();
    let mut report = WorktreeCleanupReport::default();
    for mut record in records {
        if record.status != SessionLifecycle::Running {
            continue;
        }
        record.status = SessionLifecycle::Suspended;
        record.updated_at_unix = now;
        report.suspended_sessions.push(record.id.clone());
        if record.worktree_branch.is_some() {
            reconcile_record_worktree(project_root, &mut record, &mut report);
        }
        let _ = store.save_session_record(&record);
    }
    report
}

fn reconcile_record_worktree(
    project_root: &Path,
    record: &mut SessionRecord,
    report: &mut WorktreeCleanupReport,
) {
    let worktrees_root = project_root.join(".peridot/worktrees");
    let path = record.workspace_root.clone();
    if !path.starts_with(&worktrees_root) {
        report.preserved_worktrees.push(cleanup_item(
            record,
            "outside Peridot worktree root",
            None,
        ));
        return;
    }
    if !path.exists() {
        let _ = GitManager::new(project_root).prune_worktrees();
        report
            .missing_worktrees
            .push(cleanup_item(record, "path already missing", None));
        record.workspace_root = project_root.to_path_buf();
        record.worktree_branch = None;
        return;
    }
    let status = match GitManager::new(&path).status() {
        Ok(status) => status,
        Err(err) => {
            report.errors.push(WorktreeCleanupError {
                session_id: record.id.clone(),
                path,
                message: format!("failed to inspect worktree status: {err}"),
            });
            return;
        }
    };
    if !status.changed_files.is_empty() {
        report.preserved_worktrees.push(cleanup_item(
            record,
            "contains uncommitted changes",
            Some(status.changed_files.len()),
        ));
        return;
    }
    match GitManager::new(project_root).remove_worktree(&path) {
        Ok(_) => {
            let _ = GitManager::new(project_root).prune_worktrees();
            report
                .removed_worktrees
                .push(cleanup_item(record, "clean stale worktree", None));
            record.workspace_root = project_root.to_path_buf();
            record.worktree_branch = None;
        }
        Err(err) => report.errors.push(WorktreeCleanupError {
            session_id: record.id.clone(),
            path,
            message: format!("failed to remove worktree: {err}"),
        }),
    }
}

fn cleanup_item(
    record: &SessionRecord,
    reason: impl Into<String>,
    changed_files: Option<usize>,
) -> WorktreeCleanupItem {
    WorktreeCleanupItem {
        session_id: record.id.clone(),
        path: record.workspace_root.clone(),
        branch: record.worktree_branch.clone(),
        reason: reason.into(),
        changed_files,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::process::Command;

    #[test]
    fn removes_clean_stale_running_worktree() {
        let Some((root, worktree, branch)) = temp_repo_with_worktree("clean") else {
            return;
        };
        let store = MemoryStore::new(root.join(".peridot/memory.db"));
        let mut record = SessionRecord::new("stale-clean", &worktree);
        record.status = SessionLifecycle::Running;
        record.created_at_unix = 1;
        record.updated_at_unix = 2;
        record.worktree_branch = Some(branch.clone());
        store.save_session_record(&record).unwrap();

        let report = reconcile_stale_worktrees(&root);

        assert_eq!(report.suspended_sessions, vec!["stale-clean"]);
        assert_eq!(report.removed_worktrees.len(), 1);
        assert!(!worktree.exists());
        let updated = store.get_session_record("stale-clean").unwrap().unwrap();
        assert_eq!(updated.status, SessionLifecycle::Suspended);
        assert_eq!(updated.workspace_root, root);
        assert_eq!(updated.worktree_branch, None);
        fs::remove_dir_all(updated.workspace_root).ok();
    }

    #[test]
    fn preserves_dirty_stale_running_worktree() {
        let Some((root, worktree, branch)) = temp_repo_with_worktree("dirty") else {
            return;
        };
        fs::write(worktree.join("dirty.txt"), "change\n").unwrap();
        let store = MemoryStore::new(root.join(".peridot/memory.db"));
        let mut record = SessionRecord::new("stale-dirty", &worktree);
        record.status = SessionLifecycle::Running;
        record.created_at_unix = 1;
        record.updated_at_unix = 2;
        record.worktree_branch = Some(branch.clone());
        store.save_session_record(&record).unwrap();

        let report = reconcile_stale_worktrees(&root);

        assert_eq!(report.suspended_sessions, vec!["stale-dirty"]);
        assert_eq!(report.preserved_worktrees.len(), 1);
        assert!(worktree.exists());
        let updated = store.get_session_record("stale-dirty").unwrap().unwrap();
        assert_eq!(updated.status, SessionLifecycle::Suspended);
        assert_eq!(updated.workspace_root, worktree);
        assert_eq!(updated.worktree_branch.as_deref(), Some(branch.as_str()));
        let _ = GitManager::new(&root).remove_worktree(&updated.workspace_root);
        fs::remove_dir_all(root).ok();
    }

    fn temp_repo_with_worktree(label: &str) -> Option<(PathBuf, PathBuf, String)> {
        if Command::new("git").arg("--version").output().is_err() {
            return None;
        }
        let nonce = crate::run_state::unix_timestamp();
        let root = std::env::temp_dir().join(format!(
            "peridot-worktree-cleanup-{label}-{}-{nonce}",
            std::process::id()
        ));
        let worktree = root.join(".peridot/worktrees").join(format!("wt-{label}"));
        fs::create_dir_all(&root).unwrap();
        run_git(&root, ["init"]).unwrap();
        run_git(&root, ["config", "user.email", "peridot@example.com"]).unwrap();
        run_git(&root, ["config", "user.name", "Peridot Test"]).unwrap();
        fs::write(root.join("README.md"), "hello\n").unwrap();
        run_git(&root, ["add", "--all"]).unwrap();
        run_git(&root, ["commit", "-m", "initial"]).unwrap();
        let branch = format!("peridot/test-{label}-{}", std::process::id());
        fs::create_dir_all(worktree.parent().unwrap()).unwrap();
        GitManager::new(&root)
            .add_worktree(&worktree, &branch)
            .unwrap();
        Some((root, worktree, branch))
    }

    fn run_git<const N: usize>(root: &Path, args: [&str; N]) -> peridot_common::PeriResult<String> {
        let output = Command::new("git")
            .args(args)
            .current_dir(root)
            .output()
            .map_err(|err| peridot_common::PeriError::Tool(format!("failed to run git: {err}")))?;
        if !output.status.success() {
            return Err(peridot_common::PeriError::Tool(
                String::from_utf8_lossy(&output.stderr).trim().to_string(),
            ));
        }
        Ok(String::from_utf8_lossy(&output.stdout).to_string())
    }
}
