//! Reusable [`TuiState`] fixtures for tests and headless previews.
//!
//! Each variant of [`TestScenario`] returns a stable `TuiState` that exercises
//! a specific UI surface (welcome screen, transcript with multiple kinds,
//! approval flow, multi-session tab bar, etc.). Tests can match against
//! `render_text_snapshot` output or call `draw()` with a `TestBackend` to
//! verify the rendered cells.

use crate::SessionDirectoryItem;
use crate::state::{
    AgentRunStatus, HeaderState, PlanStep, SubagentMonitorItem, TranscriptKind, TuiState,
};
use peridot_common::{AskUserRequest, ExecutionMode, Locale, PermissionMode, TuiConfig};

/// Stable test scenarios for snapshot / buffer assertions.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TestScenario {
    /// Fresh state with no transcript — Welcome card renders.
    Welcome,
    /// Mid-run with active stream + tool start lines.
    Running,
    /// Tool gated behind an approval panel with params + diff preview.
    Approval,
    /// Ask-user panel awaiting a single-select decision.
    AskUser,
    /// Esc menu open.
    Menu,
    /// Most recent run finished successfully.
    Finished,
    /// Two registered sessions to render the tab bar.
    MultiSessionTabBar,
    /// Korean locale active.
    KoreanLocale,
}

/// Builds the deterministic [`TuiState`] for `scenario`.
pub fn fixture_state(scenario: TestScenario) -> TuiState {
    let mut state = base_state();
    match scenario {
        TestScenario::Welcome => {}
        TestScenario::Running => {
            state.agent_run_status = AgentRunStatus::Running;
            state.push_transcript_entry(TranscriptKind::System, "task: rewrite README");
            state.record_tool_started("shell_exec", serde_json::json!({"command": "ls"}));
            state.begin_stream("assistant");
            state.push_stream_delta("Thinking about the README...");
            state.side_panel.plan.push(PlanStep {
                label: "Audit lib.rs".to_string(),
                done: true,
            });
            state.side_panel.plan.push(PlanStep {
                label: "Patch loop.rs".to_string(),
                done: false,
            });
        }
        TestScenario::Approval => {
            state.apply_runtime_event(crate::state::TuiRuntimeEvent::ApprovalRequested {
                tool_name: "file_patch".to_string(),
                reason: "writes outside workspace".to_string(),
                parameters: serde_json::json!({
                    "path": "src/lib.rs",
                    "old_text": "fn old() {}\n",
                    "new_text": "fn old() { 1 }\n"
                }),
            });
        }
        TestScenario::AskUser => {
            state.open_ask_user(AskUserRequest::SingleSelect {
                question: "Proceed?".to_string(),
                options: vec!["yes".to_string(), "no".to_string()],
                default_index: Some(0),
            });
        }
        TestScenario::Menu => {
            state.menu = Some(crate::ask_user::MenuState::default());
        }
        TestScenario::Finished => {
            state.agent_run_status = AgentRunStatus::Succeeded;
            state.push_transcript_entry(TranscriptKind::System, "task: ship release");
            state.push_transcript_entry(TranscriptKind::Assistant, "done: release notes drafted");
        }
        TestScenario::MultiSessionTabBar => {
            state.sessions = vec![SessionDirectoryItem::new("s1", "main task"), {
                let mut item = SessionDirectoryItem::new("s2", "audit");
                item.pending_attention = true;
                item
            }];
            state.current_session_id = "s1".to_string();
            state.subagents.push(SubagentMonitorItem {
                kind: "fork".to_string(),
                task: "compile checks".to_string(),
                status: "running".to_string(),
                summary: None,
                id: "f1".to_string(),
                parent_id: Some("s1".to_string()),
                depth: 1,
                started_at_unix: 0,
                tokens: 800,
            });
        }
        TestScenario::KoreanLocale => {
            state.config.language = Locale::Ko;
        }
    }
    state
}

fn base_state() -> TuiState {
    let header = HeaderState::new(ExecutionMode::Execute, PermissionMode::Auto, "claude-mock");
    let config = TuiConfig {
        language: Locale::En,
        ..TuiConfig::default()
    };
    TuiState::new(header).with_config(config)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::render::render_text_snapshot;

    #[test]
    fn welcome_fixture_emits_welcome_card() {
        let state = fixture_state(TestScenario::Welcome);
        assert!(render_text_snapshot(&state).contains("Welcome back"));
    }

    #[test]
    fn running_fixture_carries_active_tool_and_plan() {
        let state = fixture_state(TestScenario::Running);
        assert!(!state.active_tools.is_empty());
        assert_eq!(state.side_panel.plan.len(), 2);
        let snapshot = render_text_snapshot(&state);
        assert!(snapshot.contains("banner: Plan"));
    }

    #[test]
    fn approval_fixture_opens_panel_with_params() {
        let state = fixture_state(TestScenario::Approval);
        let panel = state.approval.as_ref().expect("approval panel");
        assert_eq!(panel.tool_name, "file_patch");
        assert!(panel.diff_preview.is_some());
    }

    #[test]
    fn ask_user_fixture_opens_question() {
        let state = fixture_state(TestScenario::AskUser);
        assert!(state.ask_user.is_some());
    }

    #[test]
    fn multi_session_fixture_populates_tab_bar() {
        let state = fixture_state(TestScenario::MultiSessionTabBar);
        assert_eq!(state.sessions.len(), 2);
        let snapshot = render_text_snapshot(&state);
        assert!(snapshot.contains("tabs:"));
        assert!(snapshot.contains("audit!"));
    }

    #[test]
    fn korean_locale_fixture_swaps_status_text() {
        let state = fixture_state(TestScenario::KoreanLocale);
        let snapshot = render_text_snapshot(&state);
        assert!(snapshot.contains("status: 대기 중"));
    }
}
