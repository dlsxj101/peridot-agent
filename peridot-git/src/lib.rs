//! Git automation boundary for Peridot.

use std::path::PathBuf;
use std::process::Command;

use peridot_common::{PeriError, PeriResult};
use serde::{Deserialize, Serialize};

/// Git status summary.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct GitStatus {
    /// Current branch name.
    pub branch: Option<String>,
    /// Changed file paths.
    pub changed_files: Vec<PathBuf>,
}

/// Git manager skeleton.
#[derive(Clone, Debug)]
pub struct GitManager {
    root: PathBuf,
}

impl GitManager {
    /// Creates a git manager rooted at a repository path.
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// Returns the repository root.
    pub fn root(&self) -> &PathBuf {
        &self.root
    }

    /// Returns a compact git status snapshot.
    pub fn status(&self) -> PeriResult<GitStatus> {
        let branch = self.run_git(["branch", "--show-current"]).ok();
        let status = self.run_git(["status", "--short", "--untracked-files=all"])?;
        Ok(GitStatus {
            branch: branch.map(|branch| branch.trim().to_string()),
            changed_files: status
                .lines()
                .filter_map(parse_status_path)
                .map(PathBuf::from)
                .collect(),
        })
    }

    /// Returns whether the root is inside a git work tree.
    pub fn is_repository(&self) -> bool {
        self.run_git(["rev-parse", "--is-inside-work-tree"])
            .map(|value| value.trim() == "true")
            .unwrap_or(false)
    }

    /// Returns the current git diff.
    pub fn diff(&self) -> PeriResult<String> {
        self.run_git(["diff"])
    }

    /// Returns a compact git log.
    pub fn log(&self, limit: usize) -> PeriResult<String> {
        self.run_git(["log", "--oneline", &format!("-{limit}")])
    }

    /// Creates and checks out a branch.
    pub fn create_branch(&self, name: &str) -> PeriResult<String> {
        self.run_git(["switch", "-c", name])
    }

    /// Creates a new git worktree from HEAD on a new branch.
    pub fn add_worktree(&self, path: impl Into<PathBuf>, branch: &str) -> PeriResult<String> {
        let path = path.into();
        let path_string = path.display().to_string();
        self.run_git(["worktree", "add", "-b", branch, &path_string, "HEAD"])
    }

    /// Removes a git worktree.
    pub fn remove_worktree(&self, path: impl Into<PathBuf>) -> PeriResult<String> {
        let path = path.into();
        let path_string = path.display().to_string();
        self.run_git(["worktree", "remove", "--force", &path_string])
    }

    /// Prunes stale worktree metadata.
    pub fn prune_worktrees(&self) -> PeriResult<String> {
        self.run_git(["worktree", "prune"])
    }

    /// Stages all changes and creates a commit.
    pub fn commit_all(&self, message: &str) -> PeriResult<String> {
        self.run_git(["add", "--all"])?;
        self.run_git(["commit", "-m", message])
    }

    fn run_git<const N: usize>(&self, args: [&str; N]) -> PeriResult<String> {
        let output = Command::new("git")
            .args(args)
            .current_dir(&self.root)
            .output()
            .map_err(|err| PeriError::Tool(format!("failed to run git: {err}")))?;
        if !output.status.success() {
            return Err(PeriError::Tool(
                String::from_utf8_lossy(&output.stderr).trim().to_string(),
            ));
        }
        Ok(String::from_utf8_lossy(&output.stdout).to_string())
    }
}

fn parse_status_path(line: &str) -> Option<&str> {
    line.get(3..).map(str::trim).filter(|path| !path.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::Path;

    #[test]
    fn creates_branch_and_commit_in_temp_repo() {
        if Command::new("git").arg("--version").output().is_err() {
            return;
        }
        let root = std::env::temp_dir().join(format!("peridot-git-{}", std::process::id()));
        fs::create_dir_all(&root).unwrap();
        run_raw_git(&root, ["init"]).unwrap();
        run_raw_git(&root, ["config", "user.email", "peridot@example.com"]).unwrap();
        run_raw_git(&root, ["config", "user.name", "Peridot Test"]).unwrap();
        fs::write(root.join("README.md"), "hello\n").unwrap();

        let manager = GitManager::new(&root);
        manager.commit_all("chore: initial").unwrap();
        manager.create_branch("feature/test").unwrap();
        fs::write(root.join("README.md"), "hello again\n").unwrap();
        manager.commit_all("docs: update readme").unwrap();

        let status = manager.status().unwrap();
        let log = manager.log(2).unwrap();

        assert_eq!(status.branch.as_deref(), Some("feature/test"));
        assert!(status.changed_files.is_empty());
        assert!(log.contains("docs: update readme"));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn creates_and_removes_worktree() {
        if Command::new("git").arg("--version").output().is_err() {
            return;
        }
        let root = std::env::temp_dir().join(format!("peridot-git-wt-{}", std::process::id()));
        let worktree =
            std::env::temp_dir().join(format!("peridot-git-wt-child-{}", std::process::id()));
        fs::create_dir_all(&root).unwrap();
        run_raw_git(&root, ["init"]).unwrap();
        run_raw_git(&root, ["config", "user.email", "peridot@example.com"]).unwrap();
        run_raw_git(&root, ["config", "user.name", "Peridot Test"]).unwrap();
        fs::write(root.join("README.md"), "hello\n").unwrap();
        let manager = GitManager::new(&root);
        manager.commit_all("chore: initial").unwrap();

        manager
            .add_worktree(&worktree, "codex/subagent-test")
            .unwrap();

        assert!(worktree.join("README.md").exists());
        manager.remove_worktree(&worktree).unwrap();
        assert!(!worktree.exists());
        fs::remove_dir_all(root).unwrap();
    }

    fn run_raw_git<const N: usize>(root: &PathBuf, args: [&str; N]) -> PeriResult<String> {
        let output = Command::new("git")
            .args(args)
            .current_dir(root)
            .output()
            .map_err(|err| PeriError::Tool(format!("failed to run git: {err}")))?;
        if !output.status.success() {
            return Err(PeriError::Tool(
                String::from_utf8_lossy(&output.stderr).trim().to_string(),
            ));
        }
        Ok(String::from_utf8_lossy(&output.stdout).to_string())
    }

    #[test]
    fn status_includes_untracked_files() {
        if Command::new("git").arg("--version").output().is_err() {
            return;
        }
        let root =
            std::env::temp_dir().join(format!("peridot-git-untracked-{}", std::process::id()));
        fs::create_dir_all(&root).unwrap();
        run_raw_git(&root, ["init"]).unwrap();
        fs::write(root.join("new.txt"), "hello\n").unwrap();

        let status = GitManager::new(&root).status().unwrap();

        assert_eq!(
            status.changed_files,
            vec![Path::new("new.txt").to_path_buf()]
        );
        fs::remove_dir_all(root).unwrap();
    }
}
