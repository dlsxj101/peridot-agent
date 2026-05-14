//! Scriptable CLI subcommand handlers.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use clap::{Subcommand, ValueEnum};
use peridot_common::PeridotConfig;
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
