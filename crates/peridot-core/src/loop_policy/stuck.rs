//! Stuck-detector policy.
//!
//! Wraps the existing [`crate::recovery::StuckDetector`] in the policy
//! lifecycle. Records each turn outcome; when the detector requests
//! `Recover`, appends a plan reminder and emits a `Recovery` event but
//! lets the loop continue. When it requests `Abort`, terminates the run
//! with [`StopReason::Interrupted`].

use peridot_common::{AgentPhase, PeriResult};
use peridot_context::{ContextEntry, ContextSource};

use crate::agent::transition_phase;
use crate::loop_policy::{Decision, LoopPolicy, PolicyCx};
use crate::recovery::{StuckAction, StuckDetector, run_recovery_event_hook};
use crate::requests::{AgentRunEvent, AgentTurnOutcome, StopReason};

/// Owns a [`StuckDetector`] and acts on its verdict each turn.
///
/// The repeat-tolerance value matches the original inline default (3
/// consecutive identical outcomes). A future refactor could surface this
/// as a configuration knob; for now the policy preserves existing
/// behaviour exactly.
pub struct StuckDetectorPolicy {
    detector: StuckDetector,
}

impl StuckDetectorPolicy {
    /// Construct with the default repeat tolerance.
    pub fn new() -> Self {
        Self {
            detector: StuckDetector::new(3),
        }
    }

    /// Construct with an explicit repeat tolerance (mostly for tests).
    pub fn with_repeat_tolerance(repeats: usize) -> Self {
        Self {
            detector: StuckDetector::new(repeats),
        }
    }
}

impl Default for StuckDetectorPolicy {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait::async_trait]
impl LoopPolicy for StuckDetectorPolicy {
    fn name(&self) -> &'static str {
        "stuck_detector"
    }

    async fn post_turn(
        &mut self,
        cx: &mut PolicyCx<'_>,
        outcome: &AgentTurnOutcome,
    ) -> PeriResult<Decision> {
        match self.detector.record(outcome) {
            StuckAction::Continue => Ok(Decision::Continue),
            StuckAction::Recover(message) => {
                transition_phase(cx.state, AgentPhase::Recovering, "stuck_recover", cx.events);
                run_recovery_event_hook(cx.project_root, cx.hooks, "stuck", &message)?;
                cx.context
                    .append(ContextEntry::trusted(ContextSource::PlanReminder, message));
                (cx.events)(AgentRunEvent::Recovery {
                    message: "stuck detector requested a new strategy".to_string(),
                });
                Ok(Decision::Continue)
            }
            StuckAction::Abort(message) => {
                // Hard circuit-breaker. The model has ignored the
                // recovery directive for too many turns; stop the run
                // before we burn more tokens. Surface a Recovery event
                // so the TUI's activity panel records what happened —
                // the driver's `Decision::Stop` arm will emit a second
                // `Recovery` carrying the same message, but that's
                // intentional: the in-policy emit preserves event
                // ordering identical to the pre-extraction inline
                // code (Recovery → Finished).
                transition_phase(cx.state, AgentPhase::Recovering, "stuck_abort", cx.events);
                run_recovery_event_hook(cx.project_root, cx.hooks, "stuck_abort", &message)?;
                (cx.events)(AgentRunEvent::Recovery {
                    message: message.clone(),
                });
                Ok(Decision::Stop(StopReason::Interrupted, None))
            }
        }
    }
}
