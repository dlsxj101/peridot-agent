//! Scriptable CLI subcommand handlers.

// Submodules pull these names through `use super::*;`. Several are traits whose
// methods are called downstream (e.g. `Context`, `Read`, `Write`, `Engine`,
// `IsTerminal`, `Digest`), so they look unused from this file's perspective.
#![allow(unused_imports)]

use std::collections::HashMap;
use std::fs;
use std::io::{IsTerminal, Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use clap::{Subcommand, ValueEnum};
use peridot_common::{HooksConfig, McpServerConfig, McpTransport, PeridotConfig};
use peridot_mcp::McpClient;
use peridot_memory::{MemoryStore, SessionSummary};
use peridot_project::{ProjectProfile, ProjectScanner};
use peridot_tools::hooks::{HookRunner, HookVariables};
use peridot_verify::{VerifyPipeline, VerifyReport, VerifyStage, VerifyStageResult};
use serde_json::Value;
use sha2::{Digest, Sha256};

mod agents;
mod auth;
mod config;
mod mcp;
mod output;
mod project;
mod session;
mod setup;
mod skills;
#[cfg(test)]
mod tests;
mod update;
mod verify;

pub(crate) use agents::run_agents_command;
use agents::{agents_draft, find_agents_instruction};
use auth::unix_timestamp;
pub(crate) use auth::{
    read_managed_env_var, read_stored_api_key, read_stored_openai_oauth_access_token,
    run_env_command, run_login_command, run_logout_command,
};
use config::init_project_config_value;
pub(crate) use config::{load_effective_config, maybe_run_first_launch_wizard, run_config_command};
pub(crate) use mcp::run_mcp_command;
use output::print_json_or_text_result;
pub(crate) use project::print_scan;
pub(crate) use session::run_session_command;
pub(crate) use setup::run_setup_command;
pub(crate) use skills::run_skill_command;
pub(crate) use update::{maybe_print_update_notice, run_update_command};
pub(crate) use verify::run_verify_command;

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
    /// OpenRouter API key.
    OpenrouterApi,
    /// OpenAI OAuth PKCE flow.
    OpenaiOauth,
}

/// User-local environment variable store commands.
#[derive(Debug, Subcommand)]
pub(crate) enum EnvCommand {
    /// Store an environment variable in Peridot's user-local env store.
    Set {
        /// Environment variable name.
        key: String,
        /// Value to store. If omitted, Peridot reads the value from stdin.
        value: Option<String>,
    },
    /// Print a stored environment variable value.
    Get {
        /// Environment variable name.
        key: String,
    },
    /// List stored environment variable names.
    List,
    /// Remove a stored environment variable.
    Unset {
        /// Environment variable name.
        key: String,
    },
}

impl AuthProvider {
    fn id(self) -> &'static str {
        match self {
            Self::ClaudeApi => "claude-api",
            Self::OpenaiApi => "openai-api",
            Self::OpenrouterApi => "openrouter-api",
            Self::OpenaiOauth => "openai-oauth",
        }
    }

    fn api_key_env_var(self) -> Option<&'static str> {
        match self {
            Self::ClaudeApi => Some("ANTHROPIC_API_KEY"),
            Self::OpenaiApi => Some("OPENAI_API_KEY"),
            Self::OpenrouterApi => Some("OPENROUTER_API_KEY"),
            Self::OpenaiOauth => None,
        }
    }
}

/// Config subcommands.
#[derive(Debug, Subcommand)]
pub(crate) enum ConfigCommand {
    /// Initialize project-local Peridot config.
    Init,
    /// Run the interactive welcome setup wizard again.
    Wizard,
    /// Set one project config value, such as auth.primary or models.main.
    Set {
        /// Dot-separated config key.
        key: String,
        /// Value to write.
        value: String,
    },
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
    /// Replay the persisted transcript for one session.
    Replay {
        /// Session id.
        id: String,
        /// Limit output to the most recent N transcript entries.
        #[arg(long)]
        last: Option<usize>,
        /// Pause for `Enter` between entries; type `q` to quit early.
        #[arg(long)]
        step: bool,
    },
    /// Follow a live session's transcript.ndjson journal (tail -f style).
    Tail {
        /// Session id.
        id: String,
        /// Polling interval in milliseconds.
        #[arg(long, default_value_t = 200)]
        interval_ms: u64,
        /// Skip the existing entries and only print new ones.
        #[arg(long)]
        from_now: bool,
    },
    /// Search persisted transcripts for a substring (case insensitive).
    Search {
        /// Substring to look for.
        query: String,
        /// Restrict the search to one session id.
        #[arg(long)]
        session: Option<String>,
        /// Stop after the first N matches.
        #[arg(long)]
        limit: Option<usize>,
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
