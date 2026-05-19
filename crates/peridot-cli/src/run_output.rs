use super::*;

pub(super) fn exit_for_summary(summary: &AgentRunSummary, headless: bool) {
    if !headless {
        return;
    }
    match summary.stopped_reason {
        StopReason::Done => {}
        StopReason::ApprovalRequired => std::process::exit(4),
        StopReason::Budget => std::process::exit(2),
        StopReason::MaxTurns => std::process::exit(3),
        StopReason::Interrupted => std::process::exit(130),
    }
}

pub(super) fn run_summary_output(
    summary: &AgentRunSummary,
    mode: ExecutionMode,
) -> serde_json::Value {
    let mut output = serde_json::to_value(summary).unwrap_or_else(|_| serde_json::json!({}));
    if mode == ExecutionMode::Plan
        && summary.stopped_reason == StopReason::Done
        && let Some(object) = output.as_object_mut()
    {
        object.insert(
            "next_actions".to_string(),
            serde_json::Value::Array(
                plan_completion_choices()
                    .into_iter()
                    .map(|choice| choice.to_json())
                    .collect(),
            ),
        );
    }
    output
}

pub(super) fn print_run_summary_text(summary: &AgentRunSummary, mode: ExecutionMode) {
    println!(
        "stopped={:?} turns={} cost=${:.6}",
        summary.stopped_reason,
        summary.turns.len(),
        summary.usage.estimated_cost_usd
    );
    if mode == ExecutionMode::Plan && summary.stopped_reason == StopReason::Done {
        println!("{}", render_plan_completion_choices());
    }
}

pub(super) fn render_plan_completion_choices() -> String {
    plan_completion_choices()
        .into_iter()
        .map(|choice| format!("[{}] {}", choice.id, choice.label))
        .collect::<Vec<_>>()
        .join("\n")
}

pub(super) fn plan_completion_choices() -> Vec<PlanCompletionChoice> {
    vec![
        PlanCompletionChoice::new(
            1,
            "Execute·auto",
            ExecutionMode::Execute,
            PermissionMode::Auto,
        ),
        PlanCompletionChoice::new(
            2,
            "Execute·safe",
            ExecutionMode::Execute,
            PermissionMode::Safe,
        ),
        PlanCompletionChoice::new(3, "Goal·auto", ExecutionMode::Goal, PermissionMode::Auto),
        PlanCompletionChoice::new(4, "Goal·yolo", ExecutionMode::Goal, PermissionMode::Yolo),
        PlanCompletionChoice::new(5, "Revise plan", ExecutionMode::Plan, PermissionMode::Safe),
        PlanCompletionChoice::new(6, "Cancel", ExecutionMode::Plan, PermissionMode::Safe),
    ]
}

#[derive(Clone, Debug)]
pub(super) struct PlanCompletionChoice {
    id: u8,
    label: &'static str,
    mode: ExecutionMode,
    permission: PermissionMode,
}

impl PlanCompletionChoice {
    fn new(id: u8, label: &'static str, mode: ExecutionMode, permission: PermissionMode) -> Self {
        Self {
            id,
            label,
            mode,
            permission,
        }
    }

    fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "id": self.id,
            "label": self.label,
            "mode": self.mode,
            "permission": self.permission
        })
    }
}
