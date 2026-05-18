use std::fs;
use std::path::Path;
use std::process::Command;

use anyhow::Result;
use peridot_common::{ExecutionMode, MemoryConfig, PeridotConfig, PermissionMode};
use peridot_core::{AgentRunSummary, StopReason};
use peridot_llm::Usage;
use peridot_memory::{MemoryStore, SessionLifecycle, SessionRecord};
use peridot_tui::{HeaderState, TuiState};

use super::checkpoints::restore_latest_checkpoint;
use super::interactive_io::*;
use super::relax_security_for_approval;
use super::run_loop::{normalize_model_service_tier, parse_reviewer_verdict};
use super::run_output::*;
use super::run_state::*;
use super::session_router::SessionRouter;
use super::{
    context_top_report, delete_persisted_session, hydrate_persisted_sessions,
    restore_latest_tui_state_from_disk, restore_tui_state_from_disk,
    scan_and_suspend_running_sessions,
};

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
fn fast_model_alias_enables_service_tier() {
    assert_eq!(
        normalize_model_service_tier("gpt-5.5-fast", &None),
        Some("fast".to_string())
    );
    assert_eq!(
        normalize_model_service_tier("gpt-5.5", &Some("priority".to_string())),
        Some("fast".to_string())
    );
    assert_eq!(
        normalize_model_service_tier("gpt-5.5", &Some("standard".to_string())),
        None
    );
}

#[test]
fn context_top_report_ranks_largest_entries() {
    let entries = vec![
        peridot_context::ContextEntry::trusted(peridot_context::ContextSource::User, "short"),
        peridot_context::ContextEntry::untrusted(
            peridot_context::ContextSource::Tool,
            "x".repeat(400),
        ),
        peridot_context::ContextEntry::trusted(
            peridot_context::ContextSource::Assistant,
            "medium text",
        ),
    ];

    let report = context_top_report(&entries, 123, 272_000, 2);

    assert!(report.contains("context top: 3 entries"));
    assert!(report.contains("status 123 / 272000"));
    assert!(report.contains("tool: 100 tok"));
    assert!(report.contains("1. tool turn 0 · 100 tok untrusted"));
    assert!(!report.contains("3. user"));
}

#[test]
fn restore_latest_checkpoint_restores_previous_content_and_consumes_checkpoint() {
    let root =
        std::env::temp_dir().join(format!("peridot-restore-checkpoint-{}", std::process::id()));
    let checkpoint_dir = root.join(".peridot/checkpoints");
    fs::create_dir_all(&checkpoint_dir).unwrap();
    fs::create_dir_all(root.join("src")).unwrap();
    fs::write(root.join("src/lib.rs"), "new").unwrap();
    let checkpoint_path = checkpoint_dir.join("1-file_patch.json");
    fs::write(
        &checkpoint_path,
        serde_json::to_vec(&serde_json::json!({
            "id": "1-file_patch",
            "tool_name": "file_patch",
            "path": "src/lib.rs",
            "existed": true,
            "previous_content": "old"
        }))
        .unwrap(),
    )
    .unwrap();

    let message = restore_latest_checkpoint(&root).unwrap();

    assert!(message.contains("1-file_patch"));
    assert_eq!(fs::read_to_string(root.join("src/lib.rs")).unwrap(), "old");
    assert!(!checkpoint_path.exists());
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn restore_latest_checkpoint_removes_file_created_by_tool() {
    let root = std::env::temp_dir().join(format!(
        "peridot-restore-new-file-checkpoint-{}",
        std::process::id()
    ));
    let checkpoint_dir = root.join(".peridot/checkpoints");
    fs::create_dir_all(&checkpoint_dir).unwrap();
    fs::create_dir_all(root.join("src")).unwrap();
    fs::write(root.join("src/generated.rs"), "created").unwrap();
    fs::write(
        checkpoint_dir.join("1-file_write.json"),
        serde_json::to_vec(&serde_json::json!({
            "id": "1-file_write",
            "tool_name": "file_write",
            "path": "src/generated.rs",
            "existed": false,
            "previous_content": null
        }))
        .unwrap(),
    )
    .unwrap();

    restore_latest_checkpoint(&root).unwrap();

    assert!(!root.join("src/generated.rs").exists());
    fs::remove_dir_all(root).unwrap();
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
    // Three distinct tool names trip the workflow-breadth branch of the
    // Hermes 4-condition gate, so the run earns an auto-skill.
    let outcome = |name: &str, done: bool| peridot_core::AgentTurnOutcome {
        tool_name: name.to_string(),
        tool_result: peridot_common::ToolResult::success("ok", serde_json::json!({})),
        usage: Usage::default(),
        done,
    };
    let summary = AgentRunSummary {
        turns: vec![
            outcome("file_read", false),
            outcome("file_write", false),
            outcome("verify_test", true),
        ],
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
fn restore_latest_tui_state_uses_most_recent_persisted_session() {
    let root =
        std::env::temp_dir().join(format!("peridot-cli-restore-latest-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(root.join(".peridot")).unwrap();
    let sessions_root = root.join(".peridot").join("sessions");
    let memory = MemoryStore::new(root.join(".peridot/memory.db"));
    for (id, updated_at_unix) in [("older", 100), ("newer", 200)] {
        let mut state = TuiState::new(HeaderState::new(
            ExecutionMode::Execute,
            PermissionMode::Auto,
            "mock",
        ));
        state.current_session_id = id.to_string();
        state.last_task = Some(format!("task {id}"));
        peridot_memory::save_session_blob(
            &sessions_root,
            id,
            "tui_state.json",
            &serde_json::to_vec(&state).unwrap(),
        )
        .unwrap();
        memory
            .save_session_record(&SessionRecord {
                id: id.to_string(),
                summary: format!("summary {id}"),
                status: SessionLifecycle::Suspended,
                created_at_unix: 1,
                updated_at_unix,
                workspace_root: root.clone(),
                worktree_branch: None,
                last_task: Some(format!("task {id}")),
                total_tokens: 0,
                total_cost_usd: 0.0,
                turns_used: 0,
            })
            .unwrap();
    }

    let (id, state) = restore_latest_tui_state_from_disk(&root).unwrap();

    assert_eq!(id, "newer");
    assert_eq!(state.last_task.as_deref(), Some("task newer"));
    fs::remove_dir_all(&root).ok();
}

#[test]
fn hydrate_persisted_sessions_registers_all_unclosed_sessions() {
    let root = std::env::temp_dir().join(format!("peridot-cli-hydrate-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(root.join(".peridot")).unwrap();
    let sessions_root = root.join(".peridot").join("sessions");
    let memory = MemoryStore::new(root.join(".peridot/memory.db"));
    for id in ["s1", "s2"] {
        let mut state = TuiState::new(HeaderState::new(
            ExecutionMode::Execute,
            PermissionMode::Auto,
            "mock",
        ));
        state.current_session_id = id.to_string();
        peridot_memory::save_session_blob(
            &sessions_root,
            id,
            "tui_state.json",
            &serde_json::to_vec(&state).unwrap(),
        )
        .unwrap();
        memory
            .save_session_record(&SessionRecord {
                id: id.to_string(),
                summary: format!("summary {id}"),
                status: SessionLifecycle::Suspended,
                created_at_unix: 1,
                updated_at_unix: 2,
                workspace_root: root.clone(),
                worktree_branch: None,
                last_task: Some(format!("task {id}")),
                total_tokens: 10,
                total_cost_usd: 0.1,
                turns_used: 1,
            })
            .unwrap();
    }
    let mut state = TuiState::new(HeaderState::new(
        ExecutionMode::Execute,
        PermissionMode::Auto,
        "mock",
    ));
    let router = std::sync::Arc::new(std::sync::Mutex::new(SessionRouter::new()));

    hydrate_persisted_sessions(&mut state, &router, &root);

    assert_eq!(state.sessions.len(), 2);
    assert_eq!(router.lock().unwrap().len(), 2);
    assert!(!state.current_session_id.is_empty());
    fs::remove_dir_all(&root).ok();
}

#[test]
fn delete_persisted_session_removes_record_summary_and_blobs() {
    let root =
        std::env::temp_dir().join(format!("peridot-cli-delete-session-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(root.join(".peridot")).unwrap();
    let sessions_root = root.join(".peridot").join("sessions");
    let memory = MemoryStore::new(root.join(".peridot/memory.db"));
    memory
        .save_session_record(&SessionRecord::new("s1", &root))
        .unwrap();
    memory
        .save_session(&peridot_memory::SessionSummary {
            id: "s1".to_string(),
            summary: "saved".to_string(),
        })
        .unwrap();
    peridot_memory::save_session_blob(&sessions_root, "s1", "tui_state.json", b"{}").unwrap();

    delete_persisted_session(&root, "s1");

    assert!(memory.get_session_record("s1").unwrap().is_none());
    assert!(memory.get_session("s1").unwrap().is_none());
    assert!(!sessions_root.join("s1").exists());
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
