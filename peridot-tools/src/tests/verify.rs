use crate::{Tool, ToolContext, VerifyBuildTool};
use peridot_common::{HooksConfig, PermissionMode};
use std::fs;

#[tokio::test]
async fn verify_tool_reports_command_status() {
    let root = std::env::temp_dir().join(format!("peridot-tools-verify-{}", std::process::id()));
    fs::create_dir_all(&root).unwrap();
    let ctx = ToolContext::new(&root, PermissionMode::Auto);
    let result = VerifyBuildTool
        .execute(serde_json::json!({"command":"printf ok"}), &ctx)
        .await
        .unwrap();

    assert_eq!(result.output["success"], true);
    assert_eq!(result.output["stdout"], "ok");
    fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn verify_tool_marks_failed_command_unsuccessful() {
    let root =
        std::env::temp_dir().join(format!("peridot-tools-verify-fail-{}", std::process::id()));
    fs::create_dir_all(&root).unwrap();
    let ctx = ToolContext::new(&root, PermissionMode::Auto);
    let result = VerifyBuildTool
        .execute(serde_json::json!({"command":"exit 7"}), &ctx)
        .await
        .unwrap();

    assert!(!result.success);
    assert_eq!(result.output["status"], 7);
    fs::remove_dir_all(root).unwrap();
}

#[cfg(unix)]
#[tokio::test]
async fn verify_tool_runs_verification_failed_hook() {
    use std::os::unix::fs::PermissionsExt;

    let root =
        std::env::temp_dir().join(format!("peridot-tools-verify-hook-{}", std::process::id()));
    let hooks_dir = root.join(".peridot/hooks");
    fs::create_dir_all(&hooks_dir).unwrap();
    let script = hooks_dir.join("verify.sh");
    fs::write(&script, "#!/bin/sh\necho \"$1:$2\" >> verify.log\n").unwrap();
    fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();
    let ctx = ToolContext::new(&root, PermissionMode::Auto).with_hooks(HooksConfig {
        event: vec![peridot_common::HookConfig {
            event: "verification_failed".to_string(),
            run: ".peridot/hooks/verify.sh {stage} {status}".to_string(),
            description: None,
            on_failure: peridot_common::HookFailureMode::Block,
            only_paths: Vec::new(),
        }],
        ..HooksConfig::default()
    });

    let result = VerifyBuildTool
        .execute(serde_json::json!({"command":"exit 7"}), &ctx)
        .await
        .unwrap();

    assert!(!result.success);
    let log = fs::read_to_string(root.join("verify.log")).unwrap();
    assert!(log.contains("build:failed"));
    fs::remove_dir_all(root).unwrap();
}
