//! Deterministic verification pipeline.

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
    /// Project lint / typecheck.
    Lint,
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

    /// Runs the deterministic stages and then asks an LLM grader to weigh in
    /// on the actual change (SPEC Stage 5). The grader only runs when every
    /// deterministic stage passed — a failing build or lint already gives
    /// the operator a verdict and burning an API call to confirm "yes, it's
    /// broken" wastes budget. The resulting grader stage carries the verdict
    /// summary in `VerifyStageResult.summary` and `passed` mirrors the
    /// LLM verdict.
    pub async fn run_all_with_grader<P>(
        &self,
        provider: &P,
        model: &str,
        task: &str,
    ) -> PeriResult<VerifyReport>
    where
        P: peridot_llm::LlmProvider + ?Sized,
    {
        let mut report = self.run_all()?;
        if !report.passed() {
            // Deterministic stages already failed — skip the grader so the
            // operator pays nothing for a duplicated negative verdict.
            return Ok(report);
        }
        let diff = self.collect_diff_for_grader();
        let verify_summary = render_verify_summary_for_grader(&report);
        match peridot_grader::grade_work(provider, model, task, &diff, &verify_summary).await {
            Ok(verdict) => {
                report.stages.push(VerifyStageResult {
                    stage: VerifyStage::Grader,
                    passed: verdict.passed,
                    summary: verdict.summary,
                });
            }
            Err(err) => {
                // Surface grader infrastructure failures as a non-passing
                // Grader stage so the operator can see *why* grading didn't
                // happen — but do not propagate the error, since the
                // deterministic verdict above is still valid.
                report.stages.push(VerifyStageResult {
                    stage: VerifyStage::Grader,
                    passed: false,
                    summary: format!("grader unavailable: {err}"),
                });
            }
        }
        Ok(report)
    }

    /// Captures `git diff HEAD` in the project root for the grader. Best-effort
    /// — when git is missing or the directory is not a repo, returns an empty
    /// string so the grader still sees the verify summary and task text.
    fn collect_diff_for_grader(&self) -> String {
        Command::new("git")
            .args(["diff", "HEAD"])
            .current_dir(&self.profile.root)
            .output()
            .ok()
            .map(|output| String::from_utf8_lossy(&output.stdout).to_string())
            .unwrap_or_default()
    }
}

/// Renders a multi-line "stage: passed/failed — summary" block the grader can
/// consume as its `verify_summary` input. Kept short so the grader call body
/// stays under typical token caps.
fn render_verify_summary_for_grader(report: &VerifyReport) -> String {
    report
        .stages
        .iter()
        .map(|stage| {
            let marker = if stage.passed { "PASS" } else { "FAIL" };
            format!("{marker} {:?}: {}", stage.stage, stage.summary)
        })
        .collect::<Vec<_>>()
        .join("\n")
}

impl VerifyPipeline {
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
        self.run_optional_command(VerifyStage::Lint, self.profile.commands.lint.as_deref())
    }

    /// Runs deterministic diff review.
    pub fn run_diff_review(&self) -> PeriResult<VerifyStageResult> {
        let changed_files = self.changed_files_since_head()?;
        let blocked = changed_files
            .iter()
            .filter(|path| {
                self.profile
                    .boundaries
                    .iter()
                    .any(|boundary| boundary_blocks_path(boundary, path))
            })
            .cloned()
            .collect::<Vec<_>>();
        if !blocked.is_empty() {
            return Ok(VerifyStageResult {
                stage: VerifyStage::DiffReview,
                passed: false,
                summary: format!(
                    "AGENTS boundaries block changed paths: {}",
                    blocked.join(", ")
                ),
            });
        }
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
        if command.starts_with("git ")
            && detail.to_ascii_lowercase().contains("not a git repository")
        {
            return Ok(VerifyStageResult {
                stage,
                passed: true,
                summary: "not a git repository; skipped git check".to_string(),
            });
        }
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

    fn changed_files_since_head(&self) -> PeriResult<Vec<String>> {
        let output = Command::new("git")
            .args(["status", "--short", "--untracked-files=all"])
            .current_dir(&self.profile.root)
            .output()
            .map_err(|err| PeriError::Verification {
                stage: "DiffReview".to_string(),
                message: format!("failed to run `git status --short --untracked-files=all`: {err}"),
            })?;
        let stderr = String::from_utf8_lossy(&output.stderr);
        if !output.status.success() && stderr.contains("not a git repository") {
            return Ok(Vec::new());
        }
        if !output.status.success() {
            return Err(PeriError::Verification {
                stage: "DiffReview".to_string(),
                message: stderr.trim().to_string(),
            });
        }
        Ok(String::from_utf8_lossy(&output.stdout)
            .lines()
            .filter_map(parse_git_status_path)
            .map(str::to_string)
            .collect())
    }
}

fn parse_git_status_path(line: &str) -> Option<&str> {
    line.get(3..).map(str::trim).filter(|path| !path.is_empty())
}

fn boundary_blocks_path(boundary: &str, path: &str) -> bool {
    let boundary = boundary.trim().trim_end_matches('/');
    let path = path.trim();
    !boundary.is_empty() && (path == boundary || path.starts_with(&format!("{boundary}/")))
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

    #[test]
    fn run_lint_stage_uses_lint_variant() {
        let root =
            std::env::temp_dir().join(format!("peridot-verify-lint-pass-{}", std::process::id()));
        fs::create_dir_all(&root).unwrap();
        let mut profile = ProjectProfile::minimal(&root);
        profile.commands.lint = Some("true".to_string());

        let stage = VerifyPipeline::new(profile).run_lint().unwrap().unwrap();

        assert_eq!(stage.stage, VerifyStage::Lint);
        assert!(stage.passed);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn run_lint_failure_reports_lint_stage_not_deterministic() {
        let root =
            std::env::temp_dir().join(format!("peridot-verify-lint-fail-{}", std::process::id()));
        fs::create_dir_all(&root).unwrap();
        let mut profile = ProjectProfile::minimal(&root);
        profile.commands.lint = Some("exit 1".to_string());

        let stage = VerifyPipeline::new(profile).run_lint().unwrap().unwrap();

        assert_eq!(
            stage.stage,
            VerifyStage::Lint,
            "lint failures must be reported under Lint, not Deterministic"
        );
        assert!(!stage.passed);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn git_checks_are_skipped_outside_repositories() {
        let root =
            std::env::temp_dir().join(format!("peridot-verify-non-git-{}", std::process::id()));
        fs::create_dir_all(&root).unwrap();
        let profile = ProjectProfile::minimal(&root);

        let report = VerifyPipeline::new(profile).run_all().unwrap();

        assert!(report.passed());
        assert!(report.stages.iter().all(|stage| stage.passed));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn diff_review_fails_for_agents_boundary_changes() {
        if Command::new("git").arg("--version").output().is_err() {
            return;
        }
        let root =
            std::env::temp_dir().join(format!("peridot-verify-boundary-{}", std::process::id()));
        fs::create_dir_all(root.join("generated")).unwrap();
        run_git(&root, ["init"]).unwrap();
        run_git(&root, ["config", "user.email", "peridot@example.com"]).unwrap();
        run_git(&root, ["config", "user.name", "Peridot Test"]).unwrap();
        fs::write(root.join("README.md"), "hello\n").unwrap();
        run_git(&root, ["add", "--all"]).unwrap();
        run_git(&root, ["commit", "-m", "chore: initial"]).unwrap();
        fs::write(root.join("generated/out.txt"), "blocked\n").unwrap();
        let mut profile = ProjectProfile::minimal(&root);
        profile.boundaries = vec!["generated/".to_string()];

        let stage = VerifyPipeline::new(profile).run_diff_review().unwrap();

        assert_eq!(stage.stage, VerifyStage::DiffReview);
        assert!(!stage.passed);
        assert!(stage.summary.contains("generated/out.txt"));
        fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn run_all_with_grader_appends_grader_stage_when_deterministic_passes() {
        use async_trait::async_trait;
        use peridot_common::PeriResult;
        use peridot_llm::{
            AuthMethod, CompletionRequest, CompletionResponse, LlmProvider, PricingTable, Usage,
        };

        struct PassGrader;
        #[async_trait]
        impl LlmProvider for PassGrader {
            async fn complete(&self, _req: CompletionRequest) -> PeriResult<CompletionResponse> {
                Ok(CompletionResponse {
                    text: r#"{"passed": true, "summary": "looks good", "recommendations": []}"#
                        .to_string(),
                    tool_calls: Vec::new(),
                    reasoning_content: None,
                    usage: Usage::default(),
                })
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
                AuthMethod::NotConfigured
            }
        }

        let root =
            std::env::temp_dir().join(format!("peridot-verify-grader-pass-{}", std::process::id()));
        fs::create_dir_all(&root).unwrap();
        let provider = PassGrader;
        let report = VerifyPipeline::new(ProjectProfile::minimal(&root))
            .run_all_with_grader(&provider, "test-model", "make a button")
            .await
            .unwrap();

        let grader_stage = report
            .stages
            .iter()
            .find(|s| s.stage == VerifyStage::Grader)
            .expect("grader stage must be appended when deterministic stages pass");
        assert!(grader_stage.passed);
        assert!(grader_stage.summary.contains("looks good"));
        fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn run_all_with_grader_skips_grader_when_deterministic_fails() {
        use async_trait::async_trait;
        use peridot_common::{PeriError, PeriResult};
        use peridot_llm::{
            AuthMethod, CompletionRequest, CompletionResponse, LlmProvider, PricingTable,
        };

        // A grader that would PANIC if invoked — proves we never call it
        // when the deterministic stages have already failed.
        struct PanickingGrader;
        #[async_trait]
        impl LlmProvider for PanickingGrader {
            async fn complete(&self, _req: CompletionRequest) -> PeriResult<CompletionResponse> {
                Err(PeriError::Provider(
                    "grader must not run on deterministic failure".to_string(),
                ))
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
                AuthMethod::NotConfigured
            }
        }

        let root =
            std::env::temp_dir().join(format!("peridot-verify-grader-skip-{}", std::process::id()));
        fs::create_dir_all(&root).unwrap();
        let mut profile = ProjectProfile::minimal(&root);
        // Force a deterministic failure via an exiting-nonzero build command.
        profile.commands.build = Some("exit 1".to_string());
        let provider = PanickingGrader;
        let report = VerifyPipeline::new(profile)
            .run_all_with_grader(&provider, "test-model", "noop")
            .await
            .unwrap();

        assert!(
            report.stages.iter().all(|s| s.stage != VerifyStage::Grader),
            "grader must NOT be appended when deterministic stages fail"
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn boundary_matching_respects_path_segments() {
        assert!(boundary_blocks_path("generated/", "generated/out.txt"));
        assert!(boundary_blocks_path("generated", "generated"));
        assert!(!boundary_blocks_path("generated", "generated-old/out.txt"));
    }

    fn run_git<const N: usize>(root: &std::path::Path, args: [&str; N]) -> PeriResult<String> {
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
}
