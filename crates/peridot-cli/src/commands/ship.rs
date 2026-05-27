//! `peridot ship` — high-level "publish my work" command.
//!
//! Wraps the per-step `git_*` + `gh_pr_*` tools into one CLI surface so
//! the operator can move from "I have local changes" to "PR is open"
//! in a single call. The model still composes individual tool calls
//! during a normal agent session — this command is the *non-agent*
//! escape hatch: deterministic, scriptable, and gated by clear flags.
//!
//! Flow:
//! 1. Refuse to run when the worktree is clean (nothing to ship).
//! 2. Create / switch to a feature branch (default: `peridot/ship-<unix>`).
//! 3. Stage and commit any pending changes with the provided message.
//! 4. Push to `origin` (set-upstream on first push).
//! 5. Open a PR via `gh pr create` (skipped with `--no-pr`).
//!
//! The push step refuses by default when the active branch is `main`
//! or `master`; `--allow-protected-branch` is the explicit override.

use std::path::Path;
use std::process::Command;

use anyhow::{Context, Result, anyhow};
use peridot_common::PeridotConfig;

use super::{OutputFormat, output::print_json_or_text_result};

#[derive(Clone, Debug, Default)]
pub(crate) struct ShipOptions {
    pub branch: Option<String>,
    pub commit_message: Option<String>,
    pub pr_title: Option<String>,
    pub pr_body: Option<String>,
    pub base: Option<String>,
    pub draft: bool,
    pub no_pr: bool,
    pub allow_protected_branch: bool,
    pub dry_run: bool,
}

pub(crate) async fn run_ship_command(
    project_root: &Path,
    _config: &PeridotConfig,
    options: ShipOptions,
    output: OutputFormat,
) -> Result<()> {
    let mut steps: Vec<serde_json::Value> = Vec::new();

    let dirty_files = collect_dirty_files(project_root)?;
    if dirty_files.is_empty() {
        anyhow::bail!(
            "nothing to ship: the worktree at {} is clean",
            project_root.display()
        );
    }
    steps.push(serde_json::json!({
        "step": "detect_changes",
        "status": "ok",
        "dirty_files": dirty_files,
    }));

    let original_branch = current_branch(project_root)?;
    let target_branch = options
        .branch
        .clone()
        .unwrap_or_else(|| format!("peridot/ship-{}", unix_seconds()));

    if !options.allow_protected_branch
        && matches!(target_branch.as_str(), "main" | "master" | "trunk")
    {
        anyhow::bail!(
            "refusing to ship directly onto protected branch `{target_branch}`; pass --allow-protected-branch to override"
        );
    }

    let commit_message = options
        .commit_message
        .clone()
        .unwrap_or_else(|| format!("ship: {} file(s) via peridot", dirty_files.len()));

    if options.dry_run {
        let exists = git_local_branch_exists(project_root, &target_branch);
        steps.push(serde_json::json!({
            "step": "switch_branch",
            "status": "planned",
            "from": original_branch,
            "to": target_branch,
            "created": !exists,
        }));
        steps.push(serde_json::json!({
            "step": "commit",
            "status": "planned",
            "message": commit_message,
        }));
        steps.push(serde_json::json!({
            "step": "push",
            "status": "planned",
            "branch": target_branch,
        }));
        if options.no_pr {
            steps.push(serde_json::json!({
                "step": "pr",
                "status": "skip",
                "reason": "--no-pr",
            }));
        } else {
            let pr_title = options.pr_title.clone().unwrap_or_else(|| {
                commit_message
                    .lines()
                    .find(|line| !line.trim().is_empty())
                    .unwrap_or(&commit_message)
                    .to_string()
            });
            steps.push(serde_json::json!({
                "step": "pr",
                "status": "planned",
                "title": pr_title,
                "base": options.base,
                "draft": options.draft,
            }));
        }
        print_ship_result(steps, output)?;
        return Ok(());
    }

    if target_branch != original_branch {
        // Switch (or create) the branch. `-c` if the branch doesn't yet
        // exist locally; falling back to plain `switch` if it does.
        let exists = git_local_branch_exists(project_root, &target_branch);
        let switch_args: Vec<&str> = if exists {
            vec!["switch", &target_branch]
        } else {
            vec!["switch", "-c", &target_branch]
        };
        run_git(project_root, &switch_args)
            .with_context(|| format!("git switch to {target_branch} failed"))?;
        steps.push(serde_json::json!({
            "step": "switch_branch",
            "status": "ok",
            "from": original_branch,
            "to": target_branch,
            "created": !exists,
        }));
    } else {
        steps.push(serde_json::json!({
            "step": "switch_branch",
            "status": "skip",
            "reason": "already on target branch",
            "branch": target_branch,
        }));
    }

    run_git(project_root, &["add", "--all"])?;
    let commit_output = Command::new("git")
        .args(["commit", "-m", &commit_message])
        .current_dir(project_root)
        .output()
        .context("git commit failed to start")?;
    if !commit_output.status.success() {
        let stderr = String::from_utf8_lossy(&commit_output.stderr);
        // "nothing to commit" surfaces here when the worktree contained
        // only ignored files; treat as a soft skip rather than a hard
        // bail so `--no-pr` smoke runs keep working.
        if stderr.contains("nothing to commit") {
            steps.push(serde_json::json!({
                "step": "commit",
                "status": "skip",
                "reason": "nothing to commit after staging",
            }));
        } else {
            return Err(anyhow!("git commit failed: {}", stderr.trim()));
        }
    } else {
        steps.push(serde_json::json!({
            "step": "commit",
            "status": "ok",
            "message": commit_message,
        }));
    }

    // Push. `-u origin <branch>` is harmless even when upstream is
    // already configured, and on first push it sets the upstream
    // reference so subsequent runs need no special-case.
    let push_output = Command::new("git")
        .args(["push", "-u", "origin", &target_branch])
        .current_dir(project_root)
        .output()
        .context("git push failed to start")?;
    if !push_output.status.success() {
        return Err(anyhow!(
            "git push origin {target_branch} failed: {}",
            String::from_utf8_lossy(&push_output.stderr).trim()
        ));
    }
    steps.push(serde_json::json!({
        "step": "push",
        "status": "ok",
        "branch": target_branch,
    }));

    if options.no_pr {
        steps.push(serde_json::json!({
            "step": "pr",
            "status": "skip",
            "reason": "--no-pr",
        }));
    } else {
        let pr_title = options.pr_title.clone().unwrap_or_else(|| {
            // First non-empty line of the commit message is a good default title.
            commit_message
                .lines()
                .find(|line| !line.trim().is_empty())
                .unwrap_or(&commit_message)
                .to_string()
        });
        let pr_body = options
            .pr_body
            .clone()
            .unwrap_or_else(|| "Opened by `peridot ship`.".to_string());
        let mut gh_args: Vec<String> = vec![
            "pr".to_string(),
            "create".to_string(),
            "--title".to_string(),
            pr_title.clone(),
            "--body".to_string(),
            pr_body.clone(),
        ];
        if let Some(base) = options.base.as_ref() {
            gh_args.push("--base".to_string());
            gh_args.push(base.clone());
        }
        if options.draft {
            gh_args.push("--draft".to_string());
        }
        let pr_output = Command::new("gh")
            .args(&gh_args)
            .current_dir(project_root)
            .output()
            .context("`gh pr create` failed to start (is the GitHub CLI installed?)")?;
        if !pr_output.status.success() {
            return Err(anyhow!(
                "gh pr create failed: {}",
                String::from_utf8_lossy(&pr_output.stderr).trim()
            ));
        }
        steps.push(serde_json::json!({
            "step": "pr",
            "status": "ok",
            "title": pr_title,
            "url": String::from_utf8_lossy(&pr_output.stdout).trim(),
        }));
    }

    print_ship_result(steps, output)?;
    Ok(())
}

fn print_ship_result(steps: Vec<serde_json::Value>, output: OutputFormat) -> Result<()> {
    let text_lines: Vec<String> = steps
        .iter()
        .map(|step| {
            let label = step["step"].as_str().unwrap_or("?");
            let status = step["status"].as_str().unwrap_or("?");
            format!("{status}\t{label}")
        })
        .collect();
    print_json_or_text_result(
        serde_json::json!({"steps": steps}),
        text_lines.join("\n"),
        output,
    )?;
    Ok(())
}

fn collect_dirty_files(project_root: &Path) -> Result<Vec<String>> {
    let output = Command::new("git")
        .args(["status", "--porcelain", "--untracked-files=all"])
        .current_dir(project_root)
        .output()
        .context("git status failed to start")?;
    if !output.status.success() {
        return Err(anyhow!(
            "git status failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| line.get(3..).map(str::trim).map(str::to_string))
        .filter(|s| !s.is_empty())
        .collect())
}

fn current_branch(project_root: &Path) -> Result<String> {
    let output = Command::new("git")
        .args(["rev-parse", "--abbrev-ref", "HEAD"])
        .current_dir(project_root)
        .output()
        .context("git rev-parse failed to start")?;
    if !output.status.success() {
        return Err(anyhow!(
            "git rev-parse failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn git_local_branch_exists(project_root: &Path, branch: &str) -> bool {
    Command::new("git")
        .args(["show-ref", "--verify", "--quiet"])
        .arg(format!("refs/heads/{branch}"))
        .current_dir(project_root)
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

fn run_git(project_root: &Path, args: &[&str]) -> Result<()> {
    let output = Command::new("git")
        .args(args)
        .current_dir(project_root)
        .output()
        .with_context(|| format!("git {args:?} failed to start"))?;
    if !output.status.success() {
        return Err(anyhow!(
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(())
}

fn unix_seconds() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn dry_run_reports_plan_without_committing_or_switching() {
        let root = temp_git_repo("ship-dry-run");
        run_git(&root, &["config", "user.email", "peridot@example.test"]).unwrap();
        run_git(&root, &["config", "user.name", "Peridot Test"]).unwrap();
        std::fs::write(root.join("README.md"), "initial\n").unwrap();
        run_git(&root, &["add", "--all"]).unwrap();
        run_git(&root, &["commit", "-m", "initial"]).unwrap();
        std::fs::write(root.join("README.md"), "initial\nchanged\n").unwrap();

        run_ship_command(
            &root,
            &PeridotConfig::default(),
            ShipOptions {
                branch: Some("peridot/test-ship".to_string()),
                commit_message: Some("ship: dry run".to_string()),
                dry_run: true,
                ..ShipOptions::default()
            },
            OutputFormat::Json,
        )
        .await
        .unwrap();

        assert_eq!(current_branch(&root).unwrap(), "main");
        let log = Command::new("git")
            .args(["log", "--oneline"])
            .current_dir(&root)
            .output()
            .unwrap();
        let log = String::from_utf8_lossy(&log.stdout);
        assert!(!log.contains("ship: dry run"));
        assert_eq!(collect_dirty_files(&root).unwrap(), vec!["README.md"]);
    }

    fn temp_git_repo(name: &str) -> std::path::PathBuf {
        let root = std::env::temp_dir().join(format!(
            "peridot-{name}-{}-{}",
            std::process::id(),
            unix_seconds()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        run_git(&root, &["init", "-b", "main"]).unwrap();
        root
    }
}
