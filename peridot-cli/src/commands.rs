//! Scriptable CLI subcommand handlers.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result};
use clap::{Subcommand, ValueEnum};
use peridot_common::{McpServerConfig, McpTransport, PeridotConfig};
use peridot_mcp::McpClient;
use peridot_memory::{MemoryStore, SessionSummary};
use peridot_project::{ProjectProfile, ProjectScanner};
use peridot_verify::VerifyPipeline;
use serde_json::Value;

/// Scriptable output format.
#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
pub(crate) enum OutputFormat {
    /// Human-readable text.
    Text,
    /// JSON.
    Json,
}

/// API-key auth providers supported by `peridot login`.
#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
pub(crate) enum AuthProvider {
    /// Anthropic Claude API key.
    ClaudeApi,
    /// OpenAI API key.
    OpenaiApi,
}

impl AuthProvider {
    fn id(self) -> &'static str {
        match self {
            Self::ClaudeApi => "claude-api",
            Self::OpenaiApi => "openai-api",
        }
    }

    fn env_var(self) -> &'static str {
        match self {
            Self::ClaudeApi => "ANTHROPIC_API_KEY",
            Self::OpenaiApi => "OPENAI_API_KEY",
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

pub(crate) fn load_project_config(project_root: &Path) -> Result<PeridotConfig> {
    let path = project_root.join(".peridot/config.toml");
    if !path.exists() {
        return Ok(PeridotConfig::default());
    }
    let content =
        fs::read_to_string(&path).with_context(|| format!("failed to read {}", path.display()))?;
    let config = toml::from_str::<PeridotConfig>(&content)
        .with_context(|| format!("failed to parse {}", path.display()))?;
    Ok(config)
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

pub(crate) fn run_skill_command(
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

pub(crate) fn run_login_command(provider: AuthProvider, output: OutputFormat) -> Result<()> {
    let api_key = std::env::var(provider.env_var())
        .with_context(|| format!("{} is required for login", provider.env_var()))?;
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
    if !check && update_available {
        anyhow::bail!("self-install is not implemented yet; download the release from {html_url}");
    }
    print_json_or_text_result(
        serde_json::json!({
            "current": current,
            "latest": latest,
            "update_available": update_available,
            "release_url": html_url,
            "checked_only": check
        }),
        if update_available {
            format!("Peridot {latest} is available (current {current}): {html_url}")
        } else {
            format!("Peridot is up to date ({current})")
        },
        output,
    )
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
}
