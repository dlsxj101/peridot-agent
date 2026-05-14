//! Scriptable CLI subcommand handlers.

use std::collections::HashMap;
use std::fs;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use clap::{Subcommand, ValueEnum};
use peridot_common::{McpServerConfig, McpTransport, PeridotConfig};
use peridot_mcp::McpClient;
use peridot_memory::{MemoryStore, SessionSummary};
use peridot_project::{ProjectProfile, ProjectScanner};
use peridot_verify::VerifyPipeline;
use serde_json::Value;
use sha2::{Digest, Sha256};

/// Scriptable output format.
#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
pub(crate) enum OutputFormat {
    /// Human-readable text.
    Text,
    /// JSON.
    Json,
}

/// Auth providers supported by `peridot login`.
#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
pub(crate) enum AuthProvider {
    /// Anthropic Claude API key.
    ClaudeApi,
    /// OpenAI API key.
    OpenaiApi,
    /// OpenAI OAuth PKCE flow.
    OpenaiOauth,
}

impl AuthProvider {
    fn id(self) -> &'static str {
        match self {
            Self::ClaudeApi => "claude-api",
            Self::OpenaiApi => "openai-api",
            Self::OpenaiOauth => "openai-oauth",
        }
    }

    fn api_key_env_var(self) -> Option<&'static str> {
        match self {
            Self::ClaudeApi => Some("ANTHROPIC_API_KEY"),
            Self::OpenaiApi => Some("OPENAI_API_KEY"),
            Self::OpenaiOauth => None,
        }
    }
}

/// Config subcommands.
#[derive(Debug, Subcommand)]
pub(crate) enum ConfigCommand {
    /// Initialize project-local Peridot config.
    Init,
    /// Print the effective config.
    Show,
    /// Open project-local config in $EDITOR.
    Edit,
}

/// Session subcommands.
#[derive(Debug, Subcommand)]
pub(crate) enum SessionCommand {
    /// List saved sessions.
    List,
    /// Print a resume prompt for one saved session.
    Resume {
        /// Session id.
        id: String,
    },
    /// Save a session summary.
    Save {
        /// Session id.
        id: String,
        /// Summary text.
        summary: Vec<String>,
    },
    /// Show one session summary.
    Show {
        /// Session id.
        id: String,
    },
    /// Delete one session summary.
    Delete {
        /// Session id.
        id: String,
    },
}

/// AGENTS.md subcommands.
#[derive(Debug, Subcommand)]
pub(crate) enum AgentsCommand {
    /// Create an AGENTS.md draft when one does not exist.
    Init,
    /// Print the current AGENTS.md-compatible instruction file.
    Show,
}

/// Skill library subcommands.
#[derive(Debug, Subcommand)]
pub(crate) enum SkillCommand {
    /// List local and global skills.
    List,
    /// Install a project-local community skill from a URL or file path.
    Install {
        /// HTTP(S) URL or local Markdown file path.
        source: String,
    },
    /// Print a skill by name.
    Show {
        /// Skill name or file stem.
        name: String,
    },
    /// Remove a project-local skill.
    Remove {
        /// Skill name or file stem.
        name: String,
    },
}

/// MCP server subcommands.
#[derive(Debug, Subcommand)]
pub(crate) enum McpCommand {
    /// List configured MCP servers.
    List,
    /// Validate one MCP server definition.
    Test {
        /// MCP server name.
        name: String,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct SkillEntry {
    name: String,
    scope: &'static str,
    path: PathBuf,
}

pub(crate) fn load_effective_config(
    project_root: &Path,
    explicit_config: Option<&Path>,
) -> Result<PeridotConfig> {
    load_effective_config_inner(project_root, explicit_config, true, true)
}

fn load_effective_config_inner(
    project_root: &Path,
    explicit_config: Option<&Path>,
    include_global: bool,
    include_env: bool,
) -> Result<PeridotConfig> {
    let mut config = PeridotConfig::default();
    if include_global && let Some(global_config) = global_config_path() {
        merge_config_file(&global_config, false, &mut config)?;
    }
    apply_agents_preferences(project_root, &mut config)?;

    let project_config;
    let (path, required) = match explicit_config {
        Some(path) => (path, true),
        None => {
            project_config = project_root.join(".peridot/config.toml");
            (project_config.as_path(), false)
        }
    };
    merge_config_file(path, required, &mut config)?;
    if include_env {
        apply_env_config(&mut config)?;
    }
    Ok(config)
}

fn merge_config_file(path: &Path, required: bool, config: &mut PeridotConfig) -> Result<()> {
    if !path.exists() {
        if required {
            anyhow::bail!("config file not found: {}", path.display());
        }
        return Ok(());
    }
    let content =
        fs::read_to_string(path).with_context(|| format!("failed to read {}", path.display()))?;
    let project_config = toml::from_str::<PeridotConfig>(&content)
        .with_context(|| format!("failed to parse {}", path.display()))?;
    let raw_config = toml::from_str::<toml::Value>(&content)
        .with_context(|| format!("failed to parse {}", path.display()))?;
    merge_project_config(&raw_config, project_config, config);
    Ok(())
}

fn global_config_path() -> Option<PathBuf> {
    if let Some(home) = std::env::var_os("PERIDOT_HOME") {
        return Some(PathBuf::from(home).join("config.toml"));
    }
    std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".peridot/config.toml"))
}

fn apply_agents_preferences(project_root: &Path, config: &mut PeridotConfig) -> Result<()> {
    let profile = ProjectScanner::new().scan(project_root)?;
    let preferences = profile.preferences;
    if let Some(mode) = preferences.default_mode {
        config.defaults.mode = mode;
    }
    if let Some(permission) = preferences.default_permission {
        config.defaults.permission = permission;
    }
    if let Some(ask_before_install) = preferences.ask_before_install {
        config.security.ask_before_install = ask_before_install;
    }
    if let Some(ask_before_delete) = preferences.ask_before_delete {
        config.security.ask_before_delete = ask_before_delete;
    }
    Ok(())
}

fn apply_env_config(config: &mut PeridotConfig) -> Result<()> {
    if let Ok(model) = std::env::var("PERIDOT_MODEL")
        && !model.trim().is_empty()
    {
        config.models.main = model;
    }
    if let Ok(mode) = std::env::var("PERIDOT_MODE") {
        config.defaults.mode = parse_env_mode("PERIDOT_MODE", &mode)?;
    }
    if let Ok(permission) = std::env::var("PERIDOT_PERMISSION") {
        config.defaults.permission = parse_env_permission("PERIDOT_PERMISSION", &permission)?;
    }
    if let Ok(budget) = std::env::var("PERIDOT_BUDGET") {
        config.defaults.budget_usd = budget.parse().with_context(|| {
            format!("failed to parse PERIDOT_BUDGET as a decimal number: {budget}")
        })?;
    }
    if let Ok(max_turns) = std::env::var("PERIDOT_MAX_TURNS") {
        config.defaults.max_turns = max_turns.parse().with_context(|| {
            format!("failed to parse PERIDOT_MAX_TURNS as an integer: {max_turns}")
        })?;
    }
    Ok(())
}

fn parse_env_mode(name: &str, value: &str) -> Result<peridot_common::ExecutionMode> {
    match value.trim().to_ascii_lowercase().as_str() {
        "plan" => Ok(peridot_common::ExecutionMode::Plan),
        "execute" => Ok(peridot_common::ExecutionMode::Execute),
        "goal" => Ok(peridot_common::ExecutionMode::Goal),
        _ => anyhow::bail!("{name} must be one of plan, execute, or goal"),
    }
}

fn parse_env_permission(name: &str, value: &str) -> Result<peridot_common::PermissionMode> {
    match value.trim().to_ascii_lowercase().as_str() {
        "safe" => Ok(peridot_common::PermissionMode::Safe),
        "auto" => Ok(peridot_common::PermissionMode::Auto),
        "yolo" => Ok(peridot_common::PermissionMode::Yolo),
        _ => anyhow::bail!("{name} must be one of safe, auto, or yolo"),
    }
}

fn merge_project_config(
    raw_config: &toml::Value,
    project_config: PeridotConfig,
    config: &mut PeridotConfig,
) {
    if raw_config.get("auth").is_some() {
        config.auth = project_config.auth;
    }
    if raw_config.get("models").is_some() {
        config.models = project_config.models;
    }
    if raw_config.get("api").is_some() {
        config.api = project_config.api;
    }
    if raw_config.get("context").is_some() {
        config.context = project_config.context;
    }
    if raw_config.get("mcp").is_some() {
        config.mcp = project_config.mcp;
    }
    if raw_config.get("hooks").is_some() {
        config.hooks = project_config.hooks;
    }
    if let Some(defaults) = raw_config.get("defaults").and_then(toml::Value::as_table) {
        if defaults.contains_key("mode") {
            config.defaults.mode = project_config.defaults.mode;
        }
        if defaults.contains_key("permission") {
            config.defaults.permission = project_config.defaults.permission;
        }
        if defaults.contains_key("max_turns") {
            config.defaults.max_turns = project_config.defaults.max_turns;
        }
        if defaults.contains_key("budget_usd") {
            config.defaults.budget_usd = project_config.defaults.budget_usd;
        }
        if defaults.contains_key("budget_warning_pct") {
            config.defaults.budget_warning_pct = project_config.defaults.budget_warning_pct;
        }
    }
    if let Some(security) = raw_config.get("security").and_then(toml::Value::as_table) {
        if security.contains_key("sandbox") {
            config.security.sandbox = project_config.security.sandbox;
        }
        if security.contains_key("docker_image") {
            config.security.docker_image = project_config.security.docker_image;
        }
        if security.contains_key("docker_network") {
            config.security.docker_network = project_config.security.docker_network;
        }
        if security.contains_key("ask_before_install") {
            config.security.ask_before_install = project_config.security.ask_before_install;
        }
        if security.contains_key("ask_before_delete") {
            config.security.ask_before_delete = project_config.security.ask_before_delete;
        }
    }
}

pub(crate) fn run_config_command(
    command: &ConfigCommand,
    config: &PeridotConfig,
    project_root: &Path,
    output: OutputFormat,
) -> Result<()> {
    match command {
        ConfigCommand::Init => init_project_config(project_root, output),
        ConfigCommand::Show => print_config(config, output),
        ConfigCommand::Edit => edit_project_config(project_root),
    }
}

pub(crate) fn run_session_command(
    command: &SessionCommand,
    project_root: &Path,
    output: OutputFormat,
) -> Result<()> {
    let store = memory_store(project_root);
    match command {
        SessionCommand::List => {
            let sessions = store.list_sessions()?;
            match output {
                OutputFormat::Json => println!("{}", serde_json::to_string_pretty(&sessions)?),
                OutputFormat::Text => {
                    for session in sessions {
                        println!("{}\t{}", session.id, session.summary);
                    }
                }
            }
        }
        SessionCommand::Resume { id } => {
            let session = store
                .get_session(id)?
                .with_context(|| format!("session not found: {id}"))?;
            let resume_task = format!(
                "Resume session {} from this summary: {}",
                session.id, session.summary
            );
            match output {
                OutputFormat::Json => println!(
                    "{}",
                    serde_json::to_string_pretty(&serde_json::json!({
                        "id": session.id,
                        "summary": session.summary,
                        "resume_task": resume_task
                    }))?
                ),
                OutputFormat::Text => println!("{resume_task}"),
            }
        }
        SessionCommand::Save { id, summary } => {
            let session = SessionSummary {
                id: id.clone(),
                summary: summary.join(" "),
            };
            store.save_session(&session)?;
            print_json_or_text_result(
                serde_json::json!({"saved": true, "id": id}),
                format!("saved session {id}"),
                output,
            )?;
        }
        SessionCommand::Show { id } => {
            let session = store.get_session(id)?;
            match output {
                OutputFormat::Json => println!("{}", serde_json::to_string_pretty(&session)?),
                OutputFormat::Text => match session {
                    Some(session) => println!("{}\t{}", session.id, session.summary),
                    None => println!("session not found: {id}"),
                },
            }
        }
        SessionCommand::Delete { id } => {
            let deleted = store.delete_session(id)?;
            print_json_or_text_result(
                serde_json::json!({"deleted": deleted, "id": id}),
                format!("deleted session {id}: {deleted}"),
                output,
            )?;
        }
    }
    Ok(())
}

pub(crate) fn run_agents_command(
    command: &AgentsCommand,
    project_root: &Path,
    output: OutputFormat,
) -> Result<()> {
    match command {
        AgentsCommand::Init => {
            let path = project_root.join("AGENTS.md");
            let created = if path.exists() {
                false
            } else {
                let profile = ProjectScanner::new().scan(project_root)?;
                fs::write(&path, agents_draft(&profile))?;
                true
            };
            print_json_or_text_result(
                serde_json::json!({"path": path, "created": created}),
                format!("AGENTS.md created={created}"),
                output,
            )
        }
        AgentsCommand::Show => {
            let path = find_agents_instruction(project_root)
                .with_context(|| "no AGENTS.md-compatible instruction file found")?;
            let content = fs::read_to_string(&path)?;
            match output {
                OutputFormat::Json => println!(
                    "{}",
                    serde_json::to_string_pretty(&serde_json::json!({
                        "path": path,
                        "content": content
                    }))?
                ),
                OutputFormat::Text => print!("{content}"),
            }
            Ok(())
        }
    }
}

pub(crate) async fn run_skill_command(
    command: &SkillCommand,
    project_root: &Path,
    output: OutputFormat,
) -> Result<()> {
    match command {
        SkillCommand::List => {
            let skills = collect_skills(project_root)?;
            match output {
                OutputFormat::Json => println!(
                    "{}",
                    serde_json::to_string_pretty(
                        &skills
                            .iter()
                            .map(skill_json)
                            .collect::<Vec<serde_json::Value>>()
                    )?
                ),
                OutputFormat::Text => {
                    for skill in skills {
                        println!("{}\t{}\t{}", skill.name, skill.scope, skill.path.display());
                    }
                }
            }
        }
        SkillCommand::Install { source } => {
            let installed = install_skill(project_root, source).await?;
            print_json_or_text_result(
                serde_json::json!({
                    "installed": true,
                    "name": installed.name,
                    "path": installed.path
                }),
                format!(
                    "installed skill {} to {}",
                    installed.name,
                    installed.path.display()
                ),
                output,
            )?;
        }
        SkillCommand::Show { name } => {
            let skill = find_skill(project_root, name)?
                .with_context(|| format!("skill not found: {name}"))?;
            let content = fs::read_to_string(&skill.path)
                .with_context(|| format!("failed to read {}", skill.path.display()))?;
            match output {
                OutputFormat::Json => println!(
                    "{}",
                    serde_json::to_string_pretty(&serde_json::json!({
                        "name": skill.name,
                        "scope": skill.scope,
                        "path": skill.path,
                        "content": content
                    }))?
                ),
                OutputFormat::Text => print!("{content}"),
            }
        }
        SkillCommand::Remove { name } => {
            let skill = find_skill(project_root, name)?
                .with_context(|| format!("skill not found: {name}"))?;
            let project_skills = project_root.join(".peridot/skills");
            if !skill.path.starts_with(&project_skills) {
                anyhow::bail!(
                    "refusing to remove non-project skill {} ({})",
                    skill.name,
                    skill.path.display()
                );
            }
            fs::remove_file(&skill.path)
                .with_context(|| format!("failed to remove {}", skill.path.display()))?;
            print_json_or_text_result(
                serde_json::json!({
                    "removed": true,
                    "name": skill.name,
                    "path": skill.path
                }),
                format!("removed skill {name}"),
                output,
            )?;
        }
    }
    Ok(())
}

async fn install_skill(project_root: &Path, source: &str) -> Result<SkillEntry> {
    let content = read_skill_source(source).await?;
    if content.trim().is_empty() {
        anyhow::bail!("skill source is empty: {source}");
    }
    let name = skill_name_from_source(source);
    let target_dir = project_root.join(".peridot/skills/community");
    fs::create_dir_all(&target_dir)?;
    let path = target_dir.join(format!("{name}.md"));
    fs::write(&path, content)?;
    Ok(SkillEntry {
        name,
        scope: "project-community",
        path,
    })
}

async fn read_skill_source(source: &str) -> Result<String> {
    if source.starts_with("https://") || source.starts_with("http://") {
        let response = reqwest::Client::new()
            .get(source)
            .header("user-agent", "peridot-agent")
            .send()
            .await
            .with_context(|| format!("failed to download skill {source}"))?;
        let status = response.status();
        let content = response.text().await?;
        if !status.is_success() {
            anyhow::bail!("skill download returned {status}: {content}");
        }
        Ok(content)
    } else {
        fs::read_to_string(source).with_context(|| format!("failed to read skill {source}"))
    }
}

fn skill_name_from_source(source: &str) -> String {
    let source = source.trim_end_matches('/');
    let last = source.rsplit('/').next().unwrap_or(source);
    let stem = last
        .strip_suffix(".md")
        .or_else(|| last.strip_suffix(".markdown"))
        .unwrap_or(last);
    sanitize_skill_name(stem)
}

fn sanitize_skill_name(name: &str) -> String {
    let sanitized = name
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || character == '-' || character == '_' {
                character.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect::<String>()
        .trim_matches('-')
        .to_string();
    if sanitized.is_empty() {
        "skill".to_string()
    } else {
        sanitized
    }
}

pub(crate) async fn run_mcp_command(
    command: &McpCommand,
    config: &PeridotConfig,
    output: OutputFormat,
) -> Result<()> {
    match command {
        McpCommand::List => match output {
            OutputFormat::Json => println!(
                "{}",
                serde_json::to_string_pretty(&config.mcp.iter().map(mcp_json).collect::<Vec<_>>())?
            ),
            OutputFormat::Text => {
                for server in &config.mcp {
                    println!(
                        "{}\t{}\t{}",
                        server.name,
                        server.transport,
                        mcp_target(server)
                    );
                }
            }
        },
        McpCommand::Test { name } => {
            let server = config
                .mcp
                .iter()
                .find(|server| server.name == *name)
                .with_context(|| format!("MCP server not found: {name}"))?;
            validate_mcp_server(server)?;
            let tools = McpClient::new(server.clone()).list_tools().await?;
            print_json_or_text_result(
                serde_json::json!({
                    "name": server.name,
                    "transport": server.transport,
                    "target": mcp_target(server),
                    "configured": true,
                    "tools": tools
                }),
                format!(
                    "MCP server {} is configured for {} ({}) with {} tools",
                    server.name,
                    server.transport,
                    mcp_target(server),
                    tools.len()
                ),
                output,
            )?;
        }
    }
    Ok(())
}

pub(crate) fn print_scan(profile: &ProjectProfile, output: OutputFormat) -> Result<()> {
    match output {
        OutputFormat::Json => println!("{}", serde_json::to_string_pretty(profile)?),
        OutputFormat::Text => {
            println!("Project: {}", profile.name);
            println!("Root: {}", profile.root.display());
            println!("Build system: {:?}", profile.build_system);
            if !profile.languages.is_empty() {
                let languages = profile
                    .languages
                    .iter()
                    .map(|language| language.name.as_str())
                    .collect::<Vec<_>>()
                    .join(", ");
                println!("Languages: {languages}");
            }
            if let Some(build) = &profile.commands.build {
                println!("Build: {build}");
            }
            if let Some(test) = &profile.commands.test {
                println!("Test: {test}");
            }
            println!("AGENTS.md: {}", profile.has_agents_md);
        }
    }
    Ok(())
}

pub(crate) fn run_verify_command(project_root: &Path, output: OutputFormat) -> Result<()> {
    let profile = ProjectScanner::new().scan(project_root)?;
    let report = VerifyPipeline::new(profile).run_all()?;
    match output {
        OutputFormat::Json => println!("{}", serde_json::to_string_pretty(&report)?),
        OutputFormat::Text => {
            for stage in &report.stages {
                let marker = if stage.passed { "PASS" } else { "FAIL" };
                println!("{marker}\t{:?}\t{}", stage.stage, stage.summary);
            }
        }
    }
    Ok(())
}

pub(crate) fn run_setup_command(project_root: &Path, output: OutputFormat) -> Result<()> {
    let config_result = init_project_config_value(project_root)?;
    let agents_path = project_root.join("AGENTS.md");
    let created_agents = if find_agents_instruction(project_root).is_none() {
        let profile = ProjectScanner::new().scan(project_root)?;
        fs::write(&agents_path, agents_draft(&profile))?;
        true
    } else {
        false
    };
    print_json_or_text_result(
        serde_json::json!({
            "config_path": config_result.config_path,
            "created_config": config_result.created_config,
            "updated_gitignore": config_result.updated_gitignore,
            "agents_path": agents_path,
            "created_agents": created_agents
        }),
        format!(
            "setup complete (created_config={}, updated_gitignore={}, created_agents={})",
            config_result.created_config, config_result.updated_gitignore, created_agents
        ),
        output,
    )
}

pub(crate) async fn run_login_command(provider: AuthProvider, output: OutputFormat) -> Result<()> {
    if provider == AuthProvider::OpenaiOauth {
        return run_openai_oauth_login(output).await;
    }
    let env_var = provider
        .api_key_env_var()
        .with_context(|| format!("{} does not use API-key login", provider.id()))?;
    let api_key =
        std::env::var(env_var).with_context(|| format!("{env_var} is required for login"))?;
    let path = auth_file(provider)?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let content = serde_json::to_string_pretty(&serde_json::json!({
        "provider": provider.id(),
        "api_key": api_key
    }))?;
    fs::write(&path, content)?;
    set_private_permissions(&path)?;
    print_json_or_text_result(
        serde_json::json!({"provider": provider.id(), "path": path, "stored": true}),
        format!("stored credentials for {}", provider.id()),
        output,
    )
}

pub(crate) fn run_logout_command(provider: AuthProvider, output: OutputFormat) -> Result<()> {
    let path = auth_file(provider)?;
    let removed = if path.exists() {
        fs::remove_file(&path)?;
        true
    } else {
        false
    };
    print_json_or_text_result(
        serde_json::json!({"provider": provider.id(), "path": path, "removed": removed}),
        format!("removed credentials for {}: {removed}", provider.id()),
        output,
    )
}

pub(crate) fn read_stored_api_key(provider: AuthProvider) -> Result<Option<String>> {
    let path = auth_file(provider)?;
    if !path.exists() {
        return Ok(None);
    }
    let value = serde_json::from_str::<Value>(&fs::read_to_string(path)?)?;
    Ok(value
        .get("api_key")
        .and_then(Value::as_str)
        .map(str::to_string))
}

pub(crate) fn read_stored_openai_oauth_access_token() -> Result<Option<String>> {
    let path = auth_file(AuthProvider::OpenaiOauth)?;
    if !path.exists() {
        return Ok(None);
    }
    let value = serde_json::from_str::<Value>(&fs::read_to_string(path)?)?;
    Ok(value
        .get("access_token")
        .and_then(Value::as_str)
        .map(str::to_string))
}

async fn run_openai_oauth_login(output: OutputFormat) -> Result<()> {
    let client_id = std::env::var("OPENAI_OAUTH_CLIENT_ID")
        .with_context(|| "OPENAI_OAUTH_CLIENT_ID is required for openai-oauth login")?;
    let port = std::env::var("PERIDOT_OAUTH_PORT")
        .ok()
        .and_then(|value| value.parse::<u16>().ok())
        .unwrap_or(14552);
    let scope = std::env::var("OPENAI_OAUTH_SCOPE")
        .unwrap_or_else(|_| "openid profile email offline_access".to_string());
    let redirect_uri = format!("http://127.0.0.1:{port}/callback");
    let state = random_urlsafe(32);
    let code_verifier = random_urlsafe(64);
    let code_challenge = pkce_challenge(&code_verifier);
    let auth_url =
        openai_oauth_authorize_url(&client_id, &redirect_uri, &scope, &state, &code_challenge);

    if output == OutputFormat::Text {
        println!("Open this URL to authorize Peridot:\n{auth_url}");
        if open_browser(&auth_url) {
            println!("Opened browser; waiting for OAuth callback on {redirect_uri}");
        } else {
            println!("Could not open a browser automatically; paste the URL into your browser.");
        }
    }

    let code = wait_for_oauth_code(port, &state)?;
    let mut token = exchange_openai_oauth_code(&client_id, &redirect_uri, &code_verifier, &code)
        .await
        .with_context(|| "failed to exchange OpenAI OAuth authorization code")?;
    if let Some(object) = token.as_object_mut() {
        object.insert(
            "provider".to_string(),
            Value::String(AuthProvider::OpenaiOauth.id().to_string()),
        );
        object.insert("client_id".to_string(), Value::String(client_id));
        object.insert("redirect_uri".to_string(), Value::String(redirect_uri));
        object.insert(
            "obtained_at_unix".to_string(),
            Value::Number(serde_json::Number::from(
                SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs(),
            )),
        );
    }

    let path = auth_file(AuthProvider::OpenaiOauth)?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(&path, serde_json::to_string_pretty(&token)?)?;
    set_private_permissions(&path)?;
    print_json_or_text_result(
        serde_json::json!({
            "provider": AuthProvider::OpenaiOauth.id(),
            "path": path,
            "stored": true,
            "token_type": token.get("token_type").and_then(Value::as_str)
        }),
        format!("stored credentials for {}", AuthProvider::OpenaiOauth.id()),
        output,
    )
}

fn openai_oauth_authorize_url(
    client_id: &str,
    redirect_uri: &str,
    scope: &str,
    state: &str,
    code_challenge: &str,
) -> String {
    format!(
        "https://auth.openai.com/oauth/authorize?response_type=code&client_id={}&redirect_uri={}&scope={}&state={}&code_challenge={}&code_challenge_method=S256",
        url_encode(client_id),
        url_encode(redirect_uri),
        url_encode(scope),
        url_encode(state),
        url_encode(code_challenge)
    )
}

async fn exchange_openai_oauth_code(
    client_id: &str,
    redirect_uri: &str,
    code_verifier: &str,
    code: &str,
) -> Result<Value> {
    let response = reqwest::Client::new()
        .post("https://auth.openai.com/oauth/token")
        .form(&[
            ("grant_type", "authorization_code"),
            ("client_id", client_id),
            ("redirect_uri", redirect_uri),
            ("code_verifier", code_verifier),
            ("code", code),
        ])
        .send()
        .await?;
    let status = response.status();
    let body = response.text().await?;
    if !status.is_success() {
        anyhow::bail!("OpenAI OAuth token exchange returned {status}: {body}");
    }
    Ok(serde_json::from_str(&body)?)
}

fn wait_for_oauth_code(port: u16, expected_state: &str) -> Result<String> {
    let listener = TcpListener::bind(("127.0.0.1", port))
        .with_context(|| format!("failed to bind local OAuth callback port {port}"))?;
    listener.set_nonblocking(true)?;
    let deadline = SystemTime::now() + Duration::from_secs(300);
    loop {
        match listener.accept() {
            Ok((mut stream, _)) => {
                stream.set_read_timeout(Some(Duration::from_secs(5)))?;
                let mut buffer = [0_u8; 8192];
                let size = stream.read(&mut buffer)?;
                let request = String::from_utf8_lossy(&buffer[..size]);
                let result = parse_oauth_callback(&request, expected_state);
                let body = if result.is_ok() {
                    "Peridot login complete. You can close this window."
                } else {
                    "Peridot login failed. Return to the terminal for details."
                };
                let response = format!(
                    "HTTP/1.1 200 OK\r\ncontent-type: text/plain; charset=utf-8\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
                stream.write_all(response.as_bytes())?;
                return result;
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                if SystemTime::now() >= deadline {
                    anyhow::bail!("timed out waiting for OpenAI OAuth callback");
                }
                std::thread::sleep(Duration::from_millis(100));
            }
            Err(error) => return Err(error).with_context(|| "failed to accept OAuth callback"),
        }
    }
}

fn parse_oauth_callback(request: &str, expected_state: &str) -> Result<String> {
    let target = request
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .with_context(|| "invalid OAuth callback request")?;
    let query = target
        .split_once('?')
        .map(|(_, query)| query)
        .unwrap_or_default();
    let params = parse_query(query)?;
    if let Some(error) = params.get("error") {
        anyhow::bail!("OpenAI OAuth error: {error}");
    }
    let state = params
        .get("state")
        .with_context(|| "OpenAI OAuth callback omitted state")?;
    if state != expected_state {
        anyhow::bail!("OpenAI OAuth state mismatch");
    }
    params
        .get("code")
        .cloned()
        .with_context(|| "OpenAI OAuth callback omitted code")
}

fn parse_query(query: &str) -> Result<HashMap<String, String>> {
    let mut params = HashMap::new();
    for pair in query.split('&').filter(|pair| !pair.is_empty()) {
        let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
        params.insert(percent_decode(key)?, percent_decode(value)?);
    }
    Ok(params)
}

fn pkce_challenge(verifier: &str) -> String {
    URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()))
}

fn random_urlsafe(bytes: usize) -> String {
    let mut random = vec![0_u8; bytes];
    for chunk in random.chunks_mut(32) {
        let sample: [u8; 32] = rand::random();
        let len = chunk.len();
        chunk.copy_from_slice(&sample[..len]);
    }
    URL_SAFE_NO_PAD.encode(random)
}

fn open_browser(url: &str) -> bool {
    let mut command = if cfg!(target_os = "macos") {
        let mut command = Command::new("open");
        command.arg(url);
        command
    } else if cfg!(target_os = "windows") {
        let mut command = Command::new("cmd");
        command.args(["/C", "start", "", url]);
        command
    } else {
        let mut command = Command::new("xdg-open");
        command.arg(url);
        command
    };
    command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .is_ok()
}

fn url_encode(value: &str) -> String {
    let mut encoded = String::new();
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                encoded.push(byte as char);
            }
            _ => encoded.push_str(&format!("%{byte:02X}")),
        }
    }
    encoded
}

fn percent_decode(value: &str) -> Result<String> {
    let mut bytes = Vec::with_capacity(value.len());
    let mut iter = value.as_bytes().iter().copied();
    while let Some(byte) = iter.next() {
        match byte {
            b'+' => bytes.push(b' '),
            b'%' => {
                let high = iter.next().with_context(|| "incomplete percent escape")?;
                let low = iter.next().with_context(|| "incomplete percent escape")?;
                let hex = [high, low];
                let decoded = u8::from_str_radix(std::str::from_utf8(&hex)?, 16)
                    .with_context(|| "invalid percent escape")?;
                bytes.push(decoded);
            }
            _ => bytes.push(byte),
        }
    }
    Ok(String::from_utf8(bytes)?)
}

pub(crate) async fn run_update_command(check: bool, output: OutputFormat) -> Result<()> {
    let current = env!("CARGO_PKG_VERSION");
    let repo = std::env::var("PERIDOT_UPDATE_REPO")
        .unwrap_or_else(|_| env!("CARGO_PKG_REPOSITORY").to_string());
    let Some((owner, name)) = github_owner_repo(&repo) else {
        anyhow::bail!("repository is not a GitHub URL: {repo}");
    };
    let url = format!("https://api.github.com/repos/{owner}/{name}/releases/latest");
    let response = reqwest::Client::new()
        .get(&url)
        .header("user-agent", "peridot-agent")
        .send()
        .await
        .with_context(|| format!("failed to query {url}"))?;
    let status = response.status();
    let body = response.text().await?;
    if !status.is_success() {
        anyhow::bail!("GitHub latest release query returned {status}: {body}");
    }
    let value = serde_json::from_str::<Value>(&body)?;
    let latest = value
        .get("tag_name")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim_start_matches('v')
        .to_string();
    let html_url = value
        .get("html_url")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let update_available = !latest.is_empty() && latest != current;
    let installed_path = if !check && update_available {
        Some(install_update(&value).await?)
    } else {
        None
    };
    print_json_or_text_result(
        serde_json::json!({
            "current": current,
            "latest": latest,
            "update_available": update_available,
            "release_url": html_url,
            "checked_only": check,
            "installed_path": installed_path
        }),
        if let Some(path) = installed_path {
            format!(
                "Updated Peridot from {current} to {latest} at {}",
                path.display()
            )
        } else if update_available {
            format!("Peridot {latest} is available (current {current}): {html_url}")
        } else {
            format!("Peridot is up to date ({current})")
        },
        output,
    )
}

async fn install_update(release: &Value) -> Result<PathBuf> {
    let target = current_release_target()?;
    let asset_name = format!("peridot-{target}.tar.gz");
    let asset_url = release_asset_url(release, &asset_name)
        .with_context(|| format!("release asset not found: {asset_name}"))?;
    let temp_dir = std::env::temp_dir().join(format!(
        "peridot-update-{}-{}",
        std::process::id(),
        SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs()
    ));
    fs::create_dir_all(&temp_dir)?;
    let archive_path = temp_dir.join(&asset_name);
    let bytes = reqwest::Client::new()
        .get(&asset_url)
        .header("user-agent", "peridot-agent")
        .send()
        .await
        .with_context(|| format!("failed to download {asset_url}"))?
        .error_for_status()
        .with_context(|| format!("failed to download {asset_url}"))?
        .bytes()
        .await?;
    fs::write(&archive_path, bytes)?;
    let status = Command::new("tar")
        .arg("-xzf")
        .arg(&archive_path)
        .arg("-C")
        .arg(&temp_dir)
        .status()
        .with_context(|| "failed to run tar for update archive")?;
    if !status.success() {
        anyhow::bail!("tar failed while extracting update archive: {status}");
    }
    let binary_name = if target.contains("windows") {
        "peridot.exe"
    } else {
        "peridot"
    };
    let extracted = temp_dir.join(binary_name);
    if !extracted.exists() {
        anyhow::bail!("update archive did not contain {binary_name}");
    }
    let current_exe = std::env::current_exe()?;
    let backup = current_exe.with_file_name(format!(
        "{}.old",
        current_exe
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("peridot")
    ));
    let _ = fs::copy(&current_exe, backup);
    fs::copy(&extracted, &current_exe)
        .with_context(|| format!("failed to replace {}", current_exe.display()))?;
    set_executable_permissions(&current_exe)?;
    let _ = fs::remove_dir_all(temp_dir);
    Ok(current_exe)
}

fn release_asset_url(release: &Value, asset_name: &str) -> Option<String> {
    release
        .get("assets")?
        .as_array()?
        .iter()
        .find(|asset| asset.get("name").and_then(Value::as_str) == Some(asset_name))?
        .get("browser_download_url")?
        .as_str()
        .map(str::to_string)
}

fn current_release_target() -> Result<&'static str> {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("linux", "x86_64") => Ok("x86_64-unknown-linux-gnu"),
        ("linux", "aarch64") => Ok("aarch64-unknown-linux-gnu"),
        ("macos", "x86_64") => Ok("x86_64-apple-darwin"),
        ("macos", "aarch64") => Ok("aarch64-apple-darwin"),
        ("windows", "x86_64") => Ok("x86_64-pc-windows-msvc"),
        ("windows", "aarch64") => Ok("aarch64-pc-windows-msvc"),
        (os, arch) => anyhow::bail!("unsupported update target: {os}-{arch}"),
    }
}

fn github_owner_repo(repository: &str) -> Option<(String, String)> {
    let trimmed = repository
        .trim()
        .trim_end_matches(".git")
        .trim_end_matches('/');
    let path = trimmed
        .strip_prefix("https://github.com/")
        .or_else(|| trimmed.strip_prefix("git@github.com:"))?;
    let mut parts = path.split('/');
    let owner = parts.next()?.to_string();
    let repo = parts.next()?.to_string();
    Some((owner, repo))
}

fn auth_file(provider: AuthProvider) -> Result<PathBuf> {
    let home = std::env::var_os("HOME").with_context(|| "HOME is required")?;
    Ok(PathBuf::from(home)
        .join(".peridot/auth")
        .join(format!("{}.json", provider.id())))
}

#[cfg(unix)]
fn set_private_permissions(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;

    fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    Ok(())
}

#[cfg(not(unix))]
fn set_private_permissions(_path: &Path) -> Result<()> {
    Ok(())
}

#[cfg(unix)]
fn set_executable_permissions(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;

    fs::set_permissions(path, fs::Permissions::from_mode(0o755))?;
    Ok(())
}

#[cfg(not(unix))]
fn set_executable_permissions(_path: &Path) -> Result<()> {
    Ok(())
}

fn init_project_config(project_root: &Path, output: OutputFormat) -> Result<()> {
    let result = init_project_config_value(project_root)?;
    print_json_or_text_result(
        serde_json::json!({
            "config_path": result.config_path,
            "created_config": result.created_config,
            "updated_gitignore": result.updated_gitignore
        }),
        format!(
            "initialized {} (created_config={}, updated_gitignore={})",
            result.peridot_dir.display(),
            result.created_config,
            result.updated_gitignore
        ),
        output,
    )
}

fn edit_project_config(project_root: &Path) -> Result<()> {
    let result = init_project_config_value(project_root)?;
    let editor = std::env::var("EDITOR").unwrap_or_else(|_| "vi".to_string());
    let status = Command::new(&editor)
        .arg(&result.config_path)
        .status()
        .with_context(|| format!("failed to launch editor `{editor}`"))?;
    if !status.success() {
        anyhow::bail!("editor `{editor}` exited with {status}");
    }
    Ok(())
}

struct ConfigInitResult {
    peridot_dir: PathBuf,
    config_path: PathBuf,
    created_config: bool,
    updated_gitignore: bool,
}

fn init_project_config_value(project_root: &Path) -> Result<ConfigInitResult> {
    let peridot_dir = project_root.join(".peridot");
    fs::create_dir_all(peridot_dir.join("hooks"))?;
    fs::create_dir_all(peridot_dir.join("skills"))?;
    let config_path = peridot_dir.join("config.toml");
    let created_config = if config_path.exists() {
        false
    } else {
        let config = toml::to_string_pretty(&PeridotConfig::default())?;
        fs::write(&config_path, config)?;
        true
    };
    let gitignore_path = project_root.join(".gitignore");
    let managed_entries = [
        ".peridot/memory.db",
        ".peridot/mem/",
        ".peridot/sessions/",
        ".peridot/skills/auto/",
        ".peridot/logs/",
    ];
    let mut gitignore = fs::read_to_string(&gitignore_path).unwrap_or_default();
    let mut changed_gitignore = false;
    for entry in managed_entries {
        if !gitignore.lines().any(|line| line.trim() == entry) {
            if !gitignore.ends_with('\n') && !gitignore.is_empty() {
                gitignore.push('\n');
            }
            gitignore.push_str(entry);
            gitignore.push('\n');
            changed_gitignore = true;
        }
    }
    if changed_gitignore {
        fs::write(&gitignore_path, gitignore)?;
    }
    Ok(ConfigInitResult {
        peridot_dir,
        config_path,
        created_config,
        updated_gitignore: changed_gitignore,
    })
}

fn memory_store(project_root: &Path) -> MemoryStore {
    MemoryStore::new(project_root.join(".peridot/memory.db"))
}

fn collect_skills(project_root: &Path) -> Result<Vec<SkillEntry>> {
    let mut skills = Vec::new();
    collect_skill_dir(
        &project_root.join(".peridot/skills"),
        "project",
        false,
        &mut skills,
    )?;
    collect_skill_dir(
        &project_root.join(".peridot/skills/community"),
        "project-community",
        true,
        &mut skills,
    )?;
    collect_skill_dir(
        &project_root.join(".peridot/skills/auto"),
        "project-auto",
        true,
        &mut skills,
    )?;
    if let Some(home) = std::env::var_os("HOME") {
        let global = PathBuf::from(home).join(".peridot/skills");
        collect_skill_dir(&global, "global", false, &mut skills)?;
        collect_skill_dir(&global.join("community"), "community", true, &mut skills)?;
    }
    skills.sort_by(|left, right| {
        left.name
            .cmp(&right.name)
            .then_with(|| left.scope.cmp(right.scope))
            .then_with(|| left.path.cmp(&right.path))
    });
    Ok(skills)
}

fn collect_skill_dir(
    root: &Path,
    scope: &'static str,
    recursive: bool,
    skills: &mut Vec<SkillEntry>,
) -> Result<()> {
    if !root.exists() {
        return Ok(());
    }
    for entry in fs::read_dir(root).with_context(|| format!("failed to read {}", root.display()))? {
        let path = entry?.path();
        if path.is_dir() {
            if recursive {
                collect_skill_dir(&path, scope, recursive, skills)?;
            }
            continue;
        }
        if path.extension().and_then(|ext| ext.to_str()) != Some("md") {
            continue;
        }
        let Some(name) = path.file_stem().and_then(|stem| stem.to_str()) else {
            continue;
        };
        skills.push(SkillEntry {
            name: name.to_string(),
            scope,
            path,
        });
    }
    Ok(())
}

fn find_skill(project_root: &Path, name: &str) -> Result<Option<SkillEntry>> {
    Ok(collect_skills(project_root)?.into_iter().find(|skill| {
        skill.name == name || skill.path.file_stem().and_then(|stem| stem.to_str()) == Some(name)
    }))
}

fn skill_json(skill: &SkillEntry) -> serde_json::Value {
    serde_json::json!({
        "name": skill.name,
        "scope": skill.scope,
        "path": skill.path
    })
}

fn validate_mcp_server(server: &McpServerConfig) -> Result<()> {
    match server.transport {
        McpTransport::Stdio => {
            if server
                .command
                .as_deref()
                .unwrap_or_default()
                .trim()
                .is_empty()
            {
                anyhow::bail!("stdio MCP server {} is missing command", server.name);
            }
        }
        McpTransport::Http => {
            if server.url.as_deref().unwrap_or_default().trim().is_empty() {
                anyhow::bail!("http MCP server {} is missing url", server.name);
            }
        }
    }
    Ok(())
}

fn mcp_target(server: &McpServerConfig) -> String {
    match server.transport {
        McpTransport::Stdio => {
            let mut parts = Vec::new();
            if let Some(command) = &server.command {
                parts.push(command.clone());
            }
            parts.extend(server.args.iter().cloned());
            parts.join(" ")
        }
        McpTransport::Http => server.url.clone().unwrap_or_default(),
    }
}

fn mcp_json(server: &McpServerConfig) -> serde_json::Value {
    serde_json::json!({
        "name": server.name,
        "transport": server.transport,
        "target": mcp_target(server),
        "configured": validate_mcp_server(server).is_ok()
    })
}

fn find_agents_instruction(project_root: &Path) -> Option<PathBuf> {
    [
        ".peridot/AGENTS.md",
        "AGENTS.md",
        "CLAUDE.md",
        ".github/copilot-instructions.md",
    ]
    .into_iter()
    .map(|path| project_root.join(path))
    .find(|path| path.exists())
}

fn agents_draft(profile: &ProjectProfile) -> String {
    let build = profile.commands.build.as_deref().unwrap_or("");
    let test = profile.commands.test.as_deref().unwrap_or("");
    let lint = profile.commands.lint.as_deref().unwrap_or("");
    let format = profile.commands.format.as_deref().unwrap_or("");
    format!(
        "# Peridot Agent Instructions\n\n\
## project\n\
name: {}\n\
description: Generated Peridot project guidance draft.\n\n\
## commands\n\
build: {}\n\
test: {}\n\
lint: {}\n\
format: {}\n\n\
## style\n\
- Keep changes scoped and buildable.\n\
- Add or update tests for behavior changes.\n\n\
## boundaries\n\
- DO NOT modify generated files without explicit approval.\n\
- DO NOT commit secrets or local memory databases.\n\n\
## preferences\n\
default_mode: execute\n\
default_permission: auto\n",
        profile.name, build, test, lint, format
    )
}

fn print_config(config: &PeridotConfig, output: OutputFormat) -> Result<()> {
    match output {
        OutputFormat::Json => println!("{}", serde_json::to_string_pretty(config)?),
        OutputFormat::Text => {
            println!("auth.primary = {}", config.auth.primary);
            println!("models.main = {}", config.models.main);
            println!("defaults.mode = {}", config.defaults.mode);
            println!("defaults.permission = {}", config.defaults.permission);
            println!("defaults.max_turns = {}", config.defaults.max_turns);
            println!("defaults.budget_usd = {}", config.defaults.budget_usd);
            println!("security.sandbox = {}", config.security.sandbox);
            println!("security.docker_image = {}", config.security.docker_image);
            println!(
                "security.docker_network = {}",
                config.security.docker_network
            );
            println!(
                "security.ask_before_install = {}",
                config.security.ask_before_install
            );
            println!(
                "security.ask_before_delete = {}",
                config.security.ask_before_delete
            );
        }
    }
    Ok(())
}

fn print_json_or_text_result(
    value: serde_json::Value,
    text: String,
    output: OutputFormat,
) -> Result<()> {
    match output {
        OutputFormat::Json => println!("{}", serde_json::to_string_pretty(&value)?),
        OutputFormat::Text => println!("{text}"),
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn collects_project_skills() {
        let root = std::env::temp_dir().join(format!("peridot-cli-skills-{}", std::process::id()));
        let skills_dir = root.join(".peridot/skills");
        fs::create_dir_all(&skills_dir).unwrap();
        fs::write(skills_dir.join("rust.md"), "Use cargo fmt.").unwrap();

        let skills = collect_skills(&root).unwrap();

        assert!(skills.iter().any(|skill| skill.name == "rust"));
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
    fn builds_openai_authorize_url_with_escaped_values() {
        let url = openai_oauth_authorize_url(
            "client id",
            "http://127.0.0.1:14552/callback",
            "openid profile",
            "state",
            "challenge",
        );

        assert!(url.contains("client_id=client%20id"));
        assert!(url.contains("redirect_uri=http%3A%2F%2F127.0.0.1%3A14552%2Fcallback"));
        assert!(url.contains("scope=openid%20profile"));
        assert!(url.contains("code_challenge_method=S256"));
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
    fn current_target_has_release_asset_name_shape() {
        let target = current_release_target().unwrap();

        assert!(target.contains('-'));
        assert!(format!("peridot-{target}.tar.gz").starts_with("peridot-"));
    }
}
