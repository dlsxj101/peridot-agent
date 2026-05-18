use super::auth::*;
use super::config::*;
use super::skills::*;
use super::update::*;
use super::*;

#[test]
fn collects_project_skills() {
    let root = std::env::temp_dir().join(format!("peridot-cli-skills-{}", std::process::id()));
    let skills_dir = root.join(".peridot/skills");
    fs::create_dir_all(&skills_dir).unwrap();
    fs::write(skills_dir.join("rust.md"), "Use cargo fmt.").unwrap();
    fs::create_dir_all(skills_dir.join("release-ci-prep")).unwrap();
    fs::write(
        skills_dir.join("release-ci-prep/SKILL.md"),
        "---\nname: release-ci-prep\ndescription: Release workflow.\n---\n",
    )
    .unwrap();

    let skills = collect_skills(&root).unwrap();

    assert!(skills.iter().any(|skill| skill.name == "rust"));
    assert!(skills.iter().any(|skill| skill.name == "release-ci-prep"));
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn loads_agents_preferences_into_effective_config() {
    let root = std::env::temp_dir().join(format!(
        "peridot-cli-agents-preferences-{}",
        std::process::id()
    ));
    fs::create_dir_all(&root).unwrap();
    fs::write(
        root.join("AGENTS.md"),
        "## preferences\n\
             default_mode: goal\n\
             default_permission: safe\n\
             ask_before_install: false\n\
             ask_before_delete: false\n",
    )
    .unwrap();

    let config = load_effective_config_inner(&root, None, false, false).unwrap();

    assert_eq!(config.defaults.mode, peridot_common::ExecutionMode::Goal);
    assert_eq!(
        config.defaults.permission,
        peridot_common::PermissionMode::Safe
    );
    assert!(!config.security.ask_before_install);
    assert!(!config.security.ask_before_delete);
    assert!(!config.git.auto_commit);
    assert_eq!(config.git.branch_prefix, "peridot/");
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn project_config_overrides_agents_preferences_selectively() {
    let root =
        std::env::temp_dir().join(format!("peridot-cli-config-merge-{}", std::process::id()));
    fs::create_dir_all(root.join(".peridot")).unwrap();
    fs::write(
        root.join("AGENTS.md"),
        "## preferences\n\
             default_mode: goal\n\
             default_permission: safe\n\
             ask_before_install: false\n\
             ask_before_delete: false\n",
    )
    .unwrap();
    fs::write(
        root.join(".peridot/config.toml"),
        "[defaults]\npermission = \"yolo\"\n\n[security]\nask_before_delete = true\n",
    )
    .unwrap();

    let config = load_effective_config_inner(&root, None, false, false).unwrap();

    assert_eq!(config.defaults.mode, peridot_common::ExecutionMode::Goal);
    assert_eq!(
        config.defaults.permission,
        peridot_common::PermissionMode::Yolo
    );
    assert!(!config.security.ask_before_install);
    assert!(config.security.ask_before_delete);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn project_config_merges_memory_and_tui_sections() {
    let root = std::env::temp_dir().join(format!(
        "peridot-cli-config-memory-tui-updates-{}",
        std::process::id()
    ));
    fs::create_dir_all(root.join(".peridot")).unwrap();
    fs::write(
            root.join(".peridot/config.toml"),
            "[memory]\nauto_skills = false\n\n[tui]\nshow_cost = false\nstream_speed = \"instant\"\n\n[updates]\nauto_check = false\nauto_check_interval = \"12h\"\n",
        )
        .unwrap();

    let config = load_effective_config_inner(&root, None, false, false).unwrap();

    assert!(!config.memory.auto_skills);
    assert!(!config.tui.show_cost);
    assert_eq!(config.tui.stream_speed, "instant");
    assert!(!config.updates.auto_check);
    assert_eq!(config.updates.auto_check_interval, "12h");
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn wizard_profile_writes_openrouter_config() {
    let config = config_from_wizard_profile(ConfigWizardProfile {
        auth_primary: "openrouter-api".to_string(),
        api_base_url: "https://openrouter.ai/api".to_string(),
        main_model: "openai/gpt-4o-mini".to_string(),
    });

    assert_eq!(config.auth.primary, "openrouter-api");
    assert_eq!(config.api.base_url, "https://openrouter.ai/api");
    assert_eq!(config.models.main, "openai/gpt-4o-mini");
    // goal_checker and compaction always mirror `main` — no separate field.
    assert_eq!(config.models.goal_checker(), "openai/gpt-4o-mini");
    assert_eq!(config.models.compaction(), "openai/gpt-4o-mini");
}

#[test]
fn config_set_rejects_separate_goal_checker_or_compaction() {
    let mut config = PeridotConfig::default();
    let err = set_config_key(&mut config, "models.goal_checker", "openai/gpt-4o-mini")
        .expect_err("goal_checker must not be settable independently");
    assert!(err.to_string().contains("follows `models.main`"));
    let err = set_config_key(&mut config, "models.compaction", "openai/gpt-4o-mini")
        .expect_err("compaction must not be settable independently");
    assert!(err.to_string().contains("follows `models.main`"));
}

#[test]
fn config_set_updates_known_keys() {
    let mut config = PeridotConfig::default();

    set_config_key(&mut config, "auth.primary", "openrouter-api").unwrap();
    set_config_key(&mut config, "models.main", "openai/gpt-4o-mini").unwrap();
    set_config_key(&mut config, "models.service_tier", "fast").unwrap();
    set_config_key(&mut config, "api.base_url", "https://openrouter.ai/api").unwrap();
    set_config_key(&mut config, "defaults.max_turns", "3").unwrap();
    set_config_key(&mut config, "git.auto_commit", "true").unwrap();

    assert_eq!(config.auth.primary, "openrouter-api");
    assert_eq!(config.models.main, "openai/gpt-4o-mini");
    assert_eq!(config.models.service_tier.as_deref(), Some("fast"));
    assert_eq!(config.api.base_url, "https://openrouter.ai/api");
    assert_eq!(config.defaults.max_turns, 3);
    assert!(config.git.auto_commit);
    assert!(set_config_key(&mut config, "unknown.key", "value").is_err());
    set_config_key(&mut config, "models.service_tier", "standard").unwrap();
    assert_eq!(config.models.service_tier, None);
}

#[test]
fn mcp_json_exposes_timeout_for_operator_status() {
    let server = McpServerConfig {
        name: "docs".to_string(),
        transport: McpTransport::Http,
        command: None,
        args: Vec::new(),
        env: Default::default(),
        url: Some("http://127.0.0.1:3333".to_string()),
        auth: None,
        timeout_seconds: 7,
    };

    let value = mcp::mcp_json(&server);

    assert_eq!(value["name"], "docs");
    assert_eq!(value["timeout_seconds"], 7);
    assert_eq!(value["configured"], true);
}

#[test]
fn mcp_validation_rejects_missing_http_url() {
    let server = McpServerConfig {
        name: "empty".to_string(),
        transport: McpTransport::Http,
        command: None,
        args: Vec::new(),
        env: Default::default(),
        url: None,
        auth: None,
        timeout_seconds: 30,
    };

    assert!(mcp::validate_mcp_server(&server).is_err());
}

#[test]
fn agents_git_preferences_feed_effective_config() {
    let root = std::env::temp_dir().join(format!(
        "peridot-cli-git-preferences-{}",
        std::process::id()
    ));
    fs::create_dir_all(&root).unwrap();
    fs::write(
        root.join("AGENTS.md"),
        "## preferences\n\
             auto_commit: true\n\
             commit_frequency: logical_unit\n\
             branch_prefix: agent/\n",
    )
    .unwrap();

    let config = load_effective_config_inner(&root, None, false, false).unwrap();

    assert!(config.git.auto_commit);
    assert_eq!(config.git.commit_frequency, "logical_unit");
    assert_eq!(config.git.branch_prefix, "agent/");
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn explicit_config_path_overrides_project_config_path() {
    let root = std::env::temp_dir().join(format!(
        "peridot-cli-explicit-config-{}",
        std::process::id()
    ));
    fs::create_dir_all(root.join(".peridot")).unwrap();
    let custom = root.join("custom-config.toml");
    fs::write(
        root.join(".peridot/config.toml"),
        "[defaults]\nmode = \"plan\"\n",
    )
    .unwrap();
    fs::write(&custom, "[defaults]\nmode = \"goal\"\n").unwrap();

    let config = load_effective_config_inner(&root, Some(&custom), false, false).unwrap();

    assert_eq!(config.defaults.mode, peridot_common::ExecutionMode::Goal);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn parses_env_override_values() {
    assert_eq!(
        parse_env_mode("PERIDOT_MODE", "goal").unwrap(),
        peridot_common::ExecutionMode::Goal
    );
    assert_eq!(
        parse_env_permission("PERIDOT_PERMISSION", "yolo").unwrap(),
        peridot_common::PermissionMode::Yolo
    );
    assert!(parse_env_mode("PERIDOT_MODE", "wander").is_err());
}

#[tokio::test]
async fn installs_local_skill_into_project_community_dir() {
    let root =
        std::env::temp_dir().join(format!("peridot-cli-install-skill-{}", std::process::id()));
    fs::create_dir_all(&root).unwrap();
    let source = root.join("My Skill.md");
    fs::write(&source, "Prefer focused tests.").unwrap();

    let installed = install_skill(&root, source.to_str().unwrap())
        .await
        .unwrap();
    let skills = collect_skills(&root).unwrap();

    assert_eq!(installed.name, "my-skill");
    assert!(
        installed
            .path
            .ends_with(".peridot/skills/community/my-skill.md")
    );
    assert!(
        skills
            .iter()
            .any(|skill| skill.name == "my-skill" && skill.scope == "project-community")
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn sanitizes_skill_names() {
    assert_eq!(
        skill_name_from_source("https://example.test/Rust Tips.md"),
        "rust-tips"
    );
    assert_eq!(sanitize_skill_name("..."), "skill");
}

#[test]
fn parses_github_repository_urls() {
    assert_eq!(
        github_owner_repo("https://github.com/peridot-ai/peridot.git"),
        Some(("peridot-ai".to_string(), "peridot".to_string()))
    );
    assert_eq!(
        github_owner_repo("git@github.com:peridot-ai/peridot"),
        Some(("peridot-ai".to_string(), "peridot".to_string()))
    );
}

#[test]
fn derives_pkce_challenge() {
    assert_eq!(
        pkce_challenge("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"),
        "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
    );
}

#[test]
fn parses_oauth_callback_query() {
    let request = "GET /callback?code=abc%20123&state=state%2Bvalue HTTP/1.1\r\n\r\n";

    let code = parse_oauth_callback(request, "state+value").unwrap();

    assert_eq!(code, "abc 123");
}

#[test]
fn detects_openai_oauth_token_expiry_window() {
    let expiring = serde_json::json!({
        "obtained_at_unix": unix_timestamp().saturating_sub(3500),
        "expires_in": 3600
    });
    let fresh = serde_json::json!({
        "obtained_at_unix": unix_timestamp(),
        "expires_in": 3600
    });

    assert!(openai_oauth_token_expires_within(&expiring, 300));
    assert!(!openai_oauth_token_expires_within(&fresh, 300));
}

#[test]
fn builds_openai_authorize_url_with_escaped_values() {
    let url = openai_oauth_authorize_url(
        "client id",
        "http://localhost:1455/auth/callback",
        "openid profile",
        "state",
        "challenge",
        "peridot",
    );

    assert!(url.contains("client_id=client%20id"));
    assert!(url.contains("redirect_uri=http%3A%2F%2Flocalhost%3A1455%2Fauth%2Fcallback"));
    assert!(url.contains("scope=openid%20profile"));
    assert!(url.contains("code_challenge_method=S256"));
    assert!(url.contains("id_token_add_organizations=true"));
    assert!(url.contains("codex_cli_simplified_flow=true"));
    assert!(url.contains("originator=peridot"));
}

#[test]
fn extracts_openai_oauth_identity_from_access_token() {
    let header = URL_SAFE_NO_PAD.encode(r#"{"alg":"none"}"#);
    let payload = URL_SAFE_NO_PAD.encode(
        r#"{"https://api.openai.com/auth":{"chatgpt_account_id":"acct_123","chatgpt_plan_type":"plus"},"https://api.openai.com/profile":{"email":"user@example.test"}}"#,
    );
    let token = format!("{header}.{payload}.sig");

    let identity = openai_oauth_access_token_identity(&token);

    assert_eq!(identity.account_id.as_deref(), Some("acct_123"));
    assert_eq!(identity.chatgpt_plan_type.as_deref(), Some("plus"));
    assert_eq!(identity.email.as_deref(), Some("user@example.test"));
}

#[test]
fn parses_manual_oauth_redirect_input() {
    let code = parse_authorization_input(
        "http://localhost:1455/auth/callback?code=abc%20123&state=state%2Bvalue",
        "state+value",
    )
    .unwrap();

    assert_eq!(code, "abc 123");
}

#[test]
fn stores_openrouter_key_in_env_store_file() {
    let root =
        std::env::temp_dir().join(format!("peridot-cli-openrouter-env-{}", std::process::id()));
    let path = root.join("env");

    let stored = upsert_env_var_file(&path, "OPENROUTER_API_KEY", "sk-or-test value").unwrap();
    let content = fs::read_to_string(&path).unwrap();

    assert_eq!(stored, path);
    assert_eq!(
        parse_local_env_value(&content, "OPENROUTER_API_KEY").as_deref(),
        Some("sk-or-test value")
    );
    assert!(content.contains("export OPENROUTER_API_KEY="));
    assert_eq!(
        parse_local_env_value(
            "export OPENROUTER_API_KEY=sk-or-test\n",
            "OPENROUTER_API_KEY"
        )
        .as_deref(),
        Some("sk-or-test")
    );
    assert!(parse_local_env_value("OTHER=1\n", "OPENROUTER_API_KEY").is_none());

    let removed = remove_env_var_file(&path, "OPENROUTER_API_KEY").unwrap();
    let content = fs::read_to_string(&path).unwrap();

    assert!(removed);
    assert!(parse_local_env_value(&content, "OPENROUTER_API_KEY").is_none());
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn finds_release_asset_url() {
    let release = serde_json::json!({
        "assets": [
            {"name": "peridot-x86_64-unknown-linux-gnu.tar.gz", "browser_download_url": "https://example.test/peridot.tar.gz"}
        ]
    });

    assert_eq!(
        release_asset_url(&release, "peridot-x86_64-unknown-linux-gnu.tar.gz"),
        Some("https://example.test/peridot.tar.gz".to_string())
    );
    assert_eq!(release_asset_url(&release, "missing.tar.gz"), None);
}

#[test]
fn reads_release_checksum_for_asset() {
    let checksums = "\
1111111111111111111111111111111111111111111111111111111111111111  peridot-aarch64-apple-darwin.tar.gz\n\
2222222222222222222222222222222222222222222222222222222222222222  *peridot-x86_64-unknown-linux-gnu.tar.gz\n";

    assert_eq!(
        checksum_for_asset(checksums, "peridot-x86_64-unknown-linux-gnu.tar.gz").unwrap(),
        "2222222222222222222222222222222222222222222222222222222222222222"
    );
    assert!(checksum_for_asset(checksums, "missing.tar.gz").is_err());
}

#[test]
fn verifies_sha256_digest() {
    let expected = sha256_hex(b"peridot");

    verify_sha256(b"peridot", &expected, "peridot-test.tar.gz").unwrap();
    assert!(verify_sha256(b"other", &expected, "peridot-test.tar.gz").is_err());
}

#[test]
fn parses_update_intervals() {
    assert_eq!(parse_update_interval("30m"), Duration::from_secs(30 * 60));
    assert_eq!(
        parse_update_interval("12h"),
        Duration::from_secs(12 * 60 * 60)
    );
    assert_eq!(
        parse_update_interval("7d"),
        Duration::from_secs(7 * 24 * 60 * 60)
    );
    assert_eq!(
        parse_update_interval("bad"),
        Duration::from_secs(24 * 60 * 60)
    );
}

#[cfg(unix)]
#[test]
fn ensure_peri_alias_creates_unix_symlink() {
    let root = std::env::temp_dir().join(format!("peridot-cli-alias-{}", std::process::id()));
    fs::create_dir_all(&root).unwrap();
    let binary = root.join("peridot");
    fs::write(&binary, "binary").unwrap();

    let alias = ensure_peri_alias(&binary, "x86_64-unknown-linux-gnu").unwrap();

    assert_eq!(alias, root.join("peri"));
    assert!(
        fs::symlink_metadata(&alias)
            .unwrap()
            .file_type()
            .is_symlink()
    );
    fs::remove_dir_all(root).unwrap();
}

#[cfg(unix)]
#[test]
fn install_executable_update_replaces_with_rename() {
    let root = std::env::temp_dir().join(format!("peridot-cli-update-{}", std::process::id()));
    fs::create_dir_all(&root).unwrap();
    let current = root.join("peridot");
    let extracted = root.join("new-peridot");
    fs::write(&current, "old").unwrap();
    fs::write(&extracted, "new").unwrap();

    install_executable_update(&extracted, &current).unwrap();

    assert_eq!(fs::read_to_string(&current).unwrap(), "new");
    assert!(fs::read_dir(&root).unwrap().all(|entry| {
        !entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .contains(".new-")
    }));
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn current_target_has_release_asset_name_shape() {
    let target = current_release_target().unwrap();

    assert!(target.contains('-'));
    assert!(format!("peridot-{target}.tar.gz").starts_with("peridot-"));
}
