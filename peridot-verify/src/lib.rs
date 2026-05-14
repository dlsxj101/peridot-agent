//! Deterministic verification pipeline skeleton.

use peridot_common::PeriError;
use peridot_common::PeriResult;
use peridot_project::ProjectProfile;
use serde::{Deserialize, Serialize};
use std::process::Command;

/// Verification pipeline stage.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VerifyStage {
    /// Deterministic checks.
    Deterministic,
    /// Project build.
    Build,
    /// Project tests.
    Test,
    /// Diff review.
    DiffReview,
    /// LLM grader.
    Grader,
}

/// Result from one verification stage.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct VerifyStageResult {
    /// Stage name.
    pub stage: VerifyStage,
    /// Whether the stage passed.
    pub passed: bool,
    /// Short stage summary.
    pub summary: String,
}

/// Full deterministic verification report.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct VerifyReport {
    /// Ordered stage results.
    pub stages: Vec<VerifyStageResult>,
}

impl VerifyReport {
    /// Returns true when all recorded stages passed.
    pub fn passed(&self) -> bool {
        self.stages.iter().all(|stage| stage.passed)
    }
}

/// Verification pipeline configured for a project.
#[derive(Clone, Debug)]
pub struct VerifyPipeline {
    profile: ProjectProfile,
}

impl VerifyPipeline {
    /// Creates a verification pipeline for a project profile.
    pub fn new(profile: ProjectProfile) -> Self {
        Self { profile }
    }

    /// Returns the profile used by this pipeline.
    pub fn profile(&self) -> &ProjectProfile {
        &self.profile
    }

    /// Runs deterministic stages in spec order.
    pub fn run_all(&self) -> PeriResult<VerifyReport> {
        let mut stages = vec![self.run_deterministic()?];
        if let Some(stage) = self.run_build()? {
            stages.push(stage);
        }
        if let Some(stage) = self.run_test()? {
            stages.push(stage);
        }
        if let Some(stage) = self.run_lint()? {
            stages.push(stage);
        }
        stages.push(self.run_diff_review()?);
        Ok(VerifyReport { stages })
    }

    /// Runs deterministic checks.
    pub fn run_deterministic(&self) -> PeriResult<VerifyStageResult> {
        self.run_command_or_pass(
            VerifyStage::Deterministic,
            "git diff --check",
            "no whitespace conflict markers detected",
        )
    }

    /// Runs the detected build command when one exists.
    pub fn run_build(&self) -> PeriResult<Option<VerifyStageResult>> {
        self.run_optional_command(VerifyStage::Build, self.profile.commands.build.as_deref())
    }

    /// Runs the detected test command when one exists.
    pub fn run_test(&self) -> PeriResult<Option<VerifyStageResult>> {
        self.run_optional_command(VerifyStage::Test, self.profile.commands.test.as_deref())
    }

    /// Runs the detected lint command when one exists.
    pub fn run_lint(&self) -> PeriResult<Option<VerifyStageResult>> {
        self.run_optional_command(
            VerifyStage::Deterministic,
            self.profile.commands.lint.as_deref(),
        )
    }

    /// Runs deterministic diff review.
    pub fn run_diff_review(&self) -> PeriResult<VerifyStageResult> {
        self.run_command_or_pass(
            VerifyStage::DiffReview,
            "git diff --stat",
            "no diff to review",
        )
    }

    fn run_optional_command(
        &self,
        stage: VerifyStage,
        command: Option<&str>,
    ) -> PeriResult<Option<VerifyStageResult>> {
        let Some(command) = command else {
            return Ok(None);
        };
        let output = Command::new("sh")
            .arg("-c")
            .arg(command)
            .current_dir(&self.profile.root)
            .output()
            .map_err(|err| PeriError::Verification {
                stage: format!("{stage:?}"),
                message: format!("failed to run `{command}`: {err}"),
            })?;
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        let summary = if output.status.success() {
            format!("passed `{command}`")
        } else {
            format!("failed `{command}`\n{stdout}{stderr}")
        };
        Ok(Some(VerifyStageResult {
            stage,
            passed: output.status.success(),
            summary,
        }))
    }

    fn run_command_or_pass(
        &self,
        stage: VerifyStage,
        command: &str,
        empty_summary: &str,
    ) -> PeriResult<VerifyStageResult> {
        let output = Command::new("sh")
            .arg("-c")
            .arg(command)
            .current_dir(&self.profile.root)
            .output()
            .map_err(|err| PeriError::Verification {
                stage: format!("{stage:?}"),
                message: format!("failed to run `{command}`: {err}"),
            })?;
        let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        let detail = [stdout, stderr]
            .into_iter()
            .filter(|part| !part.is_empty())
            .collect::<Vec<_>>()
            .join("\n");
        let summary = if detail.is_empty() {
            empty_summary.to_string()
        } else {
            detail
        };
        Ok(VerifyStageResult {
            stage,
            passed: output.status.success(),
            summary,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use peridot_project::ProjectProfile;
    use std::fs;

    #[test]
    fn run_all_records_stages() {
        let root = std::env::temp_dir().join(format!("peridot-verify-{}", std::process::id()));
        fs::create_dir_all(&root).unwrap();
        Command::new("git")
            .arg("init")
            .current_dir(&root)
            .output()
            .unwrap();
        let profile = ProjectProfile::minimal(&root);

        let report = VerifyPipeline::new(profile).run_all().unwrap();

        assert!(report.passed());
        assert!(
            report
                .stages
                .iter()
                .any(|stage| stage.stage == VerifyStage::DiffReview)
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn optional_command_failure_is_recorded() {
        let root = std::env::temp_dir().join(format!("peridot-verify-fail-{}", std::process::id()));
        fs::create_dir_all(&root).unwrap();
        let mut profile = ProjectProfile::minimal(&root);
        profile.commands.build = Some("exit 7".to_string());

        let stage = VerifyPipeline::new(profile).run_build().unwrap().unwrap();

        assert_eq!(stage.stage, VerifyStage::Build);
        assert!(!stage.passed);
        fs::remove_dir_all(root).unwrap();
    }
}
