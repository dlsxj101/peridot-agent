use crate::tools::plan::PlanFile;
use crate::{
    FileOutlineTool, FilePatchTool, FileReadTool, FileWriteTool, PlanCreateTool, PlanUpdateTool,
    RipgrepSearchTool, SymbolDefinitionTool, SymbolReferencesTool, SymbolSearchTool, Tool,
    ToolContext, ToolRegistry, WorkspaceSymbolsTool,
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

#[test]
fn builtin_registry_includes_semantic_code_tools() {
    let mut registry = ToolRegistry::new();
    crate::register_builtin_tools(&mut registry).unwrap();

    assert!(registry.get("file_outline").is_some());
    assert!(registry.get("ripgrep_search").is_some());
    assert!(registry.get("symbol_search").is_some());
    assert!(registry.get("symbol_definition").is_some());
    assert!(registry.get("symbol_references").is_some());
    assert!(registry.get("workspace_symbols").is_some());
    assert!(
        !registry
            .get("workspace_symbols")
            .unwrap()
            .requires_confirmation(PermissionMode::Safe)
    );
}

#[tokio::test]
async fn ripgrep_search_finds_workspace_text() {
    let root = std::env::temp_dir().join(format!(
        "peridot-tools-ripgrep-search-{}",
        std::process::id()
    ));
    fs::create_dir_all(&root).unwrap();
    fs::write(
        root.join("main.rs"),
        "fn main() {\n    println!(\"needle\");\n}\n",
    )
    .unwrap();
    let ctx = ToolContext::new(&root, PermissionMode::Auto);

    let result = RipgrepSearchTool
        .execute(
            serde_json::json!({"query": "needle", "path": ".", "max_matches": 5}),
            &ctx,
        )
        .await
        .unwrap();

    assert!(result.success);
    assert_eq!(result.output["query"], "needle");
    assert!(
        result.output["matches"]
            .as_array()
            .map(|matches| !matches.is_empty())
            .unwrap_or(false)
    );
    fs::remove_dir_all(root).unwrap();
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

#[tokio::test]
async fn file_read_decodes_invalid_utf8_lossily() {
    let root =
        std::env::temp_dir().join(format!("peridot-tools-invalid-utf8-{}", std::process::id()));
    fs::create_dir_all(&root).unwrap();
    fs::write(root.join("requirements.txt"), b"fastapi==0.111\n\xff\n").unwrap();
    let ctx = ToolContext::new(&root, PermissionMode::Auto);

    let result = FileReadTool
        .execute(serde_json::json!({"path":"requirements.txt"}), &ctx)
        .await
        .unwrap();

    assert_eq!(
        result.output,
        Value::String("fastapi==0.111\n\u{fffd}\n".to_string())
    );
    assert!(result.summary.contains("invalid UTF-8 bytes replaced"));
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
async fn semantic_code_tools_outline_and_search_workspace_symbols() {
    let root = std::env::temp_dir().join(format!("peridot-tools-symbols-{}", std::process::id()));
    fs::create_dir_all(root.join("src")).unwrap();
    fs::create_dir_all(root.join("node_modules/pkg")).unwrap();
    fs::write(
        root.join("src/lib.rs"),
        "pub struct ProjectScanner;\nimpl ProjectScanner {\n    pub async fn scan(&self) {}\n}\n",
    )
    .unwrap();
    fs::write(
        root.join("src/app.ts"),
        "export function renderApp() {}\nexport class AppShell {}\nconst hidden = 1;\n",
    )
    .unwrap();
    fs::write(
        root.join("node_modules/pkg/index.ts"),
        "export function ignored() {}\n",
    )
    .unwrap();
    let ctx = ToolContext::new(&root, PermissionMode::Auto);

    let outline = FileOutlineTool
        .execute(serde_json::json!({"path":"src/lib.rs"}), &ctx)
        .await
        .unwrap();
    let outline = outline.output.as_array().unwrap();
    assert_eq!(outline[0]["name"], "ProjectScanner");
    assert_eq!(outline[1]["kind"], "impl");
    assert_eq!(outline[2]["name"], "scan");

    let symbols = WorkspaceSymbolsTool
        .execute(serde_json::json!({"path":"src"}), &ctx)
        .await
        .unwrap();
    let symbols = symbols.output.as_array().unwrap();
    assert!(symbols.iter().any(|symbol| symbol["name"] == "renderApp"));
    assert!(!symbols.iter().any(|symbol| symbol["name"] == "ignored"));

    let matches = SymbolSearchTool
        .execute(serde_json::json!({"query":"scanner"}), &ctx)
        .await
        .unwrap();
    let matches = matches.output.as_array().unwrap();
    assert!(matches.len() >= 2);
    assert!(matches.iter().all(|symbol| symbol["path"] == "src/lib.rs"));
    assert!(matches.iter().any(|symbol| symbol["kind"] == "struct"));
    assert!(matches.iter().any(|symbol| symbol["kind"] == "impl"));

    fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn file_outline_parses_typescript_and_python() {
    let root = std::env::temp_dir().join(format!("peridot-tools-multilang-{}", std::process::id()));
    fs::create_dir_all(root.join("src")).unwrap();
    fs::write(
        root.join("src/app.ts"),
        "export class AppShell {\n    start(): void {}\n}\n",
    )
    .unwrap();
    fs::write(
        root.join("src/scan.py"),
        "class Scanner:\n    def scan(self):\n        pass\n",
    )
    .unwrap();
    let ctx = ToolContext::new(&root, PermissionMode::Auto);

    let ts = FileOutlineTool
        .execute(serde_json::json!({"path": "src/app.ts"}), &ctx)
        .await
        .unwrap();
    let ts = ts.output.as_array().unwrap();
    assert!(
        ts.iter()
            .any(|s| s["name"] == "AppShell" && s["kind"] == "class"),
        "{ts:?}"
    );
    assert!(
        ts.iter()
            .any(|s| s["name"] == "start" && s["kind"] == "method" && s["container"] == "AppShell"),
        "{ts:?}"
    );

    let py = FileOutlineTool
        .execute(serde_json::json!({"path": "src/scan.py"}), &ctx)
        .await
        .unwrap();
    let py = py.output.as_array().unwrap();
    assert!(
        py.iter()
            .any(|s| s["name"] == "Scanner" && s["kind"] == "class"),
        "{py:?}"
    );
    assert!(
        py.iter()
            .any(|s| s["name"] == "scan" && s["kind"] == "method"),
        "{py:?}"
    );

    fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn symbol_definition_and_references_locate_rust_symbols() {
    let root = std::env::temp_dir().join(format!("peridot-tools-symdefs-{}", std::process::id()));
    fs::create_dir_all(root.join("src")).unwrap();
    fs::write(
        root.join("src/lib.rs"),
        "pub fn target() {}\nfn caller() {\n    // target in a comment\n    target();\n}\n",
    )
    .unwrap();
    let ctx = ToolContext::new(&root, PermissionMode::Auto);

    let defs = SymbolDefinitionTool
        .execute(serde_json::json!({"name": "target"}), &ctx)
        .await
        .unwrap();
    let defs = defs.output.as_array().unwrap();
    assert_eq!(defs.len(), 1);
    assert_eq!(defs[0]["name"], "target");
    assert_eq!(defs[0]["kind"], "fn");
    assert_eq!(defs[0]["line"], 1);

    let refs = SymbolReferencesTool
        .execute(serde_json::json!({"name": "target"}), &ctx)
        .await
        .unwrap();
    let refs = refs.output.as_array().unwrap();
    // Definition (line 1) and the call (line 4); the comment occurrence on
    // line 3 is excluded by the AST-aware Rust scan.
    assert_eq!(refs.len(), 2, "{refs:?}");
    assert!(refs.iter().all(|r| r["path"] == "src/lib.rs"));
    // The definition (line 1) is tagged; the call (line 4) is a usage.
    assert_eq!(refs[0]["line"], 1);
    assert_eq!(refs[0]["kind"], "definition");
    assert_eq!(refs[1]["line"], 4);
    assert_eq!(refs[1]["kind"], "usage");

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
