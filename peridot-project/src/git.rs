use std::path::Path;
use std::process::Command;

use crate::types::GitState;

pub(crate) fn detect_git_state(root: &Path) -> Option<GitState> {
    if !root.join(".git").exists() {
        return None;
    }
    let branch = Command::new("git")
        .args(["rev-parse", "--abbrev-ref", "HEAD"])
        .current_dir(root)
        .output()
        .ok()
        .filter(|output| output.status.success())
        .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_string());
    let dirty_files = Command::new("git")
        .args(["status", "--short"])
        .current_dir(root)
        .output()
        .ok()
        .filter(|output| output.status.success())
        .map(|output| String::from_utf8_lossy(&output.stdout).lines().count())
        .unwrap_or(0);
    Some(GitState {
        branch,
        dirty_files,
    })
}
