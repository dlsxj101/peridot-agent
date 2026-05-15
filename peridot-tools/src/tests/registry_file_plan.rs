use crate::tools::plan::PlanFile;
use crate::{
    FilePatchTool, FileReadTool, FileWriteTool, PlanCreateTool, PlanUpdateTool, Tool, ToolContext,
    ToolRegistry,
};
use async_trait::async_trait;
use peridot_common::{
    HooksConfig, PeriResult, PermissionLevel, PermissionMode, ToolGroup, ToolResult,
};
use serde_json::Value;
use std::fs;

struct ReadOnlyTool;

#[async_trait]
impl Tool for ReadOnlyTool {
    fn name(&self) -> &str {
        "read_only"
    }

    fn group(&self) -> ToolGroup {
        ToolGroup::File
    }

    fn description(&self) -> &str {
        "read only fixture"
    }

    async fn execute(&self, _params: Value, _ctx: &ToolContext) -> PeriResult<ToolResult> {
        Ok(ToolResult::success("ok", Value::Null))
    }

    fn permission_level(&self) -> PermissionLevel {
        PermissionLevel::Read
    }
}

#[test]
fn registry_orders_names() {
    let mut registry = ToolRegistry::new();
    registry.register(ReadOnlyTool).unwrap();

    assert_eq!(registry.names(), vec!["read_only"]);
    assert!(
        !registry
            .get("read_only")
            .unwrap()
            .requires_confirmation(PermissionMode::Safe)
    );
}

#[tokio::test]
async fn file_write_and_read_round_trip() {
    let root = std::env::temp_dir().join(format!("peridot-tools-test-{}", std::process::id()));
    fs::create_dir_all(&root).unwrap();
    let ctx = ToolContext::new(&root, PermissionMode::Auto);
    let write = FileWriteTool;
    let read = FileReadTool;

    write
        .execute(
            serde_json::json!({"path":"sample.txt","content":"hello"}),
            &ctx,
        )
        .await
        .unwrap();
    let result = read
        .execute(serde_json::json!({"path":"sample.txt"}), &ctx)
        .await
        .unwrap();

    assert_eq!(result.output, Value::String("hello".to_string()));
    fs::remove_dir_all(root).unwrap();
}

#[cfg(unix)]
#[tokio::test]
async fn file_write_runs_file_changed_hook() {
    use std::os::unix::fs::PermissionsExt;

    let root = std::env::temp_dir().join(format!("peridot-tools-file-hook-{}", std::process::id()));
    let hooks_dir = root.join(".peridot/hooks");
    fs::create_dir_all(&hooks_dir).unwrap();
    fs::create_dir_all(root.join("src")).unwrap();
    let script = hooks_dir.join("file-changed.sh");
    fs::write(&script, "#!/bin/sh\necho \"$1\" >> changed.log\n").unwrap();
    fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();
    let ctx = ToolContext::new(&root, PermissionMode::Auto).with_hooks(HooksConfig {
        event: vec![peridot_common::HookConfig {
            event: "file_changed".to_string(),
            run: ".peridot/hooks/file-changed.sh {path}".to_string(),
            description: None,
            on_failure: peridot_common::HookFailureMode::Block,
            only_paths: vec!["src/**".to_string()],
        }],
        ..HooksConfig::default()
    });

    FileWriteTool
        .execute(
            serde_json::json!({"path":"src/sample.txt","content":"hello"}),
            &ctx,
        )
        .await
        .unwrap();

    let log = fs::read_to_string(root.join("changed.log")).unwrap();
    assert!(log.contains("src/sample.txt"));
    fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn file_patch_replaces_one_segment() {
    let root =
        std::env::temp_dir().join(format!("peridot-tools-patch-test-{}", std::process::id()));
    fs::create_dir_all(&root).unwrap();
    fs::write(root.join("sample.txt"), "hello\nhello\n").unwrap();
    let ctx = ToolContext::new(&root, PermissionMode::Auto);
    FilePatchTool
        .execute(
            serde_json::json!({
                "path": "sample.txt",
                "old_text": "hello",
                "new_text": "goodbye"
            }),
            &ctx,
        )
        .await
        .unwrap();

    assert_eq!(
        fs::read_to_string(root.join("sample.txt")).unwrap(),
        "goodbye\nhello\n"
    );
    fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn plan_create_writes_markdown_and_json() {
    let root =
        std::env::temp_dir().join(format!("peridot-tools-plan-create-{}", std::process::id()));
    fs::create_dir_all(&root).unwrap();
    let ctx = ToolContext::new(&root, PermissionMode::Auto);

    PlanCreateTool
        .execute(
            serde_json::json!({
                "objective": "ship feature",
                "steps": ["write code", {"text": "run tests"}]
            }),
            &ctx,
        )
        .await
        .unwrap();

    let markdown = fs::read_to_string(root.join("todo.md")).unwrap();
    let json = fs::read_to_string(root.join("todo.json")).unwrap();
    let plan = serde_json::from_str::<PlanFile>(&json).unwrap();

    assert!(markdown.contains("Objective: ship feature"));
    assert!(markdown.contains("1. [ ] write code"));
    assert_eq!(plan.steps[1].text, "run tests");
    fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn plan_update_marks_step_and_records_update() {
    let root =
        std::env::temp_dir().join(format!("peridot-tools-plan-update-{}", std::process::id()));
    fs::create_dir_all(&root).unwrap();
    let ctx = ToolContext::new(&root, PermissionMode::Auto);
    PlanCreateTool
        .execute(
            serde_json::json!({
                "objective": "ship feature",
                "steps": ["write code"]
            }),
            &ctx,
        )
        .await
        .unwrap();

    PlanUpdateTool
        .execute(
            serde_json::json!({
                "step": 1,
                "status": "done",
                "update": "code written"
            }),
            &ctx,
        )
        .await
        .unwrap();

    let markdown = fs::read_to_string(root.join("todo.md")).unwrap();
    let json = fs::read_to_string(root.join("todo.json")).unwrap();
    let plan = serde_json::from_str::<PlanFile>(&json).unwrap();

    assert!(markdown.contains("1. [x] write code"));
    assert!(markdown.contains("- code written"));
    assert_eq!(plan.steps[0].status, "done");
    assert_eq!(plan.updates, vec!["code written"]);
    fs::remove_dir_all(root).unwrap();
}
