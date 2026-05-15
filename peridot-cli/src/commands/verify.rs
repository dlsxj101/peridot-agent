use super::*;

pub(crate) fn run_verify_command(
    project_root: &Path,
    config: &PeridotConfig,
    output: OutputFormat,
) -> Result<()> {
    let profile = ProjectScanner::new().scan(project_root)?;
    let report = VerifyPipeline::new(profile).run_all()?;
    run_verification_event_hooks(project_root, &config.hooks, &report)?;
    match output {
        OutputFormat::Json => println!("{}", serde_json::to_string_pretty(&report)?),
        OutputFormat::Text => {
            for stage in &report.stages {
                let marker = if stage.passed { "PASS" } else { "FAIL" };
                println!("{marker}\t{:?}\t{}", stage.stage, stage.summary);
            }
        }
    }
    Ok(())
}

pub(super) fn run_verification_event_hooks(
    project_root: &Path,
    hooks: &HooksConfig,
    report: &VerifyReport,
) -> Result<()> {
    let selected = verification_hook_stage(report);
    let event = if report.passed() {
        "verification_passed"
    } else {
        "verification_failed"
    };
    let mut variables = HookVariables::new();
    variables.insert(
        "project_root".to_string(),
        project_root.display().to_string(),
    );
    variables.insert("workspace".to_string(), project_root.display().to_string());
    variables.insert(
        "stage".to_string(),
        verify_stage_name(&selected.stage).to_string(),
    );
    variables.insert(
        "status".to_string(),
        if report.passed() { "passed" } else { "failed" }.to_string(),
    );
    variables.insert("output".to_string(), hook_summary_value(&selected.summary));
    HookRunner::new(project_root, hooks.clone()).run_event_hooks(event, &variables)?;
    Ok(())
}

pub(super) fn verification_hook_stage(report: &VerifyReport) -> &VerifyStageResult {
    report
        .stages
        .iter()
        .find(|stage| !stage.passed)
        .or_else(|| report.stages.last())
        .expect("verification reports always include at least one stage")
}

pub(super) fn verify_stage_name(stage: &VerifyStage) -> &'static str {
    match stage {
        VerifyStage::Deterministic => "deterministic",
        VerifyStage::Build => "build",
        VerifyStage::Test => "test",
        VerifyStage::DiffReview => "diff_review",
        VerifyStage::Grader => "grader",
    }
}

pub(super) fn hook_summary_value(summary: &str) -> String {
    summary.replace(['\r', '\n'], " ")
}
