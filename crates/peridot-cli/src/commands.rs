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
mod attach;
mod auth;
mod codemap;
mod config;
mod daemon;
mod doctor;
mod mcp;
mod output;
mod project;
mod session;
mod settings;
mod settings_i18n;
mod setup;
mod ship;
mod skills;
#[cfg(test)]
mod tests;
mod update;
mod verify;

pub(crate) use agents::run_agents_command;
use agents::{agents_draft, find_agents_instruction};
pub(crate) use attach::{
    AttachmentArtifact, attachment_plan_reminder, attachments_from_context,
    detach_attachments_from_context, load_text_attachment,
};
use auth::unix_timestamp;
pub(crate) use auth::{
    OpenAiOAuthCredentials, openai_oauth_access_token_identity, read_managed_env_var,
    read_stored_api_key, read_stored_openai_oauth_credentials, run_env_command, run_login_command,
    run_logout_command,
};
pub(crate) use codemap::{
    CodeMapIndex, CodeMapIndexLoad, CodeMapReport, CodeMapStatus, build_code_map, code_map_status,
    find_code_map_references, load_or_refresh_code_map_index,
    load_or_refresh_code_map_index_with_status, locate_code_map_symbols, outline_code_map_file,
    refresh_code_map_index, search_code_map_index,
};
use config::init_project_config_value;
pub(crate) use config::{
    load_effective_config, maybe_run_first_launch_wizard, run_config_command, set_config_key,
};
pub(crate) use daemon::run_daemon_command;
pub(crate) use doctor::run_doctor_command;
pub(crate) use mcp::run_mcp_command;
use output::print_json_or_text_result;
pub(crate) use project::print_scan;
pub(crate) use session::{
    SessionCountSummary, SessionExportReport, SessionLocateResult, SessionResumeResult,
    SessionSearchHit, SessionSearchResult, SessionShowResult, append_session_note,
    export_session_artifacts, read_session_notes, rewind_context_entries, run_session_command,
    search_session_transcript_hits, session_count_summary, session_locate, session_resume_summary,
    session_resume_task_text, session_show_summary,
};
pub(crate) use settings::run_setting_command;
pub(crate) use setup::run_setup_command;
pub(crate) use ship::{ShipOptions, run_ship_command};
pub(crate) use skills::{move_auto_skill_to_archive, restore_archived_skill, run_skill_command};
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

/// Session artifact class selected for `peridot session export`.
#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
pub(crate) enum SessionExportArtifact {
    /// Copy the full persisted session directory.
    Full,
    /// Export reconstructed session attachments.
    Attachments,
    /// Export operator notes.
    Notes,
    /// Export replay timeline data.
    Timeline,
}

impl SessionExportArtifact {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            SessionExportArtifact::Full => "full",
            SessionExportArtifact::Attachments => "attachments",
            SessionExportArtifact::Notes => "notes",
            SessionExportArtifact::Timeline => "timeline",
        }
    }
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
    /// List supported live providers (`auth.primary` values).
    Providers,
    /// Print the configured main and goal-checker model names.
    Models,
}

/// Session subcommands.
#[derive(Debug, Subcommand)]
pub(crate) enum SessionCommand {
    /// List saved sessions.
    List {
        /// Filter by lifecycle (idle, running, suspended, done, failed).
        #[arg(long)]
        status: Option<String>,
    },
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
        /// Also print the most recent N operator notes inline.
        #[arg(long)]
        notes_tail: Option<usize>,
        /// Also print the most recent N transcript entries inline.
        #[arg(long)]
        transcript_tail: Option<usize>,
        /// Also print the most recent N committee events inline (M-COM6).
        #[arg(long)]
        committee_tail: Option<usize>,
    },
    /// Delete one persisted session.
    Delete {
        /// Session id.
        id: String,
    },
    /// Rename one session.
    Rename {
        /// Session id.
        id: String,
        /// New display title.
        title: Vec<String>,
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
    /// Remove finished / stale sessions from disk and the memory store.
    Prune {
        /// Only prune sessions whose lifecycle matches (done, failed, interrupted, suspended).
        #[arg(long)]
        status: Option<String>,
        /// Only prune sessions whose `updated_at` is older than N days.
        #[arg(long)]
        older_than_days: Option<u64>,
        /// Print which sessions would be removed without touching anything.
        #[arg(long)]
        dry_run: bool,
    },
    /// Copy one session's persisted artifacts to a portable directory.
    Export {
        /// Session id to export.
        id: String,
        /// Destination directory. Created if missing.
        #[arg(long)]
        out: PathBuf,
        /// Artifact class to export. Repeat for multiple classes. Defaults to full.
        #[arg(long = "artifact", value_enum)]
        artifacts: Vec<SessionExportArtifact>,
        /// Overwrite an existing destination directory.
        #[arg(long)]
        force: bool,
    },
    /// Import a previously-exported session directory.
    Import {
        /// Source directory containing the exported session artifacts.
        from: PathBuf,
        /// Optional session id to register under (defaults to source dir basename).
        #[arg(long)]
        id: Option<String>,
        /// Overwrite an existing session with the same id.
        #[arg(long)]
        force: bool,
    },
    /// Attach or read operator notes for one session.
    Note {
        /// Session id.
        id: String,
        /// Note subcommand.
        #[command(subcommand)]
        action: SessionNoteAction,
    },
    /// Print the on-disk directory for a session.
    Locate {
        /// Session id.
        id: String,
    },
    /// Print a count of sessions grouped by lifecycle.
    Count,
}

/// Subcommands of `peridot session note <id>`.
#[derive(Debug, Subcommand)]
pub(crate) enum SessionNoteAction {
    /// Append a new note line.
    Add {
        /// Note text (joined with spaces).
        text: Vec<String>,
    },
    /// Print all notes attached to the session.
    List {
        /// Only print the most recent N notes.
        #[arg(long)]
        last: Option<usize>,
    },
    /// Remove every note attached to the session.
    Clear,
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
    /// Install a project-local community skill from a URL, Markdown file, or
    /// directory containing SKILL.md.
    Install {
        /// HTTP(S) URL, local Markdown file path, or local skill directory.
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
    /// Restore an archived auto-skill: clears `archived_at_unix` in the
    /// DB and moves `.peridot/skills/archive/<name>.md` or
    /// `.peridot/skills/archive/<name>/` back into auto skills. Useful when the Curator was
    /// over-zealous; manual auth/scope rules still apply.
    Restore {
        /// Skill name (file stem of `<name>.md`).
        name: String,
    },
    /// Run the Curator. With no flags this applies the 30/90-day stale/
    /// archive rules to agent-authored skills. `--llm` also invokes the
    /// Hermes-style LLM reflection pass (one batch of at most 8 skills,
    /// keep/patch/consolidate/archive). `--dry-run` skips the LLM call
    /// and prints rule-only decisions.
    Curate {
        /// Print decisions but don't persist archive writes.
        #[arg(long)]
        dry_run: bool,
        /// Also run the Hermes-style LLM reflection pass.
        #[arg(long)]
        llm: bool,
    },
    /// Pin a skill so the Curator cannot archive it.
    Pin {
        /// Skill name.
        name: String,
    },
    /// Unpin a skill, restoring it to Curator management.
    Unpin {
        /// Skill name.
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
    /// Run an end-to-end health probe against every configured MCP server
    /// and report latency, tool count, and resolved permission level.
    Doctor,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct SkillEntry {
    name: String,
    scope: &'static str,
    path: PathBuf,
}
