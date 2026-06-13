use super::*;
// Handlers and helpers that moved into domain submodules but are still
// exercised directly by these tests.
use super::approval::*;
use super::mcp::*;

fn test_options(mock_response_file: Option<PathBuf>) -> AgentTaskOptions {
    AgentTaskOptions {
        permission: PermissionMode::Auto,
        model: "mock".to_string(),
        reasoning_effort: peridot_common::ReasoningEffort::Off,
        service_tier: None,
        max_turns: 2,
        budget_usd: 1.0,
        resume: None,
        mock_response_file,
        live: false,
    }
}

fn test_project(name: &str) -> PathBuf {
    let unique = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let root = std::env::temp_dir().join(format!(
        "peridot-daemon-test-{name}-{}-{unique}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    root
}

async fn dispatch_and_collect(line: &str) -> Vec<Value> {
    dispatch_and_collect_with_options(line, test_options(None)).await
}

async fn dispatch_and_collect_with_options(line: &str, options: AgentTaskOptions) -> Vec<Value> {
    let (tx, mut rx) = mpsc::unbounded_channel::<String>();
    let root = test_project("dispatch");
    let state = DaemonState::new(root.clone(), PeridotConfig::default(), options, tx);
    let _ = dispatch_line(&state, line).await.unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    let mut values = Vec::new();
    while let Ok(line) = rx.try_recv() {
        values.push(serde_json::from_str(&line).unwrap());
    }
    shutdown_sessions(&state).await;
    let _ = std::fs::remove_dir_all(root);
    values
}

#[tokio::test]
async fn version_method_returns_cargo_pkg_version() {
    let out = dispatch_and_collect(r#"{"jsonrpc":"2.0","id":1,"method":"peridot.version"}"#).await;
    assert_eq!(out[0]["jsonrpc"], "2.0");
    assert_eq!(out[0]["id"], 1);
    assert_eq!(out[0]["result"]["version"], env!("CARGO_PKG_VERSION"));
}

#[tokio::test]
async fn status_method_returns_project_context() {
    let out = dispatch_and_collect(r#"{"jsonrpc":"2.0","id":9,"method":"peridot.status"}"#).await;
    assert_eq!(out[0]["jsonrpc"], "2.0");
    assert_eq!(out[0]["id"], 9);
    assert_eq!(out[0]["result"]["version"], env!("CARGO_PKG_VERSION"));
    assert_eq!(out[0]["result"]["provider"], "claude-api");
    assert_eq!(out[0]["result"]["model"], "claude-sonnet-4-6");
    assert_eq!(out[0]["result"]["committee_mode"], "off");
    assert!(
        out[0]["result"]["model_suggestions"]
            .as_array()
            .is_some_and(|models| models.iter().any(|model| model == "claude-sonnet-4-6"))
    );
    assert!(out[0]["result"]["branch_snapshots"].as_array().is_some());
    assert!(out[0]["result"]["project_root"].as_str().is_some());
    assert_eq!(out[0]["result"]["auth"]["provider"], "claude-api");
    assert_eq!(out[0]["result"]["auth"]["method"], "api_key");
    assert!(out[0]["result"]["mcp"].as_array().is_some());
    assert!(out[0]["result"]["code_map"].is_object());
    assert!(out[0]["result"]["code_map"]["index_exists"].is_boolean());
    assert!(out[0]["result"]["code_map"]["stale"].is_boolean());
    assert!(out[0]["result"]["worktree_cleanup"].is_object());
}

#[tokio::test]
async fn mcp_add_and_remove_results_include_refreshed_inventory_rows() {
    let root = test_project("mcp-inventory-result");
    std::fs::create_dir_all(root.join(".peridot")).unwrap();
    std::fs::write(
        root.join(".peridot/config.toml"),
        r#"
[[mcp]]
name = "github"
transport = "http"
url = "https://example.com/mcp"
"#,
    )
    .unwrap();
    let (tx, _rx) = mpsc::unbounded_channel::<String>();
    let state = DaemonState::new(
        root.clone(),
        PeridotConfig::default(),
        test_options(None),
        tx,
    );

    let added = handle_command_mcp_add(
        &state,
        "/mcp add local stdio node server.js",
        "local",
        "stdio",
        "node server.js",
    )
    .unwrap();
    let added_items = added["items"].as_array().unwrap();
    assert_eq!(added_items.len(), 2);
    assert!(added_items.iter().any(|item| item["label"] == "github"));
    assert!(added_items.iter().any(|item| item["label"] == "local"));

    let removed = handle_command_mcp_remove(&state, "/mcp remove github", "github").unwrap();
    let removed_items = removed["items"].as_array().unwrap();
    assert_eq!(removed_items.len(), 1);
    assert_eq!(removed_items[0]["label"], "local");
    shutdown_sessions(&state).await;
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn mcp_probe_result_items_include_connectivity_metadata() {
    let root = test_project("mcp-probe-result");
    std::fs::create_dir_all(root.join(".peridot")).unwrap();
    let path = root.join(".peridot/config.toml");
    std::fs::write(
        &path,
        r#"
[[mcp]]
name = "github"
transport = "http"
url = "https://example.com/mcp"
"#,
    )
    .unwrap();
    let config = read_project_config(&path).unwrap();

    let items = mcp_command_items_with_probe(&config, Some(("github", true, Some(4))));

    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["label"], "github");
    assert_eq!(items[0]["tool_count"], 4);
    assert_eq!(items[0]["connected"], true);
    let _ = std::fs::remove_dir_all(root);
}

#[tokio::test]
async fn status_reconciles_stale_worktree_records() {
    let root = test_project("status-worktree-cleanup");
    let store = MemoryStore::new(root.join(".peridot/memory.db"));
    let mut record = SessionRecord::new("stale-worktree", root.join(".peridot/worktrees/wt"));
    record.status = SessionLifecycle::Running;
    record.worktree_branch = Some("peridot/stale-worktree".to_string());
    store.save_session_record(&record).unwrap();
    let (tx, mut rx) = mpsc::unbounded_channel::<String>();
    let state = DaemonState::new(
        root.clone(),
        PeridotConfig::default(),
        test_options(None),
        tx,
    );

    dispatch_line(
        &state,
        r#"{"jsonrpc":"2.0","id":91,"method":"peridot.status"}"#,
    )
    .await
    .unwrap();

    let line = rx.try_recv().unwrap();
    let value: Value = serde_json::from_str(&line).unwrap();
    let cleanup = &value["result"]["worktree_cleanup"];
    assert_eq!(cleanup["suspended_sessions"][0], "stale-worktree");
    assert_eq!(
        cleanup["missing_worktrees"][0]["session_id"],
        "stale-worktree"
    );
    let updated = store.get_session_record("stale-worktree").unwrap().unwrap();
    assert_eq!(updated.status, SessionLifecycle::Suspended);
    assert_eq!(updated.worktree_branch, None);
    assert_eq!(updated.workspace_root, root);
}

#[tokio::test]
async fn command_catalog_method_returns_tui_catalog() {
    let out =
        dispatch_and_collect(r#"{"jsonrpc":"2.0","id":10,"method":"session.command_catalog"}"#)
            .await;
    assert_eq!(out[0]["jsonrpc"], "2.0");
    assert_eq!(out[0]["id"], 10);
    let commands = out[0]["result"]["commands"].as_array().unwrap();
    let catalog = peridot_tui::slash_command_catalog();
    assert_eq!(commands.len(), catalog.len());
    for (actual, expected) in commands.iter().zip(catalog.iter()) {
        assert_eq!(actual["name"], expected.name);
        assert_eq!(actual["description"], expected.description);
        assert_eq!(actual["category"], expected.category);
        let surfaces: Vec<&str> = actual["surfaces"]
            .as_array()
            .unwrap()
            .iter()
            .map(|value| value.as_str().unwrap())
            .collect();
        assert_eq!(surfaces, peridot_tui::slash_command_surfaces(expected));
        assert_eq!(
            actual["arg_hint"].as_str().unwrap_or(""),
            expected.arg_hint.unwrap_or("")
        );
        let arg_options: Vec<&str> = actual["arg_options"]
            .as_array()
            .unwrap()
            .iter()
            .map(|value| value.as_str().unwrap())
            .collect();
        assert_eq!(
            arg_options,
            peridot_tui::slash_command_arg_options(expected)
        );
    }
    assert!(commands.iter().any(|entry| entry["name"] == "/plan"));
    assert!(commands.iter().any(|entry| {
        entry["name"] == "/collapse" && entry["surfaces"] == serde_json::json!(["tui"])
    }));
    assert!(commands.iter().any(|entry| {
        entry["name"] == "/sidepanel" && entry["surfaces"] == serde_json::json!(["tui"])
    }));
    assert!(commands.iter().any(|entry| {
        entry["name"] == "/reasoning"
            && entry["arg_options"] == serde_json::json!(["off", "low", "medium", "high", "xhigh"])
    }));
    assert!(commands.iter().any(|entry| {
        entry["name"] == "/provider"
            && entry["arg_options"]
                == serde_json::json!(["claude-api", "openai-api", "openrouter-api", "openai-oauth"])
    }));
    assert!(commands.iter().any(|entry| {
        entry["name"] == "/codemap"
            && entry["arg_options"]
                == serde_json::json!(["status", "refresh", "find", "locate", "outline", "refs"])
    }));
    assert!(
        commands
            .iter()
            .any(|entry| entry["name"] == "/branch switch")
    );
    assert!(
        commands
            .iter()
            .all(|entry| entry["description"].as_str().is_some())
    );
}

#[tokio::test]
async fn command_catalog_method_filters_by_surface() {
    let out = dispatch_and_collect(
            r#"{"jsonrpc":"2.0","id":10,"method":"session.command_catalog","params":{"surface":"vscode"}}"#,
        )
        .await;
    assert_eq!(out[0]["jsonrpc"], "2.0");
    let commands = out[0]["result"]["commands"].as_array().unwrap();
    assert!(commands.iter().any(|entry| entry["name"] == "/plan"));
    assert!(commands.iter().any(|entry| entry["name"] == "/status"));
    assert!(!commands.iter().any(|entry| entry["name"] == "/collapse"));
    assert!(!commands.iter().any(|entry| entry["name"] == "/sidepanel"));
    assert!(!commands.iter().any(|entry| entry["name"] == "/lang"));
    assert!(commands.iter().all(|entry| {
        entry["surfaces"]
            .as_array()
            .unwrap()
            .iter()
            .any(|surface| surface == "vscode")
    }));
}

#[tokio::test]
async fn session_command_help_returns_surface_filtered_catalog_rows() {
    let out = dispatch_and_collect(
            r#"{"jsonrpc":"2.0","id":11,"method":"session.command","params":{"command":"/help","surface":"vscode"}}"#,
        )
        .await;
    assert_eq!(out[0]["jsonrpc"], "2.0");
    let result = &out[0]["result"];
    assert_eq!(result["kind"], "help");
    assert_eq!(result["surface"], "vscode");
    let items = result["items"].as_array().unwrap();
    assert!(items.iter().any(|entry| entry["label"] == "/plan"));
    assert!(items.iter().any(|entry| entry["label"] == "/status"));
    assert!(!items.iter().any(|entry| entry["label"] == "/collapse"));
    assert!(!items.iter().any(|entry| entry["label"] == "/sidepanel"));
    assert!(!items.iter().any(|entry| entry["label"] == "/lang <en|ko>"));
    assert_eq!(result["total"].as_u64().unwrap(), items.len() as u64);
}

#[tokio::test]
async fn skills_list_returns_active_auto_skills() {
    let root = test_project("skills-list");
    let store = peridot_memory::MemoryStore::new(root.join(".peridot/memory.db"));
    store
        .save_skill(&peridot_memory::StoredSkill {
            name: "auto-fix-parser".into(),
            body: "repair parser tests".into(),
            description: "repair parser tests".into(),
            scope: "auto".into(),
            ..Default::default()
        })
        .unwrap();
    store
        .save_skill(&peridot_memory::StoredSkill {
            name: "community-skill".into(),
            body: "community".into(),
            scope: "community".into(),
            ..Default::default()
        })
        .unwrap();
    store
        .save_skill(&peridot_memory::StoredSkill {
            name: "archived-auto".into(),
            body: "old".into(),
            scope: "auto".into(),
            archived_at_unix: 1,
            ..Default::default()
        })
        .unwrap();
    let (tx, mut rx) = mpsc::unbounded_channel::<String>();
    let state = DaemonState::new(
        root.clone(),
        PeridotConfig::default(),
        test_options(None),
        tx,
    );
    let _ = dispatch_line(
        &state,
        r#"{"jsonrpc":"2.0","id":42,"method":"skills.list"}"#,
    )
    .await
    .unwrap();

    let line = rx.try_recv().unwrap();
    let value: Value = serde_json::from_str(&line).unwrap();
    let skills = value["result"]["skills"].as_array().unwrap();
    assert_eq!(skills.len(), 1);
    assert_eq!(skills[0]["name"], "auto-fix-parser");
    assert_eq!(skills[0]["description"], "repair parser tests");

    let _ = dispatch_line(
        &state,
        r#"{"jsonrpc":"2.0","id":43,"method":"skills.list","params":{"include_archived":true}}"#,
    )
    .await
    .unwrap();
    let line = rx.try_recv().unwrap();
    let value: Value = serde_json::from_str(&line).unwrap();
    let skills = value["result"]["skills"].as_array().unwrap();
    assert_eq!(skills.len(), 2);
    assert!(
        skills
            .iter()
            .any(|skill| skill["name"] == "archived-auto" && skill["archived"] == true)
    );
    let _ = std::fs::remove_dir_all(root);
}

#[tokio::test]
async fn session_command_skills_returns_active_skill_inventory() {
    let root = test_project("command-skills");
    let store = peridot_memory::MemoryStore::new(root.join(".peridot/memory.db"));
    store
        .save_skill(&peridot_memory::StoredSkill {
            name: "auto-fix-parser".into(),
            body: "repair parser tests".into(),
            description: "repair parser tests".into(),
            scope: "auto".into(),
            last_used_at_unix: 123,
            ..Default::default()
        })
        .unwrap();
    store
        .save_skill(&peridot_memory::StoredSkill {
            name: "review-flow".into(),
            body: "review checklist".into(),
            scope: "community".into(),
            pinned_at_unix: 456,
            ..Default::default()
        })
        .unwrap();
    let (tx, mut rx) = mpsc::unbounded_channel::<String>();
    let state = DaemonState::new(
        root.clone(),
        PeridotConfig::default(),
        test_options(None),
        tx,
    );

    let _ = dispatch_line(
        &state,
        r#"{"jsonrpc":"2.0","id":43,"method":"session.command","params":{"command":"/skills"}}"#,
    )
    .await
    .unwrap();

    let line = rx.try_recv().unwrap();
    let value: Value = serde_json::from_str(&line).unwrap();
    assert_eq!(value["result"]["kind"], "skills");
    assert_eq!(value["result"]["total"], 2);
    let items = value["result"]["items"].as_array().unwrap();
    assert!(items.iter().any(|item| {
        item["label"] == "/auto-fix-parser"
            && item["detail"] == "repair parser tests"
            && item["scope"] == "auto"
            && item["last_used_at_unix"] == 123
    }));
    assert!(items.iter().any(|item| {
        item["label"] == "/review-flow" && item["scope"] == "community" && item["pinned"] == true
    }));
    let _ = std::fs::remove_dir_all(root);
}

#[tokio::test]
async fn session_command_note_persists_and_lists_notes() {
    let root = test_project("command-notes");
    let (tx, _rx) = mpsc::unbounded_channel::<String>();
    let state = DaemonState::new(
        root.clone(),
        PeridotConfig::default(),
        test_options(None),
        tx,
    );

    let value = execute_session_command(
        &state,
        Some("note-session"),
        "/note first checkpoint",
        SlashCommand::Note("first checkpoint".to_string()),
    )
    .await
    .unwrap();
    assert_eq!(value["kind"], "note");
    assert_eq!(value["session_id"], "note-session");
    assert_eq!(value["note"]["text"], "first checkpoint");
    assert!(
        root.join(".peridot/sessions/note-session/notes.ndjson")
            .is_file()
    );

    execute_session_command(
        &state,
        Some("note-session"),
        "/note second checkpoint",
        SlashCommand::Note("second checkpoint".to_string()),
    )
    .await
    .unwrap();

    let value = execute_session_command(
        &state,
        Some("note-session"),
        "/notes last 1",
        SlashCommand::Notes(Some(1)),
    )
    .await
    .unwrap();
    assert_eq!(value["kind"], "notes");
    assert_eq!(value["total"], 2);
    assert_eq!(value["items"].as_array().unwrap().len(), 1);
    assert_eq!(value["items"][0]["text"], "second checkpoint");

    let value = execute_session_command(
        &state,
        Some("note-session"),
        "/notes clear",
        SlashCommand::NotesClear,
    )
    .await
    .unwrap();
    assert_eq!(value["kind"], "notes_clear");
    assert_eq!(value["session_id"], "note-session");
    assert_eq!(value["cleared"], true);
    assert!(
        !root
            .join(".peridot/sessions/note-session/notes.ndjson")
            .exists()
    );
    let _ = std::fs::remove_dir_all(root);
}

#[tokio::test]
async fn session_command_skills_show_returns_skill_detail() {
    let root = test_project("command-skills-show");
    let store = peridot_memory::MemoryStore::new(root.join(".peridot/memory.db"));
    store
        .save_skill(&peridot_memory::StoredSkill {
            name: "auto-fix-parser".into(),
            body: "repair parser tests\nrun cargo test".into(),
            description: "repair parser tests".into(),
            scope: "auto".into(),
            last_used_at_unix: 123,
            pinned_at_unix: 456,
            ..Default::default()
        })
        .unwrap();
    let (tx, mut rx) = mpsc::unbounded_channel::<String>();
    let state = DaemonState::new(
        root.clone(),
        PeridotConfig::default(),
        test_options(None),
        tx,
    );

    let _ = dispatch_line(
            &state,
            r#"{"jsonrpc":"2.0","id":44,"method":"session.command","params":{"command":"/skills show auto-fix-parser"}}"#,
        )
        .await
        .unwrap();

    let line = rx.try_recv().unwrap();
    let value: Value = serde_json::from_str(&line).unwrap();
    assert_eq!(value["result"]["kind"], "skill_detail");
    assert_eq!(value["result"]["name"], "auto-fix-parser");
    assert_eq!(value["result"]["label"], "/auto-fix-parser");
    assert_eq!(value["result"]["detail"], "repair parser tests");
    assert_eq!(value["result"]["scope"], "auto");
    assert_eq!(value["result"]["pinned"], true);
    assert_eq!(value["result"]["last_used_at_unix"], 123);
    assert_eq!(
        value["result"]["body"],
        "repair parser tests\nrun cargo test"
    );
    let _ = std::fs::remove_dir_all(root);
}

#[tokio::test]
async fn session_command_skills_search_returns_matching_inventory() {
    let root = test_project("command-skills-search");
    let store = peridot_memory::MemoryStore::new(root.join(".peridot/memory.db"));
    store
        .save_skill(&peridot_memory::StoredSkill {
            name: "auto-fix-parser".into(),
            body: "repair parser tests".into(),
            description: "repair parser tests".into(),
            scope: "auto".into(),
            ..Default::default()
        })
        .unwrap();
    store
        .save_skill(&peridot_memory::StoredSkill {
            name: "release-notes".into(),
            body: "prepare changelog".into(),
            description: "write release notes".into(),
            scope: "auto".into(),
            ..Default::default()
        })
        .unwrap();
    let (tx, mut rx) = mpsc::unbounded_channel::<String>();
    let state = DaemonState::new(
        root.clone(),
        PeridotConfig::default(),
        test_options(None),
        tx,
    );

    let _ = dispatch_line(
            &state,
            r#"{"jsonrpc":"2.0","id":44,"method":"session.command","params":{"command":"/skills search parser"}}"#,
        )
        .await
        .unwrap();

    let line = rx.try_recv().unwrap();
    let value: Value = serde_json::from_str(&line).unwrap();
    assert_eq!(value["result"]["kind"], "skills");
    assert_eq!(value["result"]["query"], "parser");
    assert_eq!(value["result"]["total"], 1);
    assert_eq!(value["result"]["items"][0]["label"], "/auto-fix-parser");
    let _ = std::fs::remove_dir_all(root);
}

#[tokio::test]
async fn session_command_skills_pin_toggles_skill_inventory() {
    let root = test_project("command-skills-pin");
    let store = peridot_memory::MemoryStore::new(root.join(".peridot/memory.db"));
    store
        .save_skill(&peridot_memory::StoredSkill {
            name: "auto-fix-parser".into(),
            body: "repair parser tests".into(),
            description: "repair parser tests".into(),
            scope: "auto".into(),
            ..Default::default()
        })
        .unwrap();
    let (tx, mut rx) = mpsc::unbounded_channel::<String>();
    let state = DaemonState::new(
        root.clone(),
        PeridotConfig::default(),
        test_options(None),
        tx,
    );

    let _ = dispatch_line(
            &state,
            r#"{"jsonrpc":"2.0","id":44,"method":"session.command","params":{"command":"/skills pin auto-fix-parser"}}"#,
        )
        .await
        .unwrap();

    let line = rx.try_recv().unwrap();
    let value: Value = serde_json::from_str(&line).unwrap();
    assert_eq!(value["result"]["kind"], "skills");
    assert_eq!(value["result"]["message"], "pinned skill `auto-fix-parser`");
    assert_eq!(value["result"]["items"][0]["pinned"], true);
    assert!(
        store
            .list_skills()
            .unwrap()
            .iter()
            .any(|skill| skill.name == "auto-fix-parser" && skill.pinned_at_unix > 0)
    );

    let _ = dispatch_line(
            &state,
            r#"{"jsonrpc":"2.0","id":45,"method":"session.command","params":{"command":"/skills unpin auto-fix-parser"}}"#,
        )
        .await
        .unwrap();

    let line = rx.try_recv().unwrap();
    let value: Value = serde_json::from_str(&line).unwrap();
    assert_eq!(
        value["result"]["message"],
        "unpinned skill `auto-fix-parser`"
    );
    assert_eq!(value["result"]["items"][0]["pinned"], false);
    assert!(
        store
            .list_skills()
            .unwrap()
            .iter()
            .any(|skill| skill.name == "auto-fix-parser" && skill.pinned_at_unix == 0)
    );
    let _ = std::fs::remove_dir_all(root);
}

#[tokio::test]
async fn session_command_skills_archive_hides_skill_inventory() {
    let root = test_project("command-skills-archive");
    let skill_dir = root.join(".peridot/skills/auto");
    std::fs::create_dir_all(&skill_dir).unwrap();
    std::fs::write(skill_dir.join("auto-fix-parser.md"), "repair parser tests").unwrap();
    let store = peridot_memory::MemoryStore::new(root.join(".peridot/memory.db"));
    store
        .save_skill(&peridot_memory::StoredSkill {
            name: "auto-fix-parser".into(),
            body: "repair parser tests".into(),
            description: "repair parser tests".into(),
            scope: "auto".into(),
            ..Default::default()
        })
        .unwrap();
    let (tx, mut rx) = mpsc::unbounded_channel::<String>();
    let state = DaemonState::new(
        root.clone(),
        PeridotConfig::default(),
        test_options(None),
        tx,
    );

    let _ = dispatch_line(
            &state,
            r#"{"jsonrpc":"2.0","id":44,"method":"session.command","params":{"command":"/skills archive auto-fix-parser"}}"#,
        )
        .await
        .unwrap();

    let line = rx.try_recv().unwrap();
    let value: Value = serde_json::from_str(&line).unwrap();
    assert_eq!(value["result"]["kind"], "skills");
    assert_eq!(
        value["result"]["message"],
        "archived skill `auto-fix-parser`"
    );
    assert_eq!(value["result"]["total"], 0);
    assert!(store.list_skills().unwrap().is_empty());
    assert!(
        root.join(".peridot/skills/archive/auto-fix-parser.md")
            .is_file()
    );

    let _ = dispatch_line(
            &state,
            r#"{"jsonrpc":"2.0","id":45,"method":"session.command","params":{"command":"/skills archived parser"}}"#,
        )
        .await
        .unwrap();

    let line = rx.try_recv().unwrap();
    let value: Value = serde_json::from_str(&line).unwrap();
    assert_eq!(value["result"]["kind"], "skills");
    assert_eq!(value["result"]["archived"], true);
    assert_eq!(value["result"]["total"], 1);
    assert_eq!(value["result"]["items"][0]["label"], "/auto-fix-parser");
    assert_eq!(value["result"]["items"][0]["archived"], true);

    let _ = dispatch_line(
            &state,
            r#"{"jsonrpc":"2.0","id":47,"method":"session.command","params":{"command":"/skills show auto-fix-parser"}}"#,
        )
        .await
        .unwrap();

    let line = rx.try_recv().unwrap();
    let value: Value = serde_json::from_str(&line).unwrap();
    assert_eq!(value["result"]["kind"], "skill_detail");
    assert_eq!(value["result"]["archived"], true);
    assert_eq!(value["result"]["body"], "repair parser tests");

    let _ = dispatch_line(
            &state,
            r#"{"jsonrpc":"2.0","id":46,"method":"session.command","params":{"command":"/skills restore auto-fix-parser"}}"#,
        )
        .await
        .unwrap();

    let line = rx.try_recv().unwrap();
    let value: Value = serde_json::from_str(&line).unwrap();
    assert_eq!(value["result"]["kind"], "skills");
    assert_eq!(
        value["result"]["message"],
        "restored skill `auto-fix-parser`"
    );
    assert_eq!(value["result"]["total"], 1);
    assert_eq!(store.list_skills().unwrap().len(), 1);
    assert!(
        root.join(".peridot/skills/auto/auto-fix-parser.md")
            .is_file()
    );
    let _ = std::fs::remove_dir_all(root);
}

#[tokio::test]
async fn session_list_returns_persisted_records() {
    let root = test_project("session-list");
    let store = MemoryStore::new(root.join(".peridot/memory.db"));
    let mut record = SessionRecord::new("session-recorded", &root);
    record.summary = "recorded summary".into();
    record.status = SessionLifecycle::Suspended;
    record.created_at_unix = 10;
    record.updated_at_unix = 20;
    record.last_task = Some("recorded task".into());
    store.save_session_record(&record).unwrap();
    let note_dir = root.join(".peridot/sessions/session-recorded");
    std::fs::create_dir_all(&note_dir).unwrap();
    std::fs::write(
        note_dir.join("notes.ndjson"),
        r#"{"ts":1,"text":"first note"}
{"ts":2,"text":"latest note"}
"#,
    )
    .unwrap();
    let (tx, mut rx) = mpsc::unbounded_channel::<String>();
    let state = DaemonState::new(
        root.clone(),
        PeridotConfig::default(),
        test_options(None),
        tx,
    );
    write_context_snapshot(
        &state,
        "session-recorded",
        &[ContextEntry::trusted(
            ContextSource::PlanReminder,
            "[attachment]\npath: docs/spec.md\nbytes: 7\n\n```text\nattached\n```",
        )],
    )
    .unwrap();

    let _ = dispatch_line(
        &state,
        r#"{"jsonrpc":"2.0","id":44,"method":"session.list"}"#,
    )
    .await
    .unwrap();

    let line = rx.try_recv().unwrap();
    let value: Value = serde_json::from_str(&line).unwrap();
    let sessions = value["result"]["sessions"].as_array().unwrap();
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0]["id"], "session-recorded");
    assert_eq!(sessions[0]["title"], "recorded task");
    assert_eq!(sessions[0]["status"], "suspended");
    assert_eq!(sessions[0]["notes_count"], 2);
    assert_eq!(sessions[0]["last_note"], "latest note");
    assert_eq!(sessions[0]["attachment_count"], 1);
    assert_eq!(sessions[0]["attachment_paths"][0], "docs/spec.md");
    let _ = std::fs::remove_dir_all(root);
}

#[tokio::test]
async fn session_command_count_returns_lifecycle_breakdown() {
    let root = test_project("session-command-count");
    let store = MemoryStore::new(root.join(".peridot/memory.db"));
    for (id, status) in [
        ("idle-one", SessionLifecycle::Idle),
        ("running-one", SessionLifecycle::Running),
        ("done-one", SessionLifecycle::Done),
        ("done-two", SessionLifecycle::Done),
        ("failed-one", SessionLifecycle::Failed),
    ] {
        let mut record = SessionRecord::new(id, &root);
        record.status = status;
        store.save_session_record(&record).unwrap();
    }
    let (tx, mut rx) = mpsc::unbounded_channel::<String>();
    let state = DaemonState::new(
        root.clone(),
        PeridotConfig::default(),
        test_options(None),
        tx,
    );

    dispatch_line(
            &state,
            r#"{"jsonrpc":"2.0","id":45,"method":"session.command","params":{"command":"/session count"}}"#,
        )
        .await
        .unwrap();

    let line = rx.try_recv().unwrap();
    let value: Value = serde_json::from_str(&line).unwrap();
    let result = &value["result"];
    assert_eq!(result["kind"], "session_count");
    assert_eq!(result["total"], 5);
    assert_eq!(result["idle"], 1);
    assert_eq!(result["running"], 1);
    assert_eq!(result["done"], 2);
    assert_eq!(result["failed"], 1);
    assert_eq!(result["items"].as_array().unwrap().len(), 5);
    let _ = std::fs::remove_dir_all(root);
}

#[tokio::test]
async fn session_command_list_filters_by_status() {
    let root = test_project("session-command-list-status");
    let store = MemoryStore::new(root.join(".peridot/memory.db"));
    for (id, status) in [
        ("done-one", SessionLifecycle::Done),
        ("done-two", SessionLifecycle::Done),
        ("failed-one", SessionLifecycle::Failed),
    ] {
        let mut record = SessionRecord::new(id, &root);
        record.status = status;
        record.summary = format!("{id} summary");
        store.save_session_record(&record).unwrap();
    }
    let (tx, _rx) = mpsc::unbounded_channel::<String>();
    let state = DaemonState::new(
        root.clone(),
        PeridotConfig::default(),
        test_options(None),
        tx,
    );

    let result = execute_session_command(
        &state,
        None,
        "/session list --status done",
        SlashCommand::SessionListStatus("done".to_string()),
    )
    .await
    .unwrap();

    assert_eq!(result["kind"], "session_list");
    assert_eq!(result["status_filter"], "done");
    assert_eq!(result["total"], 2);
    assert_eq!(result["message"], "sessions (done): 2 total");
    let sessions = result["sessions"].as_array().unwrap();
    assert!(sessions.iter().all(|session| session["status"] == "done"));
    let ids: Vec<_> = sessions
        .iter()
        .map(|session| session["id"].as_str().unwrap())
        .collect();
    assert_eq!(ids, vec!["done-one", "done-two"]);
    let _ = std::fs::remove_dir_all(root);
}

#[tokio::test]
async fn session_command_prune_returns_structured_result_and_removes_records() {
    let root = test_project("session-command-prune");
    let store = MemoryStore::new(root.join(".peridot/memory.db"));
    for (id, status) in [
        ("done-one", SessionLifecycle::Done),
        ("failed-one", SessionLifecycle::Failed),
    ] {
        let mut record = SessionRecord::new(id, &root);
        record.status = status;
        store.save_session_record(&record).unwrap();
        peridot_memory::save_session_blob(
            &root.join(".peridot/sessions"),
            id,
            "tui_state.json",
            b"{}",
        )
        .unwrap();
    }
    let (tx, _rx) = mpsc::unbounded_channel::<String>();
    let state = DaemonState::new(
        root.clone(),
        PeridotConfig::default(),
        test_options(None),
        tx,
    );

    let preview = execute_session_command(
        &state,
        None,
        "/session prune --status done --dry-run",
        SlashCommand::SessionPrune {
            status: Some("done".to_string()),
            older_than_days: None,
            dry_run: true,
        },
    )
    .await
    .unwrap();
    assert_eq!(preview["kind"], "session_prune");
    assert_eq!(preview["dry_run"], true);
    assert_eq!(preview["considered"], serde_json::json!(["done-one"]));
    assert!(store.get_session_record("done-one").unwrap().is_some());

    let result = execute_session_command(
        &state,
        None,
        "/session prune --status done",
        SlashCommand::SessionPrune {
            status: Some("done".to_string()),
            older_than_days: None,
            dry_run: false,
        },
    )
    .await
    .unwrap();
    assert_eq!(result["kind"], "session_prune");
    assert_eq!(result["removed"], serde_json::json!(["done-one"]));
    assert_eq!(result["total"], 1);
    assert!(store.get_session_record("done-one").unwrap().is_none());
    assert!(store.get_session_record("failed-one").unwrap().is_some());
    assert!(!root.join(".peridot/sessions/done-one").exists());
    let _ = std::fs::remove_dir_all(root);
}

#[tokio::test]
async fn session_command_search_returns_structured_hits() {
    let root = test_project("session-command-search");
    let sessions_root = root.join(".peridot").join("sessions");
    let mut tui = peridot_tui::TuiState::new(peridot_tui::HeaderState::new(
        ExecutionMode::Execute,
        PermissionMode::Auto,
        "mock",
    ));
    tui.push_transcript_entry(peridot_tui::TranscriptKind::User, "parser panic");
    tui.push_transcript_entry(
        peridot_tui::TranscriptKind::Assistant,
        "patched parser panic",
    );
    peridot_memory::save_session_blob(
        &sessions_root,
        "search-session",
        "tui_state.json",
        &serde_json::to_vec(&tui).unwrap(),
    )
    .unwrap();
    let (tx, _rx) = mpsc::unbounded_channel::<String>();
    let state = DaemonState::new(
        root.clone(),
        PeridotConfig::default(),
        test_options(None),
        tx,
    );

    let result = execute_session_command(
        &state,
        None,
        "/session search parser",
        SlashCommand::SessionSearch("parser".into()),
    )
    .await
    .unwrap();

    assert_eq!(result["kind"], "session_search");
    assert_eq!(result["query"], "parser");
    assert_eq!(result["total"], 2);
    assert_eq!(result["truncated"], false);
    assert_eq!(result["items"][0]["session_id"], "search-session");
    assert_eq!(result["items"][0]["label"], "search-session[0] user");
    assert_eq!(result["items"][0]["detail"], "parser panic");
    assert_eq!(result["hits"][1]["index"], 1);
    let _ = std::fs::remove_dir_all(root);
}

#[tokio::test]
async fn session_command_replay_returns_structured_timeline() {
    let root = test_project("session-command-replay");
    let store = MemoryStore::new(root.join(".peridot/memory.db"));
    store
        .save_session(&SessionSummary {
            id: "replay-session".into(),
            summary: "replay target".into(),
        })
        .unwrap();
    let sessions_root = root.join(".peridot").join("sessions");
    let mut tui = peridot_tui::TuiState::new(peridot_tui::HeaderState::new(
        ExecutionMode::Execute,
        PermissionMode::Auto,
        "mock",
    ));
    tui.push_transcript_entry(peridot_tui::TranscriptKind::User, "first prompt");
    tui.push_transcript_entry(peridot_tui::TranscriptKind::Assistant, "first answer");
    tui.push_transcript_entry(peridot_tui::TranscriptKind::User, "second prompt");
    peridot_memory::save_session_blob(
        &sessions_root,
        "replay-session",
        "tui_state.json",
        &serde_json::to_vec(&tui).unwrap(),
    )
    .unwrap();
    let (tx, _rx) = mpsc::unbounded_channel::<String>();
    let state = DaemonState::new(
        root.clone(),
        PeridotConfig::default(),
        test_options(None),
        tx,
    );

    let result = execute_session_command(
        &state,
        None,
        "/session replay replay --last 2",
        SlashCommand::SessionReplay {
            target: "replay".into(),
            last: Some(2),
        },
    )
    .await
    .unwrap();

    assert_eq!(result["kind"], "session_replay");
    assert_eq!(result["found"], true);
    assert_eq!(result["session_id"], "replay-session");
    assert_eq!(result["total"], 3);
    assert_eq!(result["timeline_total"], 3);
    assert_eq!(result["truncated"], true);
    assert_eq!(result["timeline"].as_array().unwrap().len(), 2);
    assert_eq!(result["items"][0]["detail"], "first answer");
    assert_eq!(result["items"][1]["detail"], "second prompt");
    let _ = std::fs::remove_dir_all(root);
}

#[tokio::test]
async fn session_command_export_resolves_persisted_title() {
    let root = test_project("session-command-export");
    let store = MemoryStore::new(root.join(".peridot/memory.db"));
    store
        .save_session(&SessionSummary {
            id: "export-session".into(),
            summary: "export target".into(),
        })
        .unwrap();
    let session_dir = root.join(".peridot/sessions/export-session");
    std::fs::create_dir_all(&session_dir).unwrap();
    std::fs::write(
        session_dir.join("notes.ndjson"),
        "{\"ts\":1,\"text\":\"remember\"}\n",
    )
    .unwrap();
    let (tx, _rx) = mpsc::unbounded_channel::<String>();
    let state = DaemonState::new(
        root.clone(),
        PeridotConfig::default(),
        test_options(None),
        tx,
    );

    let result = execute_session_command(
        &state,
        None,
        "/session export export notes",
        SlashCommand::SessionExport {
            target: "export".into(),
            artifacts: vec![ExportArtifact::Notes],
        },
    )
    .await
    .unwrap();

    assert_eq!(result["kind"], "session_export");
    assert_eq!(result["target"], "export");
    assert_eq!(result["id"], "export-session");
    assert_eq!(result["artifact_classes"], serde_json::json!(["notes"]));
    let destination = result["destination"].as_str().unwrap();
    assert!(Path::new(destination).join("notes.ndjson").is_file());
    assert_eq!(result["artifacts"][0]["count"], 1);
    let _ = std::fs::remove_dir_all(root);
}

#[tokio::test]
async fn session_command_import_returns_structured_result() {
    let root = test_project("session-command-import");
    let source = root.join("portable-session");
    std::fs::create_dir_all(&source).unwrap();
    std::fs::write(
        source.join("transcript.ndjson"),
        "{\"kind\":\"user\",\"text\":\"task\"}\n",
    )
    .unwrap();
    std::fs::write(
        source.join("notes.ndjson"),
        "{\"ts\":1,\"text\":\"imported note\"}\n",
    )
    .unwrap();
    let context = vec![ContextEntry::trusted(
        ContextSource::PlanReminder,
        "[attachment]\npath: docs/imported.md\nbytes: 8\n\n```text\nimported\n```",
    )];
    std::fs::write(
        source.join("context.bin"),
        serde_json::to_vec(&context).unwrap(),
    )
    .unwrap();
    let (tx, _rx) = mpsc::unbounded_channel::<String>();
    let state = DaemonState::new(
        root.clone(),
        PeridotConfig::default(),
        test_options(None),
        tx,
    );

    let result = execute_session_command(
        &state,
        None,
        "/session import portable-session --id imported",
        SlashCommand::SessionImport {
            from: source.display().to_string(),
            id: Some("imported".into()),
            force: false,
        },
    )
    .await
    .unwrap();

    assert_eq!(result["kind"], "session_import");
    assert_eq!(result["id"], "imported");
    assert_eq!(result["session_id"], "imported");
    assert_eq!(result["total"], 3);
    assert_eq!(result["notes_count"], 1);
    assert_eq!(result["last_note"], "imported note");
    assert_eq!(result["attachment_count"], 1);
    assert_eq!(result["attachment_paths"][0], "docs/imported.md");
    assert!(
        result["items"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| item["label"] == "transcript.ndjson")
    );
    assert!(
        root.join(".peridot/sessions/imported/transcript.ndjson")
            .is_file()
    );
    let _ = std::fs::remove_dir_all(root);
}

#[tokio::test]
async fn session_command_show_returns_persisted_details() {
    let root = test_project("session-command-show");
    let store = MemoryStore::new(root.join(".peridot/memory.db"));
    let mut record = SessionRecord::new("show-session", &root);
    record.summary = "release prep".into();
    record.last_task = Some("release prep".into());
    record.status = SessionLifecycle::Suspended;
    record.total_tokens = 2048;
    record.total_cost_usd = 0.125;
    record.turns_used = 4;
    record.worktree_branch = Some("peridot/show-session".into());
    store.save_session_record(&record).unwrap();
    store
        .save_session(&SessionSummary {
            id: "show-session".into(),
            summary: "release prep".into(),
        })
        .unwrap();
    let session_dir = root.join(".peridot").join("sessions").join("show-session");
    std::fs::create_dir_all(&session_dir).unwrap();
    std::fs::write(
        session_dir.join("notes.ndjson"),
        r#"{"ts":1,"text":"first note"}"#,
    )
    .unwrap();
    let context = vec![ContextEntry::trusted(
        ContextSource::PlanReminder,
        "[attachment]\npath: docs/release.md\nbytes: 7\n\n```text\nrelease\n```",
    )];
    std::fs::write(
        session_dir.join("context.bin"),
        serde_json::to_vec(&context).unwrap(),
    )
    .unwrap();
    let (tx, _rx) = mpsc::unbounded_channel::<String>();
    let state = DaemonState::new(
        root.clone(),
        PeridotConfig::default(),
        test_options(None),
        tx,
    );

    let result = execute_session_command(
        &state,
        None,
        "/session show release",
        SlashCommand::SessionShow("release".into()),
    )
    .await
    .unwrap();

    assert_eq!(result["kind"], "session_show");
    assert_eq!(result["found"], true);
    assert_eq!(result["session_id"], "show-session");
    assert_eq!(result["session_title"], "release prep");
    assert_eq!(result["status"], "suspended");
    assert_eq!(result["total_tokens"], 2048);
    assert!((result["total_cost_usd"].as_f64().unwrap() - 0.125).abs() < 1e-9);
    assert_eq!(result["turns_used"], 4);
    assert_eq!(result["notes_count"], 1);
    assert_eq!(result["last_note"], "first note");
    assert_eq!(result["attachment_count"], 1);
    assert_eq!(result["attachment_paths"][0], "docs/release.md");
    assert_eq!(result["worktree_branch"], "peridot/show-session");
    assert_eq!(result["items"][0]["detail"], "show-session");
    assert!(
        result["items"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| { item["label"] == "attachments" && item["detail"] == "1" })
    );
    assert!(
        result["items"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| { item["source"] == "note" && item["detail"] == "first note" })
    );
    assert!(
        result["items"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| { item["source"] == "attachment" && item["path"] == "docs/release.md" })
    );
    let _ = std::fs::remove_dir_all(root);
}

#[tokio::test]
async fn session_command_locate_resolves_persisted_title() {
    let root = test_project("session-command-locate");
    let store = MemoryStore::new(root.join(".peridot/memory.db"));
    let mut record = SessionRecord::new("locate-session", &root);
    record.summary = "locate target".into();
    record.last_task = Some("locate target".into());
    store.save_session_record(&record).unwrap();
    let session_dir = root
        .join(".peridot")
        .join("sessions")
        .join("locate-session");
    std::fs::create_dir_all(&session_dir).unwrap();
    let (tx, _rx) = mpsc::unbounded_channel::<String>();
    let state = DaemonState::new(
        root.clone(),
        PeridotConfig::default(),
        test_options(None),
        tx,
    );

    let result = execute_session_command(
        &state,
        None,
        "/session locate locate",
        SlashCommand::SessionLocate("locate".into()),
    )
    .await
    .unwrap();

    assert_eq!(result["kind"], "session_locate");
    assert_eq!(result["session_id"], "locate-session");
    assert_eq!(result["exists"], true);
    assert_eq!(result["path"], session_dir.display().to_string());
    assert_eq!(
        result["items"][1]["path"],
        session_dir.display().to_string()
    );
    let _ = std::fs::remove_dir_all(root);
}

#[tokio::test]
async fn session_command_resume_returns_start_task() {
    let root = test_project("session-command-resume");
    let store = MemoryStore::new(root.join(".peridot/memory.db"));
    let mut record = SessionRecord::new("resume-session", &root);
    record.summary = "resume target".into();
    record.last_task = Some("fix parser".into());
    store.save_session_record(&record).unwrap();
    let (tx, _rx) = mpsc::unbounded_channel::<String>();
    let state = DaemonState::new(
        root.clone(),
        PeridotConfig::default(),
        test_options(None),
        tx,
    );

    let result = execute_session_command(
        &state,
        None,
        "/session resume fix",
        SlashCommand::SessionResume("fix".into()),
    )
    .await
    .unwrap();

    assert_eq!(result["kind"], "start_task");
    assert_eq!(result["title"], "Session Resume");
    assert_eq!(result["label"], "session resume");
    assert_eq!(result["session_id"], "resume-session");
    assert_eq!(result["summary"], "resume target");
    assert_eq!(
        result["task"],
        "Resume session resume-session from this summary: resume target"
    );
    assert_eq!(result["items"][0]["detail"], "resume-session");
    let _ = std::fs::remove_dir_all(root);
}

#[tokio::test]
async fn session_command_rename_updates_persisted_session() {
    let root = test_project("session-command-rename");
    let store = MemoryStore::new(root.join(".peridot/memory.db"));
    let mut record = SessionRecord::new("session-rename", &root);
    record.summary = "old title".into();
    record.last_task = Some("old task".into());
    record.status = SessionLifecycle::Suspended;
    record.updated_at_unix = 77;
    record.total_tokens = 4096;
    record.total_cost_usd = 0.25;
    record.turns_used = 8;
    store.save_session_record(&record).unwrap();
    store
        .save_session(&SessionSummary {
            id: "session-rename".into(),
            summary: "old title".into(),
        })
        .unwrap();
    let (tx, _rx) = mpsc::unbounded_channel::<String>();
    let state = DaemonState::new(
        root.clone(),
        PeridotConfig::default(),
        test_options(None),
        tx,
    );

    let result = execute_session_command(
        &state,
        Some("session-rename"),
        "/session rename session-rename release prep",
        SlashCommand::SessionRename {
            target: "session-rename".into(),
            title: "release prep".into(),
        },
    )
    .await
    .unwrap();

    assert_eq!(result["kind"], "session_rename");
    assert_eq!(result["session_id"], "session-rename");
    assert_eq!(result["session_title"], "release prep");
    assert_eq!(result["summary"], "release prep");
    assert_eq!(result["status"], "suspended");
    assert!(result["updated_at_unix"].as_u64().unwrap() >= 77);
    assert_eq!(result["total_tokens"], 4096);
    assert_eq!(result["turns_used"], 8);
    assert!((result["total_cost_usd"].as_f64().unwrap() - 0.25).abs() < 1e-9);
    assert_eq!(result["renamed"], true);
    let renamed = store.get_session_record("session-rename").unwrap().unwrap();
    assert_eq!(renamed.summary, "release prep");
    assert_eq!(
        store
            .get_session("session-rename")
            .unwrap()
            .unwrap()
            .summary,
        "release prep"
    );
    let _ = std::fs::remove_dir_all(root);
}

#[tokio::test]
async fn session_command_delete_removes_persisted_session() {
    let root = test_project("session-command-delete");
    let store = MemoryStore::new(root.join(".peridot/memory.db"));
    let record = SessionRecord::new("session-delete", &root);
    store.save_session_record(&record).unwrap();
    store
        .save_session(&SessionSummary {
            id: "session-delete".into(),
            summary: "delete me".into(),
        })
        .unwrap();
    let sessions_root = root.join(".peridot").join("sessions");
    peridot_memory::save_session_blob(
        &sessions_root,
        "session-delete",
        "tui_state.json",
        br#"{"sessions":[]}"#,
    )
    .unwrap();
    let (tx, _rx) = mpsc::unbounded_channel::<String>();
    let state = DaemonState::new(
        root.clone(),
        PeridotConfig::default(),
        test_options(None),
        tx,
    );

    let result = execute_session_command(
        &state,
        Some("session-delete"),
        "/session delete session-delete",
        SlashCommand::SessionDelete("session-delete".into()),
    )
    .await
    .unwrap();

    assert_eq!(result["kind"], "session_delete");
    assert_eq!(result["session_id"], "session-delete");
    assert_eq!(result["deleted"], true);
    assert!(
        store
            .get_session_record("session-delete")
            .unwrap()
            .is_none()
    );
    assert!(store.get_session("session-delete").unwrap().is_none());
    assert!(!sessions_root.join("session-delete").exists());
    let _ = std::fs::remove_dir_all(root);
}

#[tokio::test]
async fn session_command_close_removes_live_and_persisted_session() {
    let root = test_project("session-command-close");
    let store = MemoryStore::new(root.join(".peridot/memory.db"));
    let record = SessionRecord::new("session-close", &root);
    store.save_session_record(&record).unwrap();
    store
        .save_session(&SessionSummary {
            id: "session-close".into(),
            summary: "close me".into(),
        })
        .unwrap();
    let sessions_root = root.join(".peridot").join("sessions");
    peridot_memory::save_session_blob(
        &sessions_root,
        "session-close",
        "tui_state.json",
        br#"{"sessions":[]}"#,
    )
    .unwrap();
    let (tx, _rx) = mpsc::unbounded_channel::<String>();
    let state = DaemonState::new(
        root.clone(),
        PeridotConfig::default(),
        test_options(None),
        tx,
    );
    state.sessions.lock().await.insert(
        "session-close".to_string(),
        SessionEntry {
            cancel: CancelToken::new(),
            compact_request: Arc::new(AtomicBool::new(false)),
            task: None,
            spec: SessionRunSpec {
                task: "close active session".to_string(),
                mode: ExecutionMode::Execute,
                permission: PermissionMode::Auto,
                model: None,
                reasoning_effort: None,
                service_tier: None,
                config: PeridotConfig::default(),
            },
            usage: Arc::new(StdMutex::new(LiveSessionUsage::default())),
            plan: Arc::new(StdMutex::new(LiveSessionPlan::default())),
            goal: Arc::new(StdMutex::new(LiveSessionGoal::default())),
            approval_grants: Vec::new(),
            waiting_approval: None,
        },
    );

    let result = execute_session_command(
        &state,
        Some("session-close"),
        "/session close session-close",
        SlashCommand::SessionClose("session-close".into()),
    )
    .await
    .unwrap();

    assert_eq!(result["kind"], "session_close");
    assert_eq!(result["session_id"], "session-close");
    assert_eq!(result["deleted"], true);
    assert_eq!(result["cancelled"], true);
    assert!(!state.sessions.lock().await.contains_key("session-close"));
    assert!(store.get_session_record("session-close").unwrap().is_none());
    assert!(store.get_session("session-close").unwrap().is_none());
    assert!(!sessions_root.join("session-close").exists());
    shutdown_sessions(&state).await;
    let _ = std::fs::remove_dir_all(root);
}

#[tokio::test]
async fn session_command_switch_resolves_persisted_session() {
    let root = test_project("session-command-switch");
    let store = MemoryStore::new(root.join(".peridot/memory.db"));
    let mut record = SessionRecord::new("session-switch", &root);
    record.summary = "switch target".into();
    record.status = SessionLifecycle::Suspended;
    record.updated_at_unix = 42;
    record.total_tokens = 2048;
    record.total_cost_usd = 0.125;
    record.turns_used = 5;
    store.save_session_record(&record).unwrap();
    let (tx, _rx) = mpsc::unbounded_channel::<String>();
    let state = DaemonState::new(
        root.clone(),
        PeridotConfig::default(),
        test_options(None),
        tx,
    );

    let result = execute_session_command(
        &state,
        None,
        "/session switch target",
        SlashCommand::SessionSwitch("target".into()),
    )
    .await
    .unwrap();

    assert_eq!(result["kind"], "session_switch");
    assert_eq!(result["session_id"], "session-switch");
    assert_eq!(result["session_title"], "switch target");
    assert_eq!(result["status"], "suspended");
    assert_eq!(result["summary"], "switch target");
    assert_eq!(result["updated_at_unix"], 42);
    assert_eq!(result["total_tokens"], 2048);
    assert_eq!(result["turns_used"], 5);
    assert!((result["total_cost_usd"].as_f64().unwrap() - 0.125).abs() < 1e-9);
    assert_eq!(result["switched"], true);
    let _ = std::fs::remove_dir_all(root);
}

#[tokio::test]
async fn session_command_goal_start_returns_goal_start_task() {
    let (tx, _rx) = mpsc::unbounded_channel::<String>();
    let root = test_project("session-command-goal-start");
    let state = DaemonState::new(
        root.clone(),
        PeridotConfig::default(),
        test_options(None),
        tx,
    );

    let result = execute_session_command(
        &state,
        None,
        "/goal ship release",
        SlashCommand::GoalStart("ship release".into()),
    )
    .await
    .unwrap();

    assert_eq!(result["kind"], "start_task");
    assert_eq!(result["label"], "goal");
    assert_eq!(result["task"], "ship release");
    assert_eq!(result["state_delta"]["mode"], "goal");
    let _ = std::fs::remove_dir_all(root);
}

#[tokio::test]
async fn session_command_goal_mode_returns_state_delta() {
    let (tx, _rx) = mpsc::unbounded_channel::<String>();
    let root = test_project("session-command-goal-mode");
    let state = DaemonState::new(
        root.clone(),
        PeridotConfig::default(),
        test_options(None),
        tx,
    );

    let result = execute_session_command(&state, None, "/goal", SlashCommand::GoalMode)
        .await
        .unwrap();

    assert_eq!(result["kind"], "setting");
    assert_eq!(result["message"], "mode: goal");
    assert_eq!(result["state_delta"]["mode"], "goal");
    let _ = std::fs::remove_dir_all(root);
}

#[tokio::test]
async fn session_command_committee_returns_lowercase_state_delta() {
    let (tx, _rx) = mpsc::unbounded_channel::<String>();
    let root = test_project("session-command-committee");
    let state = DaemonState::new(
        root.clone(),
        PeridotConfig::default(),
        test_options(None),
        tx,
    );

    let result = execute_session_command(
        &state,
        None,
        "/committee full",
        SlashCommand::Committee(peridot_common::CommitteeMode::Full),
    )
    .await
    .unwrap();

    assert_eq!(result["kind"], "setting");
    assert_eq!(result["message"], "committee: full");
    assert_eq!(result["state_delta"]["committee_mode"], "full");
    let _ = std::fs::remove_dir_all(root);
}

#[tokio::test]
async fn session_command_new_returns_structured_client_intent() {
    let (tx, _rx) = mpsc::unbounded_channel::<String>();
    let root = test_project("session-command-new");
    let state = DaemonState::new(
        root.clone(),
        PeridotConfig::default(),
        test_options(None),
        tx,
    );

    let empty =
        execute_session_command(&state, None, "/session new", SlashCommand::SessionNew(None))
            .await
            .unwrap();
    assert_eq!(empty["kind"], "session_new");
    assert_eq!(empty["has_task"], false);
    assert!(empty.get("task").is_none_or(Value::is_null));
    let empty_id = empty["session_id"].as_str().unwrap();
    assert_eq!(empty["session_title"], "new session");
    assert_eq!(empty["status"], "idle");
    assert_eq!(empty["running"], false);
    assert_eq!(empty["total_tokens"], 0);
    assert!(state.sessions.lock().await.get(empty_id).is_none());
    assert!(
        MemoryStore::new(root.join(".peridot/memory.db"))
            .get_session_record(empty_id)
            .unwrap()
            .is_some()
    );

    let with_task = execute_session_command(
        &state,
        None,
        "/session new fix tests",
        SlashCommand::SessionNew(Some("fix tests".into())),
    )
    .await
    .unwrap();
    assert_eq!(with_task["kind"], "session_new");
    assert_eq!(with_task["task"], "fix tests");
    assert_eq!(with_task["session_title"], "fix tests");
    assert_eq!(with_task["has_task"], true);
    assert_ne!(with_task["action"], "local");
    let _ = std::fs::remove_dir_all(root);
}

#[tokio::test]
async fn session_command_clear_removes_live_and_persisted_session() {
    let root = test_project("session-command-clear");
    let store = MemoryStore::new(root.join(".peridot/memory.db"));
    let record = SessionRecord::new("session-clear", &root);
    store.save_session_record(&record).unwrap();
    store
        .save_session(&SessionSummary {
            id: "session-clear".into(),
            summary: "clear me".into(),
        })
        .unwrap();
    let sessions_root = root.join(".peridot").join("sessions");
    peridot_memory::save_session_blob(
        &sessions_root,
        "session-clear",
        "tui_state.json",
        br#"{"sessions":[]}"#,
    )
    .unwrap();
    let (tx, _rx) = mpsc::unbounded_channel::<String>();
    let state = DaemonState::new(
        root.clone(),
        PeridotConfig::default(),
        test_options(None),
        tx,
    );
    state.sessions.lock().await.insert(
        "session-clear".to_string(),
        SessionEntry {
            cancel: CancelToken::new(),
            compact_request: Arc::new(AtomicBool::new(false)),
            task: None,
            spec: SessionRunSpec {
                task: "clear active session".to_string(),
                mode: ExecutionMode::Execute,
                permission: PermissionMode::Auto,
                model: None,
                reasoning_effort: None,
                service_tier: None,
                config: PeridotConfig::default(),
            },
            usage: Arc::new(StdMutex::new(LiveSessionUsage::default())),
            plan: Arc::new(StdMutex::new(LiveSessionPlan::default())),
            goal: Arc::new(StdMutex::new(LiveSessionGoal::default())),
            approval_grants: Vec::new(),
            waiting_approval: None,
        },
    );

    let result =
        execute_session_command(&state, Some("session-clear"), "/clear", SlashCommand::Clear)
            .await
            .unwrap();

    assert_eq!(result["kind"], "client_action");
    assert_eq!(result["action"], "clear");
    assert_eq!(result["session_id"], "session-clear");
    assert_eq!(result["deleted"], true);
    assert_eq!(result["cancelled"], true);
    assert!(!state.sessions.lock().await.contains_key("session-clear"));
    assert!(store.get_session_record("session-clear").unwrap().is_none());
    assert!(store.get_session("session-clear").unwrap().is_none());
    assert!(!sessions_root.join("session-clear").exists());
    shutdown_sessions(&state).await;
    let _ = std::fs::remove_dir_all(root);
}

#[tokio::test]
async fn session_command_cost_returns_live_and_aggregate_usage() {
    let root = test_project("session-command-cost");
    let store = MemoryStore::new(root.join(".peridot/memory.db"));
    let mut active_record = SessionRecord::new("active-cost", &root);
    active_record.status = SessionLifecycle::Running;
    active_record.last_task = Some("active work".into());
    active_record.total_tokens = 1000;
    active_record.total_cost_usd = 0.05;
    active_record.turns_used = 2;
    store.save_session_record(&active_record).unwrap();
    let mut background_record = SessionRecord::new("background-cost", &root);
    background_record.status = SessionLifecycle::Done;
    background_record.last_task = Some("background work".into());
    background_record.total_tokens = 700;
    background_record.total_cost_usd = 0.04;
    background_record.turns_used = 1;
    store.save_session_record(&background_record).unwrap();

    let (tx, _rx) = mpsc::unbounded_channel::<String>();
    let mut options = test_options(None);
    options.budget_usd = 0.5;
    let state = DaemonState::new(root.clone(), PeridotConfig::default(), options, tx);
    let usage = Arc::new(StdMutex::new(LiveSessionUsage {
        total_tokens: 2000,
        cost_usd: 0.10,
        turns_used: 3,
        cost_limit: Some(0.5),
        turns_limit: Some(5),
        committee_planner_tokens: 120,
        committee_planner_cost_usd: 0.01,
        committee_reviewer_tokens: 180,
        committee_reviewer_cost_usd: 0.01,
    }));
    state.sessions.lock().await.insert(
        "active-cost".to_string(),
        SessionEntry {
            cancel: CancelToken::new(),
            compact_request: Arc::new(AtomicBool::new(false)),
            task: None,
            spec: SessionRunSpec {
                task: "active work".to_string(),
                mode: ExecutionMode::Execute,
                permission: PermissionMode::Auto,
                model: None,
                reasoning_effort: None,
                service_tier: None,
                config: PeridotConfig::default(),
            },
            usage,
            plan: Arc::new(StdMutex::new(LiveSessionPlan::default())),
            goal: Arc::new(StdMutex::new(LiveSessionGoal::default())),
            approval_grants: Vec::new(),
            waiting_approval: None,
        },
    );

    let result = execute_session_command(&state, Some("active-cost"), "/cost", SlashCommand::Cost)
        .await
        .unwrap();

    assert_eq!(result["kind"], "cost");
    assert_eq!(result["session_id"], "active-cost");
    assert_eq!(result["session_count"], 2);
    assert_eq!(result["current_tokens"], 2300);
    assert_eq!(result["total_tokens"], 3000);
    assert_eq!(result["executor_tokens"], 2700);
    assert_eq!(result["committee_tokens"], 300);
    assert!((result["current_cost_usd"].as_f64().unwrap() - 0.12).abs() < 1e-9);
    assert!((result["total_cost_usd"].as_f64().unwrap() - 0.16).abs() < 1e-9);
    assert!((result["executor_cost_usd"].as_f64().unwrap() - 0.14).abs() < 1e-9);
    assert!((result["committee_cost_usd"].as_f64().unwrap() - 0.02).abs() < 1e-9);
    assert_eq!(result["budget_limit_usd"], 0.5);
    assert_eq!(result["items"].as_array().unwrap().len(), 2);
    shutdown_sessions(&state).await;
    let _ = std::fs::remove_dir_all(root);
}

#[tokio::test]
async fn session_command_plan_show_returns_live_plan_snapshot() {
    let root = test_project("session-command-plan-show");
    let (tx, _rx) = mpsc::unbounded_channel::<String>();
    let state = DaemonState::new(
        root.clone(),
        PeridotConfig::default(),
        test_options(None),
        tx,
    );
    let plan = Arc::new(StdMutex::new(LiveSessionPlan {
        steps: vec![
            PlanStepUpdate {
                label: "scan workspace".to_string(),
                done: true,
            },
            PlanStepUpdate {
                label: "apply patch".to_string(),
                done: false,
            },
        ],
        current: Some(1),
    }));
    state.sessions.lock().await.insert(
        "session-plan".to_string(),
        SessionEntry {
            cancel: CancelToken::new(),
            compact_request: Arc::new(AtomicBool::new(false)),
            task: None,
            spec: SessionRunSpec {
                task: "ship plan".to_string(),
                mode: ExecutionMode::Execute,
                permission: PermissionMode::Auto,
                model: None,
                reasoning_effort: None,
                service_tier: None,
                config: PeridotConfig::default(),
            },
            usage: Arc::new(StdMutex::new(LiveSessionUsage::default())),
            plan,
            goal: Arc::new(StdMutex::new(LiveSessionGoal::default())),
            approval_grants: Vec::new(),
            waiting_approval: None,
        },
    );

    let result = execute_session_command(
        &state,
        Some("session-plan"),
        "/plan show",
        SlashCommand::PlanShow,
    )
    .await
    .unwrap();

    assert_eq!(result["kind"], "plan");
    assert_eq!(result["message"], "plan: 1/2 steps");
    assert_eq!(result["done"], 1);
    assert_eq!(result["total"], 2);
    assert_eq!(result["current"], 1);
    assert_eq!(result["items"][0]["detail"], "done");
    assert_eq!(result["items"][1]["detail"], "in_progress");
    assert_eq!(result["steps"][1]["text"], "apply patch");
    shutdown_sessions(&state).await;
    let _ = std::fs::remove_dir_all(root);
}

#[tokio::test]
async fn session_command_save_persists_live_session_record() {
    let root = test_project("session-command-save");
    let (tx, _rx) = mpsc::unbounded_channel::<String>();
    let state = DaemonState::new(
        root.clone(),
        PeridotConfig::default(),
        test_options(None),
        tx,
    );
    state.sessions.lock().await.insert(
        "session-save".to_string(),
        SessionEntry {
            cancel: CancelToken::new(),
            compact_request: Arc::new(AtomicBool::new(false)),
            task: None,
            spec: SessionRunSpec {
                task: "save this session".to_string(),
                mode: ExecutionMode::Execute,
                permission: PermissionMode::Auto,
                model: None,
                reasoning_effort: None,
                service_tier: None,
                config: PeridotConfig::default(),
            },
            usage: Arc::new(StdMutex::new(LiveSessionUsage {
                total_tokens: 1500,
                cost_usd: 0.08,
                turns_used: 4,
                ..LiveSessionUsage::default()
            })),
            plan: Arc::new(StdMutex::new(LiveSessionPlan::default())),
            goal: Arc::new(StdMutex::new(LiveSessionGoal::default())),
            approval_grants: Vec::new(),
            waiting_approval: None,
        },
    );
    let session_dir = root.join(".peridot/sessions/session-save");
    std::fs::create_dir_all(&session_dir).unwrap();
    std::fs::write(
        session_dir.join("notes.ndjson"),
        "{\"ts\":1,\"text\":\"save note\"}\n",
    )
    .unwrap();
    let context = vec![ContextEntry::trusted(
        ContextSource::PlanReminder,
        "[attachment]\npath: docs/save.md\nbytes: 4\n\n```text\nsave\n```",
    )];
    std::fs::write(
        session_dir.join("context.bin"),
        serde_json::to_vec(&context).unwrap(),
    )
    .unwrap();

    let result = execute_session_command(
        &state,
        Some("session-save"),
        "/session save",
        SlashCommand::SessionSave,
    )
    .await
    .unwrap();

    assert_eq!(result["kind"], "session_save");
    assert_eq!(result["session_id"], "session-save");
    assert_eq!(result["status"], "running");
    assert_eq!(result["total_tokens"], 1500);
    assert_eq!(result["turns_used"], 4);
    assert!((result["total_cost_usd"].as_f64().unwrap() - 0.08).abs() < 1e-9);
    assert_eq!(result["notes_count"], 1);
    assert_eq!(result["last_note"], "save note");
    assert_eq!(result["attachment_count"], 1);
    assert_eq!(result["attachment_paths"][0], "docs/save.md");
    let store = MemoryStore::new(root.join(".peridot/memory.db"));
    let record = store.get_session_record("session-save").unwrap().unwrap();
    assert_eq!(record.last_task.as_deref(), Some("save this session"));
    assert_eq!(record.total_tokens, 1500);
    assert_eq!(record.turns_used, 4);
    assert!(store.get_session("session-save").unwrap().is_some());
    shutdown_sessions(&state).await;
    let _ = std::fs::remove_dir_all(root);
}

#[tokio::test]
async fn session_command_goal_controls_live_goal_state() {
    let root = test_project("session-command-goal-control");
    let (tx, _rx) = mpsc::unbounded_channel::<String>();
    let state = DaemonState::new(
        root.clone(),
        PeridotConfig::default(),
        test_options(None),
        tx,
    );
    let goal = Arc::new(StdMutex::new(LiveSessionGoal {
        objective: Some("finish migration".to_string()),
        status: Some(GoalStatus::Running),
        started_at_unix: Some(123),
    }));
    state.sessions.lock().await.insert(
        "session-goal".to_string(),
        SessionEntry {
            cancel: CancelToken::new(),
            compact_request: Arc::new(AtomicBool::new(false)),
            task: None,
            spec: SessionRunSpec {
                task: "finish migration".to_string(),
                mode: ExecutionMode::Goal,
                permission: PermissionMode::Auto,
                model: None,
                reasoning_effort: None,
                service_tier: None,
                config: PeridotConfig::default(),
            },
            usage: Arc::new(StdMutex::new(LiveSessionUsage::default())),
            plan: Arc::new(StdMutex::new(LiveSessionPlan {
                steps: vec![
                    PlanStepUpdate {
                        label: "scan".to_string(),
                        done: true,
                    },
                    PlanStepUpdate {
                        label: "patch".to_string(),
                        done: false,
                    },
                ],
                current: Some(1),
            })),
            goal,
            approval_grants: Vec::new(),
            waiting_approval: None,
        },
    );

    let pause = execute_session_command(
        &state,
        Some("session-goal"),
        "/goal pause",
        SlashCommand::GoalPause,
    )
    .await
    .unwrap();
    assert_eq!(pause["kind"], "goal");
    assert_eq!(pause["status"], "paused");
    assert_eq!(pause["objective"], "finish migration");
    assert_eq!(pause["done"], 1);
    assert_eq!(pause["total"], 2);

    let resume = execute_session_command(
        &state,
        Some("session-goal"),
        "/goal resume",
        SlashCommand::GoalResume,
    )
    .await
    .unwrap();
    assert_eq!(resume["status"], "running");

    let clear = execute_session_command(
        &state,
        Some("session-goal"),
        "/goal clear",
        SlashCommand::GoalClear,
    )
    .await
    .unwrap();
    assert_eq!(clear["status"], "cleared");
    assert!(clear["objective"].is_null());
    assert_eq!(clear["total"], 0);
    shutdown_sessions(&state).await;
    let _ = std::fs::remove_dir_all(root);
}

#[tokio::test]
async fn session_command_info_returns_live_and_persisted_context() {
    let root = test_project("session-command-info");
    let store = MemoryStore::new(root.join(".peridot/memory.db"));
    let mut record = SessionRecord::new("session-info", &root);
    record.status = SessionLifecycle::Suspended;
    record.last_task = Some("inspect daemon info".into());
    record.total_tokens = 1234;
    record.total_cost_usd = 0.42;
    record.turns_used = 7;
    store.save_session_record(&record).unwrap();

    let mut config = PeridotConfig::default();
    config.auth.primary = "openai-api".to_string();
    config.models.main = "configured-model".to_string();
    let (tx, _rx) = mpsc::unbounded_channel::<String>();
    let state = DaemonState::new(root.clone(), config.clone(), test_options(None), tx);
    let mut spec_config = config;
    spec_config.auth.primary = "openrouter-api".to_string();
    state.sessions.lock().await.insert(
        "session-info".to_string(),
        SessionEntry {
            cancel: CancelToken::new(),
            compact_request: Arc::new(AtomicBool::new(false)),
            task: None,
            spec: SessionRunSpec {
                task: "inspect daemon info".to_string(),
                mode: ExecutionMode::Goal,
                permission: PermissionMode::Safe,
                model: Some("live-model".to_string()),
                reasoning_effort: Some(peridot_common::ReasoningEffort::High),
                service_tier: Some(Some("fast".to_string())),
                config: spec_config,
            },
            usage: Arc::new(StdMutex::new(LiveSessionUsage::default())),
            plan: Arc::new(StdMutex::new(LiveSessionPlan::default())),
            goal: Arc::new(StdMutex::new(LiveSessionGoal::default())),
            approval_grants: Vec::new(),
            waiting_approval: None,
        },
    );

    execute_session_command(
        &state,
        Some("session-info"),
        "/provider claude-api",
        SlashCommand::Provider("claude-api".to_string()),
    )
    .await
    .unwrap();
    let result = execute_session_command(&state, Some("session-info"), "/info", SlashCommand::Info)
        .await
        .unwrap();

    assert_eq!(result["kind"], "info");
    assert_eq!(result["session_id"], "session-info");
    assert_eq!(result["status"], "running");
    assert_eq!(result["model"], "live-model");
    assert_eq!(result["provider"], "claude-api");
    assert_eq!(result["mode"], "goal");
    assert_eq!(result["permission"], "safe");
    assert_eq!(result["reasoning_effort"], "high");
    assert_eq!(result["service_tier"], "fast");
    assert_eq!(result["turns_used"], 7);
    assert_eq!(result["total_tokens"], 1234);
    assert_eq!(result["total_cost_usd"], 0.42);
    let items = result["items"].as_array().unwrap();
    assert!(
        items.iter().any(|item| {
            item["label"] == "last task" && item["detail"] == "inspect daemon info"
        })
    );
    shutdown_sessions(&state).await;
    let _ = std::fs::remove_dir_all(root);
}

#[tokio::test]
async fn session_subscribe_list_emits_start_notifications() {
    let root = test_project("session-list-subscribe");
    let response_file = root.join("responses.jsonl");
    std::fs::write(
        &response_file,
        r#"{"action":"agent_done","parameters":{"summary":"done"}}
"#,
    )
    .unwrap();
    let (tx, mut rx) = mpsc::unbounded_channel::<String>();
    let state = DaemonState::new(
        root.clone(),
        PeridotConfig::default(),
        test_options(Some(response_file)),
        tx,
    );

    let _ = dispatch_line(
        &state,
        r#"{"jsonrpc":"2.0","id":45,"method":"session.subscribe_list"}"#,
    )
    .await
    .unwrap();
    let _ = dispatch_line(
        &state,
        r#"{"jsonrpc":"2.0","id":46,"method":"session.start","params":{"task":"sync me"}}"#,
    )
    .await
    .unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    let mut values = Vec::new();
    while let Ok(line) = rx.try_recv() {
        values.push(serde_json::from_str::<Value>(&line).unwrap());
    }
    let start_response = values
        .iter()
        .find(|value| value["id"] == 46)
        .expect("start response");
    let session_id = start_response["result"]["session_id"].as_str().unwrap();
    assert!(values.iter().any(|value| {
        value["method"] == "session.list_changed"
            && value["params"]["sessions"]
                .as_array()
                .map(|sessions| {
                    sessions
                        .iter()
                        .any(|session| session["id"] == session_id && session["running"] == true)
                })
                .unwrap_or(false)
    }));
    shutdown_sessions(&state).await;
    let _ = std::fs::remove_dir_all(root);
}

#[tokio::test]
async fn session_context_commands_emit_list_changed_notifications() {
    let root = test_project("session-context-list-changed");
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("src/lib.rs"), "pub fn attached() {}\n").unwrap();
    let store = MemoryStore::new(root.join(".peridot/memory.db"));
    store
        .save_session_record(&SessionRecord::new("context-session", &root))
        .unwrap();
    let (tx, mut rx) = mpsc::unbounded_channel::<String>();
    let state = DaemonState::new(
        root.clone(),
        PeridotConfig::default(),
        test_options(None),
        tx,
    );

    let _ = dispatch_line(
        &state,
        r#"{"jsonrpc":"2.0","id":45,"method":"session.subscribe_list"}"#,
    )
    .await
    .unwrap();
    while rx.try_recv().is_ok() {}

    let _ = dispatch_line(
            &state,
            r#"{"jsonrpc":"2.0","id":46,"method":"session.command","params":{"session_id":"context-session","command":"/note checkpoint"}}"#,
        )
        .await
        .unwrap();
    let note_values = drain_json_lines(&mut rx);
    let note_session = changed_session(&note_values, "context-session").expect("note list change");
    assert_eq!(note_session["notes_count"], 1);
    assert_eq!(note_session["last_note"], "checkpoint");

    let _ = dispatch_line(
            &state,
            r#"{"jsonrpc":"2.0","id":47,"method":"session.command","params":{"session_id":"context-session","command":"/attach src/lib.rs"}}"#,
        )
        .await
        .unwrap();
    let attach_values = drain_json_lines(&mut rx);
    let attach_session =
        changed_session(&attach_values, "context-session").expect("attach list change");
    assert_eq!(attach_session["attachment_count"], 1);
    assert_eq!(attach_session["attachment_paths"][0], "src/lib.rs");

    let _ = dispatch_line(
            &state,
            r#"{"jsonrpc":"2.0","id":48,"method":"session.command","params":{"session_id":"context-session","command":"/detach src/lib.rs"}}"#,
        )
        .await
        .unwrap();
    let detach_values = drain_json_lines(&mut rx);
    let detach_session =
        changed_session(&detach_values, "context-session").expect("detach list change");
    assert_eq!(detach_session["attachment_count"], 0);
    assert_eq!(
        detach_session["attachment_paths"].as_array().unwrap().len(),
        0
    );

    shutdown_sessions(&state).await;
    let _ = std::fs::remove_dir_all(root);
}

fn drain_json_lines(rx: &mut mpsc::UnboundedReceiver<String>) -> Vec<Value> {
    let mut values = Vec::new();
    while let Ok(line) = rx.try_recv() {
        values.push(serde_json::from_str::<Value>(&line).unwrap());
    }
    values
}

fn changed_session<'a>(values: &'a [Value], session_id: &str) -> Option<&'a Value> {
    values
        .iter()
        .rev()
        .filter(|value| value["method"] == "session.list_changed")
        .find_map(|value| {
            value["params"]["sessions"]
                .as_array()?
                .iter()
                .find(|session| session["id"] == session_id)
        })
}

#[tokio::test]
async fn session_command_skill_appends_plan_reminder_context() {
    let root = test_project("skill-command");
    let store = peridot_memory::MemoryStore::new(root.join(".peridot/memory.db"));
    store
        .save_skill(&peridot_memory::StoredSkill {
            name: "auto-fix-parser".into(),
            body: "## Steps\nRun parser tests".into(),
            description: "repair parser tests".into(),
            scope: "auto".into(),
            ..Default::default()
        })
        .unwrap();
    let (tx, mut rx) = mpsc::unbounded_channel::<String>();
    let state = DaemonState::new(
        root.clone(),
        PeridotConfig::default(),
        test_options(None),
        tx,
    );

    let _ = dispatch_line(
            &state,
            r#"{"jsonrpc":"2.0","id":43,"method":"session.command","params":{"session_id":"session-skill","command":"/auto-fix-parser --dry"}}"#,
        )
        .await
        .unwrap();

    let mut values = Vec::new();
    while let Ok(line) = rx.try_recv() {
        values.push(serde_json::from_str::<Value>(&line).unwrap());
    }
    assert!(
        values
            .iter()
            .any(|value| { value["id"] == 43 && value["result"]["kind"] == "skill" })
    );
    let entries = read_context_snapshot(&state, "session-skill").unwrap();
    let last = entries.last().unwrap();
    assert_eq!(last.source, ContextSource::PlanReminder);
    assert!(last.content.contains("[skill:auto-fix-parser]"));
    assert!(last.content.contains("Operator passed args: --dry"));
    assert!(last.content.contains("Run parser tests"));
    let _ = std::fs::remove_dir_all(root);
}

#[tokio::test]
async fn session_command_skills_use_appends_plan_reminder_context() {
    let root = test_project("skills-use-command");
    let store = peridot_memory::MemoryStore::new(root.join(".peridot/memory.db"));
    store
        .save_skill(&peridot_memory::StoredSkill {
            name: "auto-fix-parser".into(),
            body: "## Steps\nRun parser tests".into(),
            description: "repair parser tests".into(),
            scope: "auto".into(),
            ..Default::default()
        })
        .unwrap();
    let (tx, mut rx) = mpsc::unbounded_channel::<String>();
    let state = DaemonState::new(
        root.clone(),
        PeridotConfig::default(),
        test_options(None),
        tx,
    );

    let _ = dispatch_line(
            &state,
            r#"{"jsonrpc":"2.0","id":44,"method":"session.command","params":{"session_id":"session-skill","command":"/skills use auto-fix-parser --dry"}}"#,
        )
        .await
        .unwrap();

    let mut values = Vec::new();
    while let Ok(line) = rx.try_recv() {
        values.push(serde_json::from_str::<Value>(&line).unwrap());
    }
    assert!(
        values
            .iter()
            .any(|value| { value["id"] == 44 && value["result"]["kind"] == "skill" })
    );
    let entries = read_context_snapshot(&state, "session-skill").unwrap();
    let last = entries.last().unwrap();
    assert_eq!(last.source, ContextSource::PlanReminder);
    assert!(last.content.contains("[skill:auto-fix-parser]"));
    assert!(last.content.contains("Operator passed args: --dry"));
    let _ = std::fs::remove_dir_all(root);
}

#[tokio::test]
async fn session_command_attach_appends_file_context() {
    let root = test_project("attach-command");
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("src/lib.rs"), "pub fn attached() {}\n").unwrap();
    let (tx, mut rx) = mpsc::unbounded_channel::<String>();
    let state = DaemonState::new(
        root.clone(),
        PeridotConfig::default(),
        test_options(None),
        tx,
    );

    let _ = dispatch_line(
            &state,
            r#"{"jsonrpc":"2.0","id":44,"method":"session.command","params":{"session_id":"session-attach","command":"/attach src/lib.rs"}}"#,
        )
        .await
        .unwrap();

    let mut values = Vec::new();
    while let Ok(line) = rx.try_recv() {
        values.push(serde_json::from_str::<Value>(&line).unwrap());
    }
    let response = values
        .iter()
        .find(|value| value["id"] == 44 && value["result"]["kind"] == "attach")
        .expect("attach response");
    assert_eq!(response["result"]["attachment"]["path"], "src/lib.rs");
    assert_eq!(response["result"]["attachment"]["media_type"], "text/plain");
    assert_eq!(response["result"]["attachment"]["inlined"], true);
    assert!(
        response["result"]["attachment"]["content"]
            .as_str()
            .unwrap()
            .contains("pub fn attached()")
    );
    assert_eq!(response["result"]["items"][0]["source"], "attachment");
    assert_eq!(response["result"]["items"][0]["inlined"], true);
    let entries = read_context_snapshot(&state, "session-attach").unwrap();
    let last = entries.last().unwrap();
    assert_eq!(last.source, ContextSource::PlanReminder);
    assert!(last.content.contains("[attachment]"));
    assert!(last.content.contains("path: src/lib.rs"));
    assert!(last.content.contains("pub fn attached()"));

    std::fs::write(root.join("screen.png"), [0x89, b'P', b'N', b'G']).unwrap();
    let _ = dispatch_line(
            &state,
            r#"{"jsonrpc":"2.0","id":45,"method":"session.command","params":{"session_id":"session-attach","command":"/attach screen.png"}}"#,
        )
        .await
        .unwrap();
    let mut image_response = None;
    while let Ok(line) = rx.try_recv() {
        let value: Value = serde_json::from_str(&line).unwrap();
        if value["id"] == 45 {
            image_response = Some(value);
            break;
        }
    }
    let image_response = image_response.expect("image attach response");
    assert_eq!(image_response["result"]["kind"], "attach");
    assert_eq!(
        image_response["result"]["attachment"]["media_type"],
        "image/png"
    );
    assert_eq!(image_response["result"]["attachment"]["inlined"], false);
    assert!(image_response["result"]["attachment"]["content"].is_null());
    assert_eq!(image_response["result"]["items"][0]["inlined"], false);

    let _ = dispatch_line(
            &state,
            r#"{"jsonrpc":"2.0","id":46,"method":"session.command","params":{"session_id":"session-attach","command":"/attachments"}}"#,
        )
        .await
        .unwrap();
    let mut list_response = None;
    while let Ok(line) = rx.try_recv() {
        let value: Value = serde_json::from_str(&line).unwrap();
        if value["id"] == 46 {
            list_response = Some(value);
            break;
        }
    }
    let list_response = list_response.expect("attachments response");
    assert_eq!(list_response["result"]["kind"], "attachments");
    assert_eq!(list_response["result"]["total"], 2);
    assert_eq!(
        list_response["result"]["attachments"][0]["path"],
        "src/lib.rs"
    );
    assert_eq!(
        list_response["result"]["attachments"][1]["media_type"],
        "image/png"
    );

    let _ = dispatch_line(
            &state,
            r#"{"jsonrpc":"2.0","id":47,"method":"session.command","params":{"session_id":"session-attach","command":"/detach ./src/lib.rs"}}"#,
        )
        .await
        .unwrap();
    let mut detach_response = None;
    while let Ok(line) = rx.try_recv() {
        let value: Value = serde_json::from_str(&line).unwrap();
        if value["id"] == 47 {
            detach_response = Some(value);
            break;
        }
    }
    let detach_response = detach_response.expect("detach response");
    assert_eq!(detach_response["result"]["kind"], "detach");
    assert_eq!(detach_response["result"]["removed_count"], 1);
    assert_eq!(detach_response["result"]["remaining_count"], 1);
    assert_eq!(
        detach_response["result"]["removed"][0]["path"],
        "src/lib.rs"
    );
    assert_eq!(
        detach_response["result"]["attachments"][0]["path"],
        "screen.png"
    );
    let entries = read_context_snapshot(&state, "session-attach").unwrap();
    let remaining = crate::commands::attachments_from_context(&entries);
    assert_eq!(remaining.len(), 1);
    assert_eq!(remaining[0].path, "screen.png");

    let _ = std::fs::remove_dir_all(root);
}

#[tokio::test]
async fn session_command_export_writes_session_artifacts() {
    let root = test_project("session-export");
    let session_dir = root.join(".peridot/sessions/session-export");
    std::fs::create_dir_all(&session_dir).unwrap();
    let context = vec![ContextEntry::trusted(
        ContextSource::PlanReminder,
        "[attachment]\npath: src/lib.rs\nbytes: 5\n\n```text\nhello\n```",
    )];
    std::fs::write(
        session_dir.join("context.bin"),
        serde_json::to_vec(&context).unwrap(),
    )
    .unwrap();
    std::fs::write(
        session_dir.join("notes.ndjson"),
        "{\"ts\":1,\"text\":\"remember\"}\n",
    )
    .unwrap();
    std::fs::write(
        session_dir.join("transcript.ndjson"),
        "{\"kind\":\"user\",\"text\":\"task\",\"ts\":1}\n",
    )
    .unwrap();
    let (tx, mut rx) = mpsc::unbounded_channel::<String>();
    let state = DaemonState::new(
        root.clone(),
        PeridotConfig::default(),
        test_options(None),
        tx,
    );

    let _ = dispatch_line(
            &state,
            r#"{"jsonrpc":"2.0","id":48,"method":"session.command","params":{"session_id":"session-export","command":"/export full attachments notes timeline"}}"#,
        )
        .await
        .unwrap();
    let mut response = None;
    while let Ok(line) = rx.try_recv() {
        let value: Value = serde_json::from_str(&line).unwrap();
        if value["id"] == 48 {
            response = Some(value);
            break;
        }
    }
    let response = response.expect("export response");
    assert_eq!(response["result"]["kind"], "session_export");
    assert_eq!(
        response["result"]["artifact_classes"],
        serde_json::json!(["full", "attachments", "notes", "timeline"])
    );
    let destination = response["result"]["destination"].as_str().unwrap();
    assert!(Path::new(destination).join("context.bin").is_file());
    assert!(Path::new(destination).join("transcript.ndjson").is_file());
    assert!(Path::new(destination).join("attachments.json").is_file());
    assert!(Path::new(destination).join("notes.ndjson").is_file());
    assert!(Path::new(destination).join("timeline.json").is_file());
    assert_eq!(
        response["result"]["files"],
        serde_json::json!(["context.bin", "notes.ndjson", "transcript.ndjson"])
    );
    assert_eq!(response["result"]["items"][0]["source"], "full_copy");
    assert_eq!(response["result"]["items"][0]["label"], "context.bin");
    assert_eq!(response["result"]["items"][0]["detail"], "full copy");
    assert_eq!(response["result"]["artifacts"][0]["count"], 1);

    let _ = std::fs::remove_dir_all(root);
}

#[tokio::test]
async fn echo_method_returns_text_unchanged() {
    let out = dispatch_and_collect(
        r#"{"jsonrpc":"2.0","id":2,"method":"peridot.echo","params":{"text":"hello"}}"#,
    )
    .await;
    assert_eq!(out[0]["id"], 2);
    assert_eq!(out[0]["result"]["echo"], "hello");
}

#[tokio::test]
async fn echo_with_non_object_params_returns_invalid_params_error() {
    let out = dispatch_and_collect(
        r#"{"jsonrpc":"2.0","id":3,"method":"peridot.echo","params":"not-an-object"}"#,
    )
    .await;
    assert_eq!(out[0]["id"], 3);
    assert_eq!(out[0]["error"]["code"], -32602);
}

#[tokio::test]
async fn unknown_method_returns_method_not_found() {
    let out = dispatch_and_collect(r#"{"jsonrpc":"2.0","id":4,"method":"not.real"}"#).await;
    assert_eq!(out[0]["id"], 4);
    assert_eq!(out[0]["error"]["code"], -32601);
}

#[tokio::test]
async fn generate_title_rejects_missing_task() {
    let out = dispatch_and_collect(
        r#"{"jsonrpc":"2.0","id":51,"method":"session.generate_title","params":{}}"#,
    )
    .await;
    assert_eq!(out[0]["id"], 51);
    assert_eq!(out[0]["error"]["code"], -32602);
}

#[tokio::test]
async fn generate_title_rejects_empty_task() {
    let out = dispatch_and_collect(
        r#"{"jsonrpc":"2.0","id":52,"method":"session.generate_title","params":{"task":"   "}}"#,
    )
    .await;
    assert_eq!(out[0]["id"], 52);
    assert_eq!(out[0]["error"]["code"], -32602);
}

#[tokio::test]
async fn generate_title_rejects_non_object_params() {
    let out = dispatch_and_collect(
        r#"{"jsonrpc":"2.0","id":53,"method":"session.generate_title","params":"oops"}"#,
    )
    .await;
    assert_eq!(out[0]["id"], 53);
    assert_eq!(out[0]["error"]["code"], -32602);
}

#[tokio::test]
async fn settings_list_returns_curated_items_with_config_path() {
    let out = dispatch_and_collect(r#"{"jsonrpc":"2.0","id":80,"method":"settings.list"}"#).await;
    assert_eq!(out[0]["id"], 80);
    let result = &out[0]["result"];
    let items = result["items"].as_array().expect("items array present");
    assert!(
        items.len() >= 15,
        "expected curated registry to expose 15+ items, got {}",
        items.len()
    );
    // settings_registry must include a stable autonomy toggle so the
    // webview's `Auto-verify` section actually has something to render.
    let auto_verify = items
        .iter()
        .find(|i| i["id"] == "defaults.auto_verify_after_mutation")
        .expect("auto-verify item exposed");
    assert_eq!(auto_verify["value"]["kind"], "Bool");
    assert!(
        result["config_path"]
            .as_str()
            .unwrap_or_default()
            .ends_with("config.toml")
    );
}

#[tokio::test]
async fn settings_save_round_trips_through_list() {
    // Drive a real save+reload through the dispatcher so the on-disk
    // TOML round-trips end to end (encode, write, re-read, decode).
    let (tx, mut rx) = mpsc::unbounded_channel::<String>();
    let root = test_project("settings_round_trip");
    let state = DaemonState::new(
        root.clone(),
        PeridotConfig::default(),
        test_options(None),
        tx,
    );
    // Prime the config file via settings.list.
    dispatch_line(
        &state,
        r#"{"jsonrpc":"2.0","id":1,"method":"settings.list"}"#,
    )
    .await
    .unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(30)).await;
    let mut list_first = Vec::new();
    while let Ok(line) = rx.try_recv() {
        list_first.push(serde_json::from_str::<Value>(&line).unwrap());
    }
    // The handshake notification arrives ahead of the response, hence
    // we pick the entry with our id.
    let list_response = list_first
        .iter()
        .find(|v| v["id"] == 1)
        .expect("settings.list response");
    let mut items: Vec<Value> = list_response["result"]["items"].as_array().unwrap().clone();

    // Flip auto_verify_after_mutation to true.
    for item in items.iter_mut() {
        if item["id"] == "defaults.auto_verify_after_mutation" {
            item["value"]["data"] = Value::Bool(true);
        }
    }

    let save_req = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "settings.save",
        "params": { "items": items },
    });
    dispatch_line(&state, &serde_json::to_string(&save_req).unwrap())
        .await
        .unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(30)).await;
    let mut save_responses = Vec::new();
    while let Ok(line) = rx.try_recv() {
        save_responses.push(serde_json::from_str::<Value>(&line).unwrap());
    }
    let save_response = save_responses
        .iter()
        .find(|v| v["id"] == 2)
        .expect("settings.save response");
    assert_eq!(save_response["result"]["saved"], true);

    // Re-list and confirm the change survived a TOML round trip.
    dispatch_line(
        &state,
        r#"{"jsonrpc":"2.0","id":3,"method":"settings.list"}"#,
    )
    .await
    .unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(30)).await;
    let mut list_second = Vec::new();
    while let Ok(line) = rx.try_recv() {
        list_second.push(serde_json::from_str::<Value>(&line).unwrap());
    }
    let list_response_second = list_second
        .iter()
        .find(|v| v["id"] == 3)
        .expect("second settings.list response");
    let items_second = list_response_second["result"]["items"].as_array().unwrap();
    let saved_item = items_second
        .iter()
        .find(|i| i["id"] == "defaults.auto_verify_after_mutation")
        .unwrap();
    assert_eq!(saved_item["value"]["data"], true);

    shutdown_sessions(&state).await;
    let _ = std::fs::remove_dir_all(root);
}

#[tokio::test]
async fn settings_save_rejects_missing_items_array() {
    let out =
        dispatch_and_collect(r#"{"jsonrpc":"2.0","id":81,"method":"settings.save","params":{}}"#)
            .await;
    let response = out.iter().find(|v| v["id"] == 81).expect("save response");
    assert_eq!(response["error"]["code"], -32602);
}

#[tokio::test]
async fn settings_save_rejects_non_object_params() {
    let out = dispatch_and_collect(
        r#"{"jsonrpc":"2.0","id":82,"method":"settings.save","params":"oops"}"#,
    )
    .await;
    let response = out.iter().find(|v| v["id"] == 82).expect("save response");
    assert_eq!(response["error"]["code"], -32602);
}

#[tokio::test]
async fn handshake_emits_schema_and_daemon_version() {
    let (tx, mut rx) = mpsc::unbounded_channel::<String>();
    let root = test_project("handshake");
    let state = DaemonState::new(
        root.clone(),
        PeridotConfig::default(),
        test_options(None),
        tx,
    );
    emit_handshake(&state).unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    let line = rx.try_recv().unwrap();
    let value: Value = serde_json::from_str(&line).unwrap();
    assert_eq!(value["jsonrpc"], "2.0");
    assert_eq!(value["method"], "peridot.handshake");
    // Should not be a response/request — no id field on a notification.
    assert!(value.get("id").is_none());
    assert_eq!(
        value["params"]["schema_version"],
        peridot_core::AGENT_RUN_EVENT_SCHEMA_VERSION
    );
    assert_eq!(value["params"]["daemon_version"], env!("CARGO_PKG_VERSION"));
    shutdown_sessions(&state).await;
    let _ = std::fs::remove_dir_all(root);
}

#[tokio::test]
async fn session_command_todos_returns_structured_hits() {
    let (tx, mut rx) = mpsc::unbounded_channel::<String>();
    let root = test_project("command-todos");
    let src = root.join("src");
    std::fs::create_dir_all(&src).unwrap();
    std::fs::write(src.join("lib.rs"), "// TODO: wire command rpc\n").unwrap();
    let state = DaemonState::new(
        root.clone(),
        PeridotConfig::default(),
        test_options(None),
        tx,
    );
    let line = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 41,
        "method": "session.command",
        "params": { "command": "/todos" }
    })
    .to_string();

    let _ = dispatch_line(&state, &line).await.unwrap();
    let response: Value = serde_json::from_str(&rx.recv().await.unwrap()).unwrap();

    assert_eq!(response["id"], 41);
    assert_eq!(response["result"]["kind"], "todos");
    assert_eq!(response["result"]["items"][0]["path"], "src/lib.rs");
    assert_eq!(response["result"]["items"][0]["line"], 1);

    shutdown_sessions(&state).await;
    let _ = std::fs::remove_dir_all(root);
}

#[tokio::test]
async fn session_command_codemap_returns_symbols_and_todos() {
    let (tx, mut rx) = mpsc::unbounded_channel::<String>();
    let root = test_project("command-codemap");
    let src = root.join("src");
    std::fs::create_dir_all(&src).unwrap();
    std::fs::write(
        src.join("lib.rs"),
        "pub struct Runner;\n// TODO: finish codemap\nfn use_runner(value: Runner) {}\n",
    )
    .unwrap();
    let state = DaemonState::new(
        root.clone(),
        PeridotConfig::default(),
        test_options(None),
        tx,
    );
    let line = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 42,
        "method": "session.command",
        "params": { "command": "/codemap" }
    })
    .to_string();

    let _ = dispatch_line(&state, &line).await.unwrap();
    let mut response = Value::Null;
    while let Some(line) = rx.recv().await {
        let value: Value = serde_json::from_str(&line).unwrap();
        if value["id"] == 42 {
            response = value;
            break;
        }
    }

    assert_eq!(response["id"], 42);
    assert_eq!(response["result"]["kind"], "codemap");
    assert_eq!(response["result"]["symbol_count"], 1);
    assert_eq!(response["result"]["todo_count"], 1);
    assert_eq!(response["result"]["refreshed"], true);
    assert!(root.join(".peridot/codemap.json").is_file());
    assert!(
        response["result"]["items"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| item["source"] == "symbol" && item["label"] == "struct Runner")
    );
    assert!(
        response["result"]["items"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| item["source"] == "todo"
                && item["detail"].as_str().unwrap().contains("TODO"))
    );

    let status_line = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 421,
        "method": "session.command",
        "params": { "command": "/codemap status" }
    })
    .to_string();
    let _ = dispatch_line(&state, &status_line).await.unwrap();
    let status_response: Value = serde_json::from_str(&rx.recv().await.unwrap()).unwrap();
    assert_eq!(status_response["id"], 421);
    assert_eq!(status_response["result"]["kind"], "codemap_status");
    assert_eq!(status_response["result"]["index_exists"], true);
    assert_eq!(status_response["result"]["symbol_count"], 1);
    assert_eq!(status_response["result"]["todo_count"], 1);
    assert_eq!(status_response["result"]["source_files"], 1);

    let refresh_line = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 43,
        "method": "session.command",
        "params": { "command": "/codemap refresh" }
    })
    .to_string();
    let _ = dispatch_line(&state, &refresh_line).await.unwrap();
    let refresh_response: Value = serde_json::from_str(&rx.recv().await.unwrap()).unwrap();
    assert_eq!(refresh_response["id"], 43);
    assert_eq!(refresh_response["result"]["kind"], "codemap");
    assert_eq!(refresh_response["result"]["refreshed"], true);

    let find_line = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 44,
        "method": "session.command",
        "params": { "command": "/codemap find runner" }
    })
    .to_string();
    let _ = dispatch_line(&state, &find_line).await.unwrap();
    let find_response: Value = serde_json::from_str(&rx.recv().await.unwrap()).unwrap();
    assert_eq!(find_response["id"], 44);
    assert_eq!(find_response["result"]["kind"], "codemap");
    assert_eq!(
        find_response["result"]["title"],
        "Workspace Code Map Search"
    );
    assert_eq!(find_response["result"]["query"], "runner");
    assert_eq!(find_response["result"]["symbol_count"], 1);
    assert_eq!(find_response["result"]["todo_count"], 0);
    assert!(
        find_response["result"]["items"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| item["source"] == "symbol" && item["label"] == "struct Runner")
    );

    let locate_line = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 45,
        "method": "session.command",
        "params": { "command": "/codemap locate runner" }
    })
    .to_string();
    let _ = dispatch_line(&state, &locate_line).await.unwrap();
    let locate_response: Value = serde_json::from_str(&rx.recv().await.unwrap()).unwrap();
    assert_eq!(locate_response["id"], 45);
    assert_eq!(locate_response["result"]["kind"], "codemap");
    assert_eq!(
        locate_response["result"]["title"],
        "Workspace Symbol Locations"
    );
    assert_eq!(locate_response["result"]["query"], "runner");
    assert_eq!(locate_response["result"]["symbol_count"], 1);
    assert_eq!(locate_response["result"]["todo_count"], 0);
    assert_eq!(locate_response["result"]["items"][0]["path"], "src/lib.rs");
    assert_eq!(locate_response["result"]["items"][0]["line"], 1);

    let outline_line = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 46,
        "method": "session.command",
        "params": { "command": "/codemap outline src/lib.rs" }
    })
    .to_string();
    let _ = dispatch_line(&state, &outline_line).await.unwrap();
    let outline_response: Value = serde_json::from_str(&rx.recv().await.unwrap()).unwrap();
    assert_eq!(outline_response["id"], 46);
    assert_eq!(outline_response["result"]["kind"], "codemap");
    assert_eq!(
        outline_response["result"]["title"],
        "Workspace File Outline"
    );
    assert_eq!(outline_response["result"]["query"], "src/lib.rs");
    assert_eq!(outline_response["result"]["symbol_count"], 1);
    assert_eq!(outline_response["result"]["todo_count"], 0);
    assert!(
        outline_response["result"]["items"]
            .as_array()
            .unwrap()
            .iter()
            .all(|item| item["source"] == "symbol")
    );

    let refs_line = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 47,
        "method": "session.command",
        "params": { "command": "/codemap refs Runner" }
    })
    .to_string();
    let _ = dispatch_line(&state, &refs_line).await.unwrap();
    let refs_response: Value = serde_json::from_str(&rx.recv().await.unwrap()).unwrap();
    assert_eq!(refs_response["id"], 47);
    assert_eq!(refs_response["result"]["kind"], "codemap");
    assert_eq!(
        refs_response["result"]["title"],
        "Workspace Symbol References"
    );
    assert_eq!(refs_response["result"]["query"], "Runner");
    assert_eq!(refs_response["result"]["reference_count"], 1);
    assert_eq!(refs_response["result"]["symbol_count"], 0);
    assert_eq!(refs_response["result"]["todo_count"], 0);
    assert_eq!(refs_response["result"]["items"][0]["source"], "reference");
    assert_eq!(refs_response["result"]["items"][0]["line"], 3);

    shutdown_sessions(&state).await;
    let _ = std::fs::remove_dir_all(root);
}

#[tokio::test]
async fn session_command_branch_returns_picker_result() {
    let (tx, mut rx) = mpsc::unbounded_channel::<String>();
    let root = test_project("command-branch-picker");
    let state = DaemonState::new(
        root.clone(),
        PeridotConfig::default(),
        test_options(None),
        tx,
    );
    let session_id = "session-test-branch";
    let snapshot_path = context_snapshot_path(&state, session_id);
    std::fs::create_dir_all(snapshot_path.parent().unwrap()).unwrap();
    let mut first = ContextEntry::trusted(ContextSource::User, "draft the plan");
    first.turn_id = 1;
    let mut second = ContextEntry::trusted(ContextSource::Assistant, "implemented the plan");
    second.turn_id = 2;
    std::fs::write(
        &snapshot_path,
        serde_json::to_vec(&vec![first, second]).unwrap(),
    )
    .unwrap();
    let line = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 42,
        "method": "session.command",
        "params": { "session_id": session_id, "command": "/branch" }
    })
    .to_string();

    let _ = dispatch_line(&state, &line).await.unwrap();
    let mut response = Value::Null;
    while let Some(line) = rx.recv().await {
        let value: Value = serde_json::from_str(&line).unwrap();
        if value["id"] == 42 {
            response = value;
            break;
        }
    }

    assert_eq!(response["id"], 42);
    assert_eq!(response["result"]["kind"], "branch_picker");
    assert_eq!(response["result"]["items"].as_array().unwrap().len(), 2);
    assert_eq!(response["result"]["items"][0]["turn_id"], 1);
    assert_eq!(response["result"]["items"][0]["source"], "user");
    assert_eq!(response["result"]["items"][1]["turn_id"], 2);

    shutdown_sessions(&state).await;
    let _ = std::fs::remove_dir_all(root);
}

#[tokio::test]
async fn session_command_rewind_updates_context_snapshot() {
    let (tx, _rx) = mpsc::unbounded_channel::<String>();
    let root = test_project("command-rewind");
    let state = DaemonState::new(
        root.clone(),
        PeridotConfig::default(),
        test_options(None),
        tx,
    );
    let session_id = "session-test-rewind";
    let snapshot_path = context_snapshot_path(&state, session_id);
    std::fs::create_dir_all(snapshot_path.parent().unwrap()).unwrap();
    let mut first = ContextEntry::trusted(ContextSource::User, "first prompt");
    first.turn_id = 1;
    let mut first_reply = ContextEntry::trusted(ContextSource::Assistant, "first reply");
    first_reply.turn_id = 1;
    let mut second = ContextEntry::trusted(ContextSource::User, "second prompt");
    second.turn_id = 2;
    let mut second_reply = ContextEntry::trusted(ContextSource::Assistant, "second reply");
    second_reply.turn_id = 2;
    std::fs::write(
        &snapshot_path,
        serde_json::to_vec(&vec![first, first_reply, second, second_reply]).unwrap(),
    )
    .unwrap();
    let result = execute_session_command(&state, Some(session_id), "/rewind", SlashCommand::Rewind)
        .await
        .unwrap();

    assert_eq!(result["kind"], "rewind");
    assert_eq!(result["restored_prompt"], "second prompt");
    assert_eq!(result["removed_context_entries"], 2);
    let entries = read_context_snapshot(&state, session_id).unwrap();
    assert_eq!(entries.len(), 2);
    assert_eq!(entries[0].content, "first prompt");
    assert_eq!(entries[1].content, "first reply");

    shutdown_sessions(&state).await;
    let _ = std::fs::remove_dir_all(root);
}

#[tokio::test]
async fn session_start_without_task_returns_invalid_params() {
    let out = dispatch_and_collect(r#"{"jsonrpc":"2.0","id":5,"method":"session.start"}"#).await;
    assert_eq!(out[0]["id"], 5);
    assert_eq!(out[0]["error"]["code"], -32602);
}

#[tokio::test]
async fn session_start_rejects_invalid_reasoning_effort() {
    let out = dispatch_and_collect(
            r#"{"jsonrpc":"2.0","id":17,"method":"session.start","params":{"task":"finish","reasoning_effort":"huge"}}"#,
        )
        .await;
    assert_eq!(out[0]["id"], 17);
    assert_eq!(out[0]["error"]["code"], -32602);
}

#[tokio::test]
async fn session_start_rejects_invalid_service_tier() {
    let out = dispatch_and_collect(
            r#"{"jsonrpc":"2.0","id":18,"method":"session.start","params":{"task":"finish","service_tier":"expensive"}}"#,
        )
        .await;
    assert_eq!(out[0]["id"], 18);
    assert_eq!(out[0]["error"]["code"], -32602);
}

#[tokio::test]
async fn session_start_with_task_returns_id_and_started_event() {
    let root = test_project("mock");
    let response_file = root.join("responses.jsonl");
    std::fs::write(
        &response_file,
        r#"{"action":"agent_done","parameters":{"summary":"done"}}
"#,
    )
    .unwrap();
    let out = dispatch_and_collect_with_options(
        r#"{"jsonrpc":"2.0","id":6,"method":"session.start","params":{"task":"finish"}}"#,
        test_options(Some(response_file)),
    )
    .await;
    let session_id = out[0]["result"]["session_id"].as_str().unwrap();
    assert!(session_id.starts_with("session-"));
    assert!(out.iter().any(|value| {
        value["method"] == "event"
            && value["params"]["session_id"] == session_id
            && value["params"]["event"]["kind"] == "run_started"
    }));
    let _ = std::fs::remove_dir_all(root);
}

#[tokio::test]
async fn session_start_can_continue_requested_session_id() {
    let root = test_project("mock-continue");
    let response_file = root.join("responses.jsonl");
    std::fs::write(
        &response_file,
        r#"{"action":"agent_done","parameters":{"summary":"done"}}
"#,
    )
    .unwrap();
    let out = dispatch_and_collect_with_options(
            r#"{"jsonrpc":"2.0","id":16,"method":"session.start","params":{"task":"continue","session_id":"session-existing"}}"#,
            test_options(Some(response_file)),
        )
        .await;
    assert_eq!(out[0]["result"]["session_id"], "session-existing");
    assert!(out.iter().any(|value| {
        value["method"] == "event"
            && value["params"]["session_id"] == "session-existing"
            && value["params"]["event"]["kind"] == "run_started"
    }));
    let _ = std::fs::remove_dir_all(root);
}

#[tokio::test]
async fn session_cancel_unknown_id_returns_false() {
    let out = dispatch_and_collect(
        r#"{"jsonrpc":"2.0","id":7,"method":"session.cancel","params":{"session_id":"missing"}}"#,
    )
    .await;
    assert_eq!(out[0]["id"], 7);
    assert_eq!(out[0]["result"]["cancelled"], false);
    assert_eq!(out[0]["result"]["session_id"], "missing");
}

#[tokio::test]
async fn session_start_rejects_path_traversal_session_id() {
    // A client-supplied session_id is joined into `.peridot/sessions/<id>/`,
    // so a traversal id must be rejected before it can escape the sessions dir.
    let out = dispatch_and_collect(
        r#"{"jsonrpc":"2.0","id":9,"method":"session.start","params":{"task":"x","session_id":"../../../tmp/peridot-evil"}}"#,
    )
    .await;
    assert_eq!(out[0]["id"], 9);
    assert_eq!(out[0]["error"]["code"], -32602);
    assert!(out[0]["result"].is_null());
}

#[tokio::test]
async fn session_command_rejects_path_traversal_session_id() {
    let out = dispatch_and_collect(
        r#"{"jsonrpc":"2.0","id":10,"method":"session.command","params":{"session_id":"../escape","command":"/note hi"}}"#,
    )
    .await;
    assert_eq!(out[0]["id"], 10);
    assert_eq!(out[0]["error"]["code"], -32602);
    assert!(out[0]["result"].is_null());
}

#[tokio::test]
async fn fast_toggle_uses_current_session_tier() {
    let (tx, _rx) = mpsc::unbounded_channel::<String>();
    let root = test_project("fast-toggle");
    let state = DaemonState::new(
        root.clone(),
        PeridotConfig::default(),
        test_options(None),
        tx,
    );
    let session_id = "session-fast";
    state.sessions.lock().await.insert(
        session_id.to_string(),
        SessionEntry {
            cancel: CancelToken::new(),
            compact_request: Arc::new(AtomicBool::new(false)),
            task: None,
            spec: SessionRunSpec {
                task: "work".to_string(),
                mode: ExecutionMode::Execute,
                permission: PermissionMode::Auto,
                model: None,
                reasoning_effort: None,
                service_tier: None,
                config: PeridotConfig::default(),
            },
            usage: Arc::new(StdMutex::new(LiveSessionUsage::default())),
            plan: Arc::new(StdMutex::new(LiveSessionPlan::default())),
            goal: Arc::new(StdMutex::new(LiveSessionGoal::default())),
            approval_grants: Vec::new(),
            waiting_approval: None,
        },
    );

    let first = execute_session_command(
        &state,
        Some(session_id),
        "/fast toggle",
        SlashCommand::Fast(None),
    )
    .await
    .unwrap();
    assert_eq!(first["message"], "service tier: fast");
    assert_eq!(first["state_delta"]["service_tier"], "fast");
    assert_eq!(
        state
            .sessions
            .lock()
            .await
            .get(session_id)
            .unwrap()
            .spec
            .service_tier,
        Some(Some("fast".to_string()))
    );

    let second = execute_session_command(
        &state,
        Some(session_id),
        "/fast toggle",
        SlashCommand::Fast(None),
    )
    .await
    .unwrap();
    assert_eq!(second["message"], "service tier: standard");
    assert!(second["state_delta"]["service_tier"].is_null());
    assert_eq!(
        state
            .sessions
            .lock()
            .await
            .get(session_id)
            .unwrap()
            .spec
            .service_tier,
        Some(None)
    );

    shutdown_sessions(&state).await;
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn approval_response_parameters_override_snapshot_and_resume_sidecar() {
    let (tx, _rx) = mpsc::unbounded_channel::<String>();
    let root = test_project("approval-override");
    let state = DaemonState::new(
        root.clone(),
        PeridotConfig::default(),
        test_options(None),
        tx,
    );
    let session_id = "session-approval";
    let sidecar = context_snapshot_path(&state, session_id)
        .parent()
        .unwrap()
        .join("pending_resume.bin");
    std::fs::create_dir_all(sidecar.parent().unwrap()).unwrap();
    std::fs::write(
        &sidecar,
        serde_json::to_vec(&ToolCall::new(
            "file_patch",
            serde_json::json!({"path":"src/lib.rs","old_text":"a","new_text":"b"}),
        ))
        .unwrap(),
    )
    .unwrap();
    let snapshot = ApprovalRequestSnapshot {
        tool_name: "file_patch".to_string(),
        reason: "file_patch requires explicit user approval".to_string(),
        parameters: serde_json::json!({"path":"src/lib.rs","old_text":"a","new_text":"b"}),
        risk_class: Some("local_write".to_string()),
    };
    let params = serde_json::json!({
        "session_id": session_id,
        "approved": true,
        "scope": "once",
        "tool_name": "file_patch",
        "reason": "file_patch requires explicit user approval",
        "parameters": {"path":"src/lib.rs","old_text":"a","new_text":"partial"}
    });
    let params = params.as_object().unwrap();

    let overridden = approval_snapshot_from_response(&snapshot, params).unwrap();
    assert_eq!(overridden.parameters["new_text"], "partial");
    assert_eq!(overridden.risk_class.as_deref(), Some("local_write"));
    assert!(rewrite_pending_resume_parameters(
        &state,
        session_id,
        &overridden.parameters
    ));

    let bytes = std::fs::read(sidecar).unwrap();
    let call: ToolCall = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(call.parameters["new_text"], "partial");
    let _ = std::fs::remove_dir_all(root);
}

#[tokio::test]
async fn interaction_respond_unknown_request_returns_not_accepted() {
    let out = dispatch_and_collect(
            r#"{"jsonrpc":"2.0","id":10,"method":"interaction.respond","params":{"request_id":"missing","answer":{"kind":"cancelled"}}}"#,
        )
        .await;
    assert_eq!(out[0]["id"], 10);
    assert_eq!(out[0]["result"]["accepted"], false);
    assert_eq!(out[0]["result"]["request_id"], "missing");
}

#[tokio::test]
async fn daemon_ask_user_port_roundtrips_response() {
    let (tx, mut rx) = mpsc::unbounded_channel::<String>();
    let root = test_project("ask-user");
    let state = DaemonState::new(
        root.clone(),
        PeridotConfig::default(),
        test_options(None),
        tx,
    );
    let port = DaemonAskUserPort {
        state: state.clone(),
        session_id: "session-test".to_string(),
    };

    let ask_task = tokio::spawn(async move {
        port.ask(AskUserRequest::FreeForm {
            question: "Continue?".to_string(),
            hint: None,
            default: None,
        })
        .await
    });

    let line = tokio::time::timeout(std::time::Duration::from_secs(1), rx.recv())
        .await
        .unwrap()
        .unwrap();
    let value: Value = serde_json::from_str(&line).unwrap();
    let request_id = value["params"]["event"]["request_id"].as_str().unwrap();
    let response = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 11,
        "method": "interaction.respond",
        "params": {
            "request_id": request_id,
            "answer": { "kind": "text", "text": "yes" }
        }
    });
    dispatch_line(&state, &response.to_string()).await.unwrap();

    assert_eq!(
        ask_task.await.unwrap(),
        AskUserAnswer::Text("yes".to_string())
    );
    let _ = std::fs::remove_dir_all(root);
}

#[tokio::test]
async fn shutdown_with_id_returns_ack() {
    let out = dispatch_and_collect(r#"{"jsonrpc":"2.0","id":8,"method":"shutdown"}"#).await;
    assert_eq!(out[0]["id"], 8);
    assert_eq!(out[0]["result"]["shutdown"], true);
}

#[tokio::test]
async fn malformed_json_returns_parse_error_with_null_id() {
    let out = dispatch_and_collect("not json at all").await;
    assert!(out[0]["id"].is_null());
    assert_eq!(out[0]["error"]["code"], -32700);
}
