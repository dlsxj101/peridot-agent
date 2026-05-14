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

    /// Runs the currently implemented skeleton checks.
    pub fn run_deterministic(&self) -> PeriResult<VerifyStageResult> {
        Ok(VerifyStageResult {
            stage: VerifyStage::Deterministic,
            passed: true,
            summary: "verification skeleton passed".to_string(),
        })
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
        self.run_optional_command(VerifyStage::Build, self.profile.commands.lint.as_deref())
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
}
