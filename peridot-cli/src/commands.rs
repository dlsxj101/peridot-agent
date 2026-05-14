//! Scriptable CLI subcommand handlers.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use clap::{Subcommand, ValueEnum};
use peridot_common::{McpServerConfig, McpTransport, PeridotConfig};
use peridot_memory::{MemoryStore, SessionSummary};
use peridot_project::{ProjectProfile, ProjectScanner};

/// Scriptable output format.
#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
pub(crate) enum OutputFormat {
    /// Human-readable text.
    Text,
    /// JSON.
    Json,
}

/// Config subcommands.
#[derive(Debug, Subcommand)]
pub(crate) enum ConfigCommand {
    /// Initialize project-local Peridot config.
    Init,
    /// Print the effective config.
    Show,
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

pub(crate) fn run_mcp_command(
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
            print_json_or_text_result(
                serde_json::json!({
                    "name": server.name,
                    "transport": server.transport,
                    "target": mcp_target(server),
                    "configured": true
                }),
                format!(
                    "MCP server {} is configured for {} ({})",
                    server.name,
                    server.transport,
                    mcp_target(server)
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

fn init_project_config(project_root: &Path, output: OutputFormat) -> Result<()> {
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
    print_json_or_text_result(
        serde_json::json!({
            "config_path": config_path,
            "created_config": created_config,
            "updated_gitignore": changed_gitignore
        }),
        format!(
            "initialized {} (created_config={created_config}, updated_gitignore={changed_gitignore})",
            peridot_dir.display()
        ),
        output,
    )
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
}
