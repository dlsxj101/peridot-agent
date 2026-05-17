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
    let classification = classify_error(error);
    // Verification failures get a stronger, narrower directive — the model
    // tends to drift back to feature work after a single nudge, but
    // failing tests need an explicit "fix this first" anchor. Other
    // failure classes rotate through a small bank of phrasings so the
    // model doesn't memorise one wording and overfit on subsequent
    // recovery turns ("Structured Variation" from spec section 7.3).
    if classification == "verification" {
        return format!(
            "Verification failed ({classification}): {error}. STOP all new work. Read the failing output above, find the smallest change that restores the failing check, and re-run the same verify tool to confirm. Do not add new features until the verifier passes."
        );
    }
    let templates: [&str; 5] = [
        "Recovery directive: previous turn failed with {kind}: {error}. Preserve this error in context, avoid repeating the same action, and choose a concrete recovery strategy.",
        "Turn failed ({kind}: {error}). The same approach will not work twice. Read the prior failure, switch tactics, and explain the new strategy before re-attempting the tool.",
        "Last attempt errored as {kind}: {error}. Do not repeat the call. Identify the root cause (typo, wrong path, missing arg) and address that before invoking any tool again.",
        "Recovery: {kind} error ({error}) on the previous turn. The conversation history already shows this failure. Branch to a different tool, narrower argument, or a clarifying read before another write.",
        "Caught {kind} failure: {error}. Treat the previous tool call as discarded. Restate your current goal in one sentence, then pick a new tool call that targets that goal differently.",
    ];
    let pick = recovery_template_index(error) % templates.len();
    templates[pick]
        .replace("{kind}", classification)
        .replace("{error}", &error.to_string())
}

/// Deterministically picks a recovery-message template index from the
/// error's text hash so identical failures keep the same phrasing within a
/// run (stuck-detector signatures stay stable) while different failures
/// rotate through different wordings.
fn recovery_template_index(error: &PeriError) -> usize {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    error.to_string().hash(&mut hasher);
    hasher.finish() as usize
}

pub(crate) fn format_reminder_message() -> String {
    "Format reminder: call one of the provided tools using the native tool-calling protocol, or reply with a plain text message when no tool is needed. Do not emit raw JSON action envelopes — the harness no longer parses them.".to_string()
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

/// Outcome of a [`StuckDetector::record`] call.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum StuckAction {
    /// Nothing to do; the agent is making progress.
    Continue,
    /// The same action has repeated `threshold` times; inject the recovery
    /// directive so the model gets a hint to try a different approach.
    Recover(String),
    /// The same action has repeated long enough that we no longer trust the
    /// model to recover on its own. The agent loop should abort the run with
    /// the supplied reason so we stop burning tokens.
    Abort(String),
}

impl StuckDetector {
    pub(crate) fn new(threshold: usize) -> Self {
        Self {
            last_signature: None,
            repeat_count: 0,
            threshold,
        }
    }

    pub(crate) fn record(&mut self, outcome: &AgentTurnOutcome) -> StuckAction {
        let signature = format!("{}:{}", outcome.tool_name, outcome.tool_result.summary);
        if self.last_signature.as_deref() == Some(signature.as_str()) {
            self.repeat_count += 1;
        } else {
            self.last_signature = Some(signature);
            self.repeat_count = 1;
        }
        if self.repeat_count < self.threshold {
            return StuckAction::Continue;
        }
        // Soft signal at `threshold`, hard stop at `2 * threshold`. Without the
        // hard stop a model that ignores the recovery directive can keep
        // repeating itself for dozens of turns, racking up cost. Two times the
        // threshold gives the directive a fair chance to land before we pull
        // the plug.
        if self.repeat_count >= self.threshold * 2 {
            return StuckAction::Abort(format!(
                "Stuck-detector circuit breaker: tool `{}` repeated {} times with identical result. Aborting the run so the model can be retried or guided by the user.",
                self.last_signature
                    .as_deref()
                    .and_then(|sig| sig.split_once(':').map(|(name, _)| name))
                    .unwrap_or("(unknown)"),
                self.repeat_count
            ));
        }
        StuckAction::Recover(format!(
            "Recovery directive: the last action repeated {} times with the same result. The conversation history above already contains that tool call and its `tool` role result paired by `tool_call_id` — read that prior result instead of calling the same tool again. Choose a different tool, change the arguments, or finish with `agent_done` if the answer is already in the prior result.",
            self.repeat_count
        ))
    }
}
