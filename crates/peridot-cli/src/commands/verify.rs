use super::*;
use crate::providers::live_provider;

pub(crate) async fn run_verify_command(
    project_root: &Path,
    config: &PeridotConfig,
    output: OutputFormat,
    with_grader: bool,
    grader_task: Option<String>,
) -> Result<()> {
    let profile = ProjectScanner::new().scan(project_root)?;
    let pipeline = VerifyPipeline::new(profile);
    let report = if with_grader {
        // The grader needs a task description to evaluate against; an empty
        // one is almost always a mistake (the grader will hallucinate
        // intent from the diff alone). Reject early with a helpful message.
        let task = grader_task.ok_or_else(|| {
            anyhow::anyhow!(
                "--with-grader requires --grader-task <TEXT> so the grader \
                 knows what to evaluate the change against"
            )
        })?;
        let model = config.models.main.clone();
        let provider = live_provider(config, &model, project_root).await?;
        pipeline
            .run_all_with_grader(provider.as_ref(), &model, &task)
            .await?
    } else {
        pipeline.run_all()?
    };
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
        VerifyStage::Lint => "lint",
        VerifyStage::DiffReview => "diff_review",
        VerifyStage::Grader => "grader",
    }
}

pub(super) fn hook_summary_value(summary: &str) -> String {
    summary.replace(['\r', '\n'], " ")
}
