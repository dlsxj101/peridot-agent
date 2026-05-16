use std::fs;
use std::path::Path;
use std::process::Command;

use anyhow::Result;
use peridot_common::{ExecutionMode, MemoryConfig, PeridotConfig, PermissionMode};
use peridot_core::{AgentRunSummary, StopReason};
use peridot_llm::Usage;
use peridot_memory::{MemoryStore, SessionLifecycle, SessionRecord};
use peridot_tui::{HeaderState, TuiState};

use super::interactive_io::*;
use super::relax_security_for_approval;
use super::run_loop::parse_reviewer_verdict;
use super::run_output::*;
use super::run_state::*;
use super::{restore_tui_state_from_disk, scan_and_suspend_running_sessions};

#[test]
fn parse_reviewer_verdict_handles_each_outcome() {
    use peridot_core::ReviewerVerdict;
    assert_eq!(
        parse_reviewer_verdict(r#"{"verdict":"approve","comments":""}"#),
        Some(ReviewerVerdict::Approve),
    );
    assert_eq!(
        parse_reviewer_verdict(r#"{"verdict":"request_changes","comments":"indent"}"#),
        Some(ReviewerVerdict::RequestChanges {
            comments: "indent".to_string(),
        }),
    );
    assert_eq!(
        parse_reviewer_verdict(r#"{"verdict":"block","comments":"writes outside workspace"}"#),
        Some(ReviewerVerdict::Block {
            reason: "writes outside workspace".to_string(),
        }),
    );
    assert!(parse_reviewer_verdict("not json at all").is_none());
    assert!(parse_reviewer_verdict(r#"{"unrelated":1}"#).is_none());
}

#[test]
fn parse_reviewer_verdict_strips_json_code_fence() {
    use peridot_core::ReviewerVerdict;
    let raw = "```json\n{\"verdict\":\"approve\",\"comments\":\"ok\"}\n```";
    assert_eq!(parse_reviewer_verdict(raw), Some(ReviewerVerdict::Approve));
    let raw_bare_fence = "```\n{\"verdict\":\"approve\",\"comments\":\"\"}\n```";
    assert_eq!(
        parse_reviewer_verdict(raw_bare_fence),
        Some(ReviewerVerdict::Approve),
    );
}

#[test]
fn resume_text_wraps_current_task() {
    let text = resume_task_text("demo", "created parser", "finish tests");

    assert!(text.contains("Resume session demo"));
    assert!(text.contains("created parser"));
    assert!(text.contains("Current task: finish tests"));
}

#[test]
fn resume_text_handles_empty_task() {
    let text = resume_task_text("demo", "created parser", "");

    assert_eq!(
        text,
        "Resume session demo from this summary: created parser"
    );
}

#[test]
fn approval_relaxes_matching_security_gate_only() {
    let mut config = PeridotConfig::default();

    relax_security_for_approval(
        &mut config,
        "dependency installation requires explicit user approval",
    );

    assert!(!config.security.ask_before_install);
    assert!(config.security.ask_before_delete);

    relax_security_for_approval(
        &mut config,
        "destructive shell command requires explicit user approval",
    );

    assert!(!config.security.ask_before_delete);
}

#[cfg(unix)]
#[test]
fn tui_lifecycle_hooks_run_for_switches() {
    use peridot_common::{HookConfig, HookFailureMode, HooksConfig};
    use std::os::unix::fs::PermissionsExt;

    let root = std::env::temp_dir().join(format!("peridot-cli-tui-hooks-{}", std::process::id()));
    let hooks_dir = root.join(".peridot/hooks");
    std::fs::create_dir_all(&hooks_dir).unwrap();
    let script = hooks_dir.join("switch.sh");
    std::fs::write(&script, "#!/bin/sh\necho \"$1:$2:$3\" >> switches.log\n").unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    let mut state = TuiState::new(HeaderState::new(
        ExecutionMode::Execute,
        PermissionMode::Auto,
        "mock",
    ));
    state.lifecycle_events.push(peridot_tui::TuiLifecycleEvent {
        event: "mode_switch".to_string(),
        from: "execute".to_string(),
        to: "goal".to_string(),
    });
    state.lifecycle_events.push(peridot_tui::TuiLifecycleEvent {
        event: "permission_switch".to_string(),
        from: "auto".to_string(),
        to: "safe".to_string(),
    });
    let config = PeridotConfig {
        hooks: HooksConfig {
            lifecycle: vec![
                HookConfig {
                    event: "mode_switch".to_string(),
                    run: ".peridot/hooks/switch.sh mode {from_mode} {to_mode}".to_string(),
                    description: None,
                    on_failure: HookFailureMode::Block,
                    only_paths: Vec::new(),
                },
                HookConfig {
                    event: "permission_switch".to_string(),
                    run: ".peridot/hooks/switch.sh permission {from_permission} {to_permission}"
                        .to_string(),
                    description: None,
                    on_failure: HookFailureMode::Block,
                    only_paths: Vec::new(),
                },
            ],
            ..HooksConfig::default()
        },
        ..PeridotConfig::default()
    };

    run_tui_lifecycle_hooks(&state, &config, &root).unwrap();

    let log = std::fs::read_to_string(root.join("switches.log")).unwrap();
    assert!(log.contains("mode:execute:goal"));
    assert!(log.contains("permission:auto:safe"));
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn saves_run_summary_for_resume() {
    let root = std::env::temp_dir().join(format!("peridot-cli-run-save-{}", std::process::id()));
    let summary = AgentRunSummary {
        turns: Vec::new(),
        usage: Usage::default(),
        stopped_reason: StopReason::Done,
    duration_ms: 0,
    };

    save_run_session(
        &root,
        "session-test",
        &summary,
        "finish the parser",
        &MemoryConfig {
            auto_skills: false,
            ..MemoryConfig::default()
        },
    )
    .unwrap();

    let session = MemoryStore::new(root.join(".peridot/memory.db"))
        .get_session("session-test")
        .unwrap()
        .unwrap();
    assert!(session.summary.contains("finish the parser"));
    assert!(session.summary.contains("stopped=Done"));
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn completed_run_saves_auto_skill_when_enabled() {
    let root = std::env::temp_dir().join(format!("peridot-cli-auto-skill-{}", std::process::id()));
    let summary = AgentRunSummary {
        turns: vec![peridot_core::AgentTurnOutcome {
            tool_name: "verify_test".to_string(),
            tool_result: peridot_common::ToolResult::success("tests passed", serde_json::json!({})),
            usage: Usage::default(),
            done: true,
        }],
        usage: Usage::default(),
        stopped_reason: StopReason::Done,
    duration_ms: 0,
    };

    save_run_session(
        &root,
        "session-auto",
        &summary,
        "fix parser tests",
        &MemoryConfig::default(),
    )
    .unwrap();

    let skill = MemoryStore::new(root.join(".peridot/memory.db"))
        .search_skills("parser")
        .unwrap()
        .pop()
        .unwrap();
    assert_eq!(skill.name, "auto-fix-parser-tests");
    assert!(
        root.join(".peridot/skills/auto/auto-fix-parser-tests.md")
            .exists()
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn compact_summary_text_limits_long_tasks() {
    let compact = compact_summary_text("a b c d e f", 5);

    assert_eq!(compact, "a ...");
}

#[test]
fn plan_summary_output_includes_execution_choices() {
    let summary = AgentRunSummary {
        turns: Vec::new(),
        usage: Usage::default(),
        stopped_reason: StopReason::Done,
    duration_ms: 0,
    };

    let output = run_summary_output(&summary, ExecutionMode::Plan);

    assert_eq!(output["next_actions"][0]["label"], "Execute·auto");
    assert_eq!(output["next_actions"][3]["permission"], "yolo");
    assert!(render_plan_completion_choices().contains("[6] Cancel"));
}

#[test]
fn commit_message_uses_conventional_style() {
    assert_eq!(
        commit_message_for_task("fix the parser", "conventional"),
        "chore(agent): fix the parser"
    );
    assert_eq!(slugify_for_branch("Fix the parser!"), "fix-the-parser");
}

#[test]
fn auto_commit_run_commits_dirty_worktree() {
    if Command::new("git").arg("--version").output().is_err() {
        return;
    }
    let root = std::env::temp_dir().join(format!("peridot-cli-auto-commit-{}", std::process::id()));
    fs::create_dir_all(&root).unwrap();
    run_git(&root, ["init"]).unwrap();
    run_git(&root, ["config", "user.email", "peridot@example.com"]).unwrap();
    run_git(&root, ["config", "user.name", "Peridot Test"]).unwrap();
    fs::write(root.join("README.md"), "hello\n").unwrap();
    run_git(&root, ["add", "--all"]).unwrap();
    run_git(&root, ["commit", "-m", "chore: initial"]).unwrap();
    fs::write(root.join("result.txt"), "done\n").unwrap();
    let summary = AgentRunSummary {
        turns: Vec::new(),
        usage: Usage::default(),
        stopped_reason: StopReason::Done,
    duration_ms: 0,
    };
    let config = PeridotConfig {
        git: peridot_common::GitConfig {
            auto_commit: true,
            auto_branch: true,
            branch_prefix: "peridot/".to_string(),
            ..peridot_common::GitConfig::default()
        },
        ..PeridotConfig::default()
    };

    let message = auto_commit_run(&root, &config, &summary, "write result file")
        .unwrap()
        .unwrap();
    let status = run_git(&root, ["status", "--short"]).unwrap();
    let branch = run_git(&root, ["rev-parse", "--abbrev-ref", "HEAD"]).unwrap();

    assert_eq!(message, "chore(agent): write result file");
    assert!(status.trim().is_empty());
    assert!(branch.trim().starts_with("peridot/write-result-file-"));
    fs::remove_dir_all(root).unwrap();
}

fn run_git<const N: usize>(root: &Path, args: [&str; N]) -> Result<String> {
    let output = Command::new("git").args(args).current_dir(root).output()?;
    if !output.status.success() {
        anyhow::bail!("{}", String::from_utf8_lossy(&output.stderr).trim());
    }
    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

#[test]
fn restore_returns_serde_roundtrip_of_persisted_state() {
    let root = std::env::temp_dir().join(format!("peridot-cli-restore-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).unwrap();
    let id = "test-session";
    let mut state = TuiState::new(HeaderState::new(
        ExecutionMode::Execute,
        PermissionMode::Auto,
        "mock",
    ));
    state.current_session_id = id.to_string();
    state.last_task = Some("rewrite README".to_string());

    let sessions_root = root.join(".peridot").join("sessions");
    let bytes = serde_json::to_vec(&state).unwrap();
    peridot_memory::save_session_blob(&sessions_root, id, "tui_state.json", &bytes).unwrap();

    let (restored_id, restored) = restore_tui_state_from_disk(id, &root).unwrap();
    assert_eq!(restored_id, id);
    assert_eq!(restored.last_task.as_deref(), Some("rewrite README"));
    assert_eq!(restored.current_session_id, id);

    fs::remove_dir_all(&root).ok();
}

#[test]
fn startup_scan_downgrades_running_sessions_to_suspended() {
    let root = std::env::temp_dir().join(format!("peridot-cli-scan-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(root.join(".peridot")).unwrap();
    let memory = MemoryStore::new(root.join(".peridot/memory.db"));
    let record = SessionRecord {
        id: "s-running".to_string(),
        summary: "running session".to_string(),
        status: SessionLifecycle::Running,
        created_at_unix: 100,
        updated_at_unix: 200,
        workspace_root: root.clone(),
        worktree_branch: None,
        last_task: None,
        total_tokens: 0,
        total_cost_usd: 0.0,
        turns_used: 0,
    };
    memory.save_session_record(&record).unwrap();

    let suspended = scan_and_suspend_running_sessions(&root);
    assert_eq!(suspended, vec!["s-running".to_string()]);

    let after = memory.get_session_record("s-running").unwrap().unwrap();
    assert_eq!(after.status, SessionLifecycle::Suspended);

    fs::remove_dir_all(&root).ok();
}
