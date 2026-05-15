use std::path::Path;

use peridot_common::{HooksConfig, PeriError, PeriResult};
use peridot_tools::hooks::{HookRunner, HookVariables};

use crate::requests::AgentTurnOutcome;

pub(crate) fn should_emit_budget_warning(
    budget_usd: f64,
    warning_pct: u8,
    current_usd: f64,
    already_sent: bool,
) -> bool {
    if already_sent || budget_usd <= 0.0 || warning_pct == 0 {
        return false;
    }
    let threshold = budget_usd * (warning_pct.min(100) as f64 / 100.0);
    current_usd >= threshold
}

pub(crate) fn run_budget_warning_hook(
    root: &Path,
    hooks: &HooksConfig,
    current_usd: f64,
    limit_usd: f64,
) -> PeriResult<()> {
    let percentage = if limit_usd > 0.0 {
        (current_usd / limit_usd) * 100.0
    } else {
        0.0
    };
    let mut variables = HookVariables::new();
    variables.insert("project_root".to_string(), root.display().to_string());
    variables.insert("workspace".to_string(), root.display().to_string());
    variables.insert("current".to_string(), format!("{current_usd:.6}"));
    variables.insert("limit".to_string(), format!("{limit_usd:.6}"));
    variables.insert("percentage".to_string(), format!("{percentage:.0}"));
    HookRunner::new(root, hooks.clone()).run_event_hooks("budget_warning", &variables)?;
    Ok(())
}

pub(crate) fn budget_warning_message(current_usd: f64, limit_usd: f64) -> String {
    format!(
        "Budget warning: estimated spend is ${current_usd:.6} against a ${limit_usd:.6} limit. In goal mode, ask the user before taking costly follow-up steps unless the remaining work is clearly cheap and necessary."
    )
}

pub(crate) fn budget_exceeded_message(current_usd: f64, limit_usd: f64) -> String {
    format!(
        "Budget exceeded: estimated spend is ${current_usd:.6} against a ${limit_usd:.6} limit. Pause autonomous work and use agent_ask_user before continuing."
    )
}

pub(crate) fn run_context_compacted_hook(
    root: &Path,
    hooks: &HooksConfig,
    current_tokens: usize,
    limit_tokens: usize,
) -> PeriResult<()> {
    let percentage = if limit_tokens > 0 {
        (current_tokens as f64 / limit_tokens as f64) * 100.0
    } else {
        0.0
    };
    let mut variables = HookVariables::new();
    variables.insert("project_root".to_string(), root.display().to_string());
    variables.insert("workspace".to_string(), root.display().to_string());
    variables.insert("current".to_string(), current_tokens.to_string());
    variables.insert("limit".to_string(), limit_tokens.to_string());
    variables.insert("percentage".to_string(), format!("{percentage:.0}"));
    HookRunner::new(root, hooks.clone()).run_event_hooks("context_compacted", &variables)?;
    Ok(())
}

pub(crate) fn run_error_event_hooks(
    root: &Path,
    hooks: &HooksConfig,
    error: &PeriError,
) -> PeriResult<()> {
    let mut variables = recovery_hook_variables(root, "error", &error.to_string());
    variables.insert("error_type".to_string(), classify_error(error).to_string());
    variables.insert("error_message".to_string(), error.to_string());
    let runner = HookRunner::new(root, hooks.clone());
    runner.run_event_hooks("error", &variables)?;
    runner.run_event_hooks("recovery_triggered", &variables)?;
    Ok(())
}

pub(crate) fn run_recovery_event_hook(
    root: &Path,
    hooks: &HooksConfig,
    recovery_type: &str,
    message: &str,
) -> PeriResult<()> {
    let variables = recovery_hook_variables(root, recovery_type, message);
    HookRunner::new(root, hooks.clone()).run_event_hooks("recovery_triggered", &variables)?;
    Ok(())
}

fn recovery_hook_variables(root: &Path, recovery_type: &str, message: &str) -> HookVariables {
    let mut variables = HookVariables::new();
    variables.insert("project_root".to_string(), root.display().to_string());
    variables.insert("workspace".to_string(), root.display().to_string());
    variables.insert("recovery_type".to_string(), recovery_type.to_string());
    variables.insert("message".to_string(), message.replace(['\r', '\n'], " "));
    variables
}

pub(crate) fn recovery_message(error: &PeriError) -> String {
    format!(
        "Recovery directive: previous turn failed with {}: {error}. Preserve this error in context, avoid repeating the same action, and choose a concrete recovery strategy.",
        classify_error(error)
    )
}

pub(crate) fn format_reminder_message() -> String {
    "Format reminder: respond with a single JSON object like {\"thinking\":\"brief reason\",\"action\":\"tool_name\",\"parameters\":{}}. Do not wrap it in prose unless the JSON object remains recoverable.".to_string()
}

pub(crate) fn classify_error(error: &PeriError) -> &'static str {
    match error {
        PeriError::PermissionDenied(_) | PeriError::PathBoundary(_) => "permission",
        PeriError::Provider(_) => "api_error",
        PeriError::Parse(_) => "parse",
        PeriError::Verification { .. } => "verification",
        PeriError::Config(_) => "config",
        PeriError::Tool(message) => classify_tool_error(message),
    }
}

fn classify_tool_error(message: &str) -> &'static str {
    let lower = message.to_ascii_lowercase();
    if lower.contains("timed out") || lower.contains("timeout") {
        "timeout"
    } else if lower.contains("not found") || lower.contains("no such file") {
        "not_found"
    } else if lower.contains("permission denied") || lower.contains("denied") {
        "permission"
    } else {
        "tool"
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct StuckDetector {
    last_signature: Option<String>,
    repeat_count: usize,
    threshold: usize,
}

impl StuckDetector {
    pub(crate) fn new(threshold: usize) -> Self {
        Self {
            last_signature: None,
            repeat_count: 0,
            threshold,
        }
    }

    pub(crate) fn record(&mut self, outcome: &AgentTurnOutcome) -> Option<String> {
        let signature = format!("{}:{}", outcome.tool_name, outcome.tool_result.summary);
        if self.last_signature.as_deref() == Some(signature.as_str()) {
            self.repeat_count += 1;
        } else {
            self.last_signature = Some(signature);
            self.repeat_count = 1;
        }
        if self.repeat_count < self.threshold {
            return None;
        }
        Some(format!(
            "Recovery directive: the last action repeated {} times with the same result. Re-read the goal, choose a different tool or path, and update the plan before continuing.",
            self.repeat_count
        ))
    }
}
