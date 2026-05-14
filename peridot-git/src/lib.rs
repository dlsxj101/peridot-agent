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
        let branch = self.run_git(["rev-parse", "--abbrev-ref", "HEAD"])?;
        let status = self.run_git(["status", "--short"])?;
        Ok(GitStatus {
            branch: Some(branch.trim().to_string()),
            changed_files: status
                .lines()
                .filter_map(|line| line.get(3..))
                .map(PathBuf::from)
                .collect(),
        })
    }

    /// Returns the current git diff.
    pub fn diff(&self) -> PeriResult<String> {
        self.run_git(["diff"])
    }

    /// Returns a compact git log.
    pub fn log(&self, limit: usize) -> PeriResult<String> {
        self.run_git(["log", "--oneline", &format!("-{limit}")])
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
