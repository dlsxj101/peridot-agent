//! Floating slash-command suggestions.
//!
//! Provides static metadata (`SlashCommandSpec`) for every slash command, plus
//! prefix-matching helpers used by the input handler and the autocompletion
//! overlay.

use serde::{Deserialize, Serialize};

/// Stable description of one slash command surfaced in the picker / help.
#[derive(Clone, Copy, Debug)]
pub struct SlashCommandSpec {
    /// Command keyword including the leading slash.
    pub name: &'static str,
    /// One-line description shown in the picker.
    pub description: &'static str,
    /// Optional argument hint (e.g., `<name>`).
    pub arg_hint: Option<&'static str>,
    /// Category for grouping / filtering.
    pub category: &'static str,
}

/// Returns the client surfaces where a slash command is meaningful.
///
/// The TUI catalog remains the source of truth for accepted commands; editor
/// clients use this additive metadata to avoid suggesting TUI-only controls.
pub fn slash_command_surfaces(spec: &SlashCommandSpec) -> &'static [&'static str] {
    match spec.name {
        "/collapse" | "/lang" | "/sidepanel" => &["tui"],
        _ => &["tui", "vscode"],
    }
}

/// Returns structured finite argument options for clients that should not
/// parse the human-readable `arg_hint` string.
pub fn slash_command_arg_options(spec: &SlashCommandSpec) -> Vec<&'static str> {
    finite_argument_options(spec)
}

/// Dynamic auto-skill entry surfaced as a slash suggestion.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct SkillSlashSuggestion {
    /// Skill name without the leading slash.
    pub name: String,
    /// Short description shown in the picker.
    #[serde(default)]
    pub description: String,
    /// Whether the skill is archived and therefore restore-only.
    #[serde(default)]
    pub archived: bool,
}

/// Render-ready slash suggestion, covering both built-ins and dynamic skills.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SlashSuggestion {
    /// Command keyword including the leading slash.
    pub name: String,
    /// One-line description shown in the picker.
    pub description: String,
    /// Optional argument hint.
    pub arg_hint: Option<String>,
    /// Category for grouping / filtering.
    pub category: String,
    /// Whether this row came from the auto-skill store.
    pub skill: bool,
}

/// Returns the catalog of every slash command the TUI accepts.
pub fn slash_command_catalog() -> &'static [SlashCommandSpec] {
    &[
        SlashCommandSpec {
            name: "/plan",
            description: "switch to plan mode",
            arg_hint: None,
            category: "mode",
        },
        SlashCommandSpec {
            name: "/execute",
            description: "switch to execute mode",
            arg_hint: None,
            category: "mode",
        },
        SlashCommandSpec {
            name: "/goal",
            description: "start a durable goal (or pause/resume/clear/status)",
            arg_hint: Some("<objective>"),
            category: "mode",
        },
        SlashCommandSpec {
            name: "/safe",
            description: "switch to safe permission mode",
            arg_hint: None,
            category: "permission",
        },
        SlashCommandSpec {
            name: "/auto",
            description: "switch to auto permission mode",
            arg_hint: None,
            category: "permission",
        },
        SlashCommandSpec {
            name: "/yolo",
            description: "switch to yolo permission mode",
            arg_hint: None,
            category: "permission",
        },
        SlashCommandSpec {
            name: "/model",
            description: "switch the active model",
            arg_hint: Some("<name>"),
            category: "session",
        },
        SlashCommandSpec {
            name: "/provider",
            description: "switch the active provider",
            arg_hint: Some("<claude-api|openai-api|openrouter-api|openai-oauth>"),
            category: "session",
        },
        SlashCommandSpec {
            name: "/note",
            description: "attach an operator note to the current session",
            arg_hint: Some("<text>"),
            category: "session",
        },
        SlashCommandSpec {
            name: "/notes",
            description: "list operator notes attached to the current session",
            arg_hint: Some("[last <N>]"),
            category: "session",
        },
        SlashCommandSpec {
            name: "/info",
            description: "print a one-shot summary of the current session",
            arg_hint: None,
            category: "session",
        },
        SlashCommandSpec {
            name: "/committee",
            description: "toggle multi-LLM committee mode",
            arg_hint: Some("<off|planner|full>"),
            category: "session",
        },
        SlashCommandSpec {
            name: "/cost",
            description: "show cost / token / cache totals",
            arg_hint: None,
            category: "session",
        },
        SlashCommandSpec {
            name: "/compact",
            description: "queue a context compaction at the next turn",
            arg_hint: None,
            category: "session",
        },
        SlashCommandSpec {
            name: "/context",
            description: "show the largest entries in the current context",
            arg_hint: None,
            category: "session",
        },
        SlashCommandSpec {
            name: "/context top",
            description: "show the largest entries in the current context",
            arg_hint: None,
            category: "session",
        },
        SlashCommandSpec {
            name: "/sidepanel",
            description: "toggle the Status side panel (same as Ctrl+] / F2)",
            arg_hint: None,
            category: "tui",
        },
        SlashCommandSpec {
            name: "/status",
            description: "show or toggle the local status surface",
            arg_hint: None,
            category: "session",
        },
        SlashCommandSpec {
            name: "/collapse",
            description: "toggle collapse of tool/diff transcript blocks",
            arg_hint: None,
            category: "tui",
        },
        SlashCommandSpec {
            name: "/session save",
            description: "save the current session for later resume",
            arg_hint: None,
            category: "session",
        },
        SlashCommandSpec {
            name: "/plan show",
            description: "show the current plan steps",
            arg_hint: None,
            category: "plan",
        },
        SlashCommandSpec {
            name: "/diff",
            description: "show working-tree diff (tool-backed)",
            arg_hint: None,
            category: "git",
        },
        SlashCommandSpec {
            name: "/undo",
            description: "undo the last change (requires tool approval)",
            arg_hint: None,
            category: "git",
        },
        SlashCommandSpec {
            name: "/lang",
            description: "switch display locale",
            arg_hint: Some("<en|ko>"),
            category: "tui",
        },
        SlashCommandSpec {
            name: "/clear",
            description: "clear the transcript",
            arg_hint: None,
            category: "tui",
        },
        SlashCommandSpec {
            name: "/help",
            description: "show this help",
            arg_hint: None,
            category: "tui",
        },
        SlashCommandSpec {
            name: "/skills",
            description: "list, search, show, use, pin, unpin, archive, or restore stored skills",
            arg_hint: Some("[list|search|show|use|pin|unpin|archive|archived|restore]"),
            category: "skill",
        },
        SlashCommandSpec {
            name: "/fork",
            description: "spawn a Fork subagent inline (single-turn)",
            arg_hint: Some("<task>"),
            category: "subagent",
        },
        SlashCommandSpec {
            name: "/teammate",
            description: "spawn a long-running Teammate subagent in a worktree",
            arg_hint: Some("<task>"),
            category: "subagent",
        },
        SlashCommandSpec {
            name: "/worktree",
            description: "explicit worktree-isolated fork",
            arg_hint: Some("<branch> <task>"),
            category: "subagent",
        },
        SlashCommandSpec {
            name: "/subagent model",
            description: "override the default model for spawned subagents (or 'reset')",
            arg_hint: Some("<name|reset>"),
            category: "subagent",
        },
        SlashCommandSpec {
            name: "/reasoning",
            description: "set reasoning intensity",
            arg_hint: Some("<off|low|medium|high|xhigh>"),
            category: "session",
        },
        SlashCommandSpec {
            name: "/fast",
            description: "toggle OpenAI fast / priority service tier",
            arg_hint: Some("[on|off|toggle]"),
            category: "session",
        },
        SlashCommandSpec {
            name: "/think",
            description: "shortcut for /reasoning high (use `/think off` to disable)",
            arg_hint: Some("[off|low|medium|high|xhigh]"),
            category: "session",
        },
        SlashCommandSpec {
            name: "/mcp list",
            description: "list configured MCP servers from config.toml",
            arg_hint: None,
            category: "mcp",
        },
        SlashCommandSpec {
            name: "/mcp add",
            description: "register a new MCP server in config.toml",
            arg_hint: Some("<name> <stdio|http> <command|url>"),
            category: "mcp",
        },
        SlashCommandSpec {
            name: "/mcp remove",
            description: "remove an MCP server from config.toml",
            arg_hint: Some("<name>"),
            category: "mcp",
        },
        SlashCommandSpec {
            name: "/mcp test",
            description: "test connectivity to a configured MCP server",
            arg_hint: Some("<name>"),
            category: "mcp",
        },
        SlashCommandSpec {
            name: "/todos",
            description: "list every TODO / FIXME / HACK / XXX / BUG comment in the project",
            arg_hint: None,
            category: "plan",
        },
        SlashCommandSpec {
            name: "/codemap",
            description: "show, check, refresh, search, locate, outline, or reference workspace symbols",
            arg_hint: Some(
                "[status|refresh|find <query>|locate <symbol>|outline <path>|refs <symbol>]",
            ),
            category: "plan",
        },
        SlashCommandSpec {
            name: "/attach",
            description: "attach a workspace file to the current session context",
            arg_hint: Some("<path>"),
            category: "session",
        },
        SlashCommandSpec {
            name: "/attachments",
            description: "list files attached to the current session context",
            arg_hint: None,
            category: "session",
        },
        SlashCommandSpec {
            name: "/detach",
            description: "remove an attached file from the current session context",
            arg_hint: Some("<path>"),
            category: "session",
        },
        SlashCommandSpec {
            name: "/export",
            description: "export session attachments, notes, and replay timeline",
            arg_hint: Some("[attachments|notes|timeline|full]"),
            category: "session",
        },
        SlashCommandSpec {
            name: "/rewind",
            description: "pop the last user-agent exchange and restore the prompt to the input box",
            arg_hint: None,
            category: "session",
        },
        SlashCommandSpec {
            name: "/branch save",
            description: "snapshot the current session context under a name for later restore",
            arg_hint: Some("<name>"),
            category: "session",
        },
        SlashCommandSpec {
            name: "/branch restore",
            description: "restore a named branch snapshot into the current session (agent must be idle)",
            arg_hint: Some("<name>"),
            category: "session",
        },
        SlashCommandSpec {
            name: "/branch list",
            description: "list every saved branch snapshot",
            arg_hint: None,
            category: "session",
        },
        SlashCommandSpec {
            name: "/branch tree",
            description: "show the DAG journal of abandoned conversation limbs",
            arg_hint: None,
            category: "session",
        },
        SlashCommandSpec {
            name: "/branch turn",
            description: "fork the conversation at a past turn id",
            arg_hint: Some("<turn-id>"),
            category: "session",
        },
        SlashCommandSpec {
            name: "/branch switch",
            description: "swap the active path with a saved limb from the DAG journal",
            arg_hint: Some("<index>"),
            category: "session",
        },
        SlashCommandSpec {
            name: "/session new",
            description: "open a new session, optionally with an initial task",
            arg_hint: Some("[task]"),
            category: "session",
        },
        SlashCommandSpec {
            name: "/session switch",
            description: "switch the foreground session",
            arg_hint: Some("<id|title>"),
            category: "session",
        },
        SlashCommandSpec {
            name: "/session close",
            description: "close a session",
            arg_hint: Some("<id|title>"),
            category: "session",
        },
        SlashCommandSpec {
            name: "/session delete",
            description: "delete a session and its persisted data",
            arg_hint: Some("<id|title>"),
            category: "session",
        },
        SlashCommandSpec {
            name: "/session rename",
            description: "rename a session",
            arg_hint: Some("<id|title> <new title>"),
            category: "session",
        },
        SlashCommandSpec {
            name: "/session list",
            description: "list all sessions",
            arg_hint: None,
            category: "session",
        },
        SlashCommandSpec {
            name: "/session count",
            description: "show persisted session lifecycle counts",
            arg_hint: None,
            category: "session",
        },
        SlashCommandSpec {
            name: "/autofix",
            description: "toggle or configure the auto-fix loop (on|off|<max>)",
            arg_hint: Some("[on|off|<N>]"),
            category: "session",
        },
    ]
}

/// Converts the static catalog plus dynamic skills into slash suggestions.
pub fn slash_suggestions(skills: &[SkillSlashSuggestion]) -> Vec<SlashSuggestion> {
    let mut suggestions: Vec<SlashSuggestion> = slash_command_catalog()
        .iter()
        .map(|spec| SlashSuggestion {
            name: spec.name.to_string(),
            description: spec.description.to_string(),
            arg_hint: spec.arg_hint.map(str::to_string),
            category: spec.category.to_string(),
            skill: false,
        })
        .collect();
    for skill in skills {
        if skill.archived {
            continue;
        }
        let name = format!("/{}", skill.name.trim_start_matches('/'));
        if suggestions.iter().any(|entry| entry.name == name) {
            continue;
        }
        suggestions.push(SlashSuggestion {
            name,
            description: if skill.description.trim().is_empty() {
                "stored auto-skill".to_string()
            } else {
                skill.description.trim().to_string()
            },
            arg_hint: None,
            category: "skill".to_string(),
            skill: true,
        });
    }
    suggestions
}

/// Finite argument options for a command such as `<off|low>` or `[on|off]`.
pub(crate) struct SlashArgumentContext {
    /// Command whose argument is being selected.
    pub command_name: String,
    /// Filtered options matching the current typed argument prefix.
    pub options: Vec<String>,
    /// Whether accepting an option should leave a trailing space for the
    /// next free-form argument.
    pub append_space: bool,
}

/// Filters the catalog by prefix. Empty query returns the whole catalog.
pub fn filtered_specs(query: &str) -> Vec<&'static SlashCommandSpec> {
    let needle = query.trim().trim_start_matches('/').to_ascii_lowercase();
    if needle.is_empty() {
        return slash_command_catalog().iter().collect();
    }
    slash_command_catalog()
        .iter()
        .filter(|spec| {
            let name = spec.name.trim_start_matches('/').to_ascii_lowercase();
            let description = spec.description.to_ascii_lowercase();
            name.starts_with(&needle)
                || name.contains(&format!(" {needle}"))
                || description.contains(&needle)
        })
        .collect()
}

/// Filters built-in slash commands plus dynamic skills by prefix/search text.
pub fn filtered_suggestions(query: &str, skills: &[SkillSlashSuggestion]) -> Vec<SlashSuggestion> {
    let needle = query.trim().trim_start_matches('/').to_ascii_lowercase();
    let mut matches: Vec<SlashSuggestion> = slash_suggestions(skills)
        .into_iter()
        .filter(|suggestion| {
            if needle.is_empty() {
                return true;
            }
            let name = suggestion.name.trim_start_matches('/').to_ascii_lowercase();
            let description = suggestion.description.to_ascii_lowercase();
            name.starts_with(&needle)
                || name.contains(&format!(" {needle}"))
                || description.contains(&needle)
        })
        .collect();
    if !needle.is_empty() {
        matches.sort_by_key(|suggestion| suggestion_match_rank(suggestion, &needle));
    }
    matches
}

fn suggestion_match_rank(suggestion: &SlashSuggestion, needle: &str) -> (u8, String) {
    let name = suggestion.name.trim_start_matches('/').to_ascii_lowercase();
    let description = suggestion.description.to_ascii_lowercase();
    let rank = if name.starts_with(needle) {
        0
    } else if name.contains(&format!(" {needle}")) {
        1
    } else if description.contains(needle) {
        2
    } else {
        3
    };
    (rank, name)
}

/// Returns finite argument options from an arg hint, excluding placeholder arms.
pub(crate) fn finite_argument_options(spec: &SlashCommandSpec) -> Vec<&'static str> {
    if spec.name == "/codemap" {
        return vec!["status", "refresh", "find", "locate", "outline", "refs"];
    }
    finite_argument_options_from_hint(spec.arg_hint)
}

/// Returns finite argument options from a raw arg hint.
pub(crate) fn finite_argument_options_from_hint(hint: Option<&str>) -> Vec<&str> {
    let Some(hint) = hint.map(str::trim) else {
        return Vec::new();
    };
    let opens_choice_list = (hint.starts_with('<') && hint.ends_with('>'))
        || (hint.starts_with('[') && hint.ends_with(']'));
    if !opens_choice_list {
        return Vec::new();
    }
    let body = &hint[1..hint.len().saturating_sub(1)];
    if !body.contains('|') || body.chars().any(char::is_whitespace) {
        return Vec::new();
    }
    body.split('|')
        .map(str::trim)
        .filter(|option| !option.is_empty() && !is_placeholder_option(option))
        .collect()
}

pub(crate) fn accepted_command_text(name: &str, arg_hint: Option<&str>) -> String {
    if arg_hint.is_some() {
        format!("{name} ")
    } else {
        name.to_string()
    }
}

/// Returns the active finite-argument picker, if the input is inside one.
#[cfg(test)]
fn slash_argument_context(query: &str) -> Option<SlashArgumentContext> {
    slash_argument_context_with_skills(query, &[])
}

/// Returns the active argument picker, including dynamic skill-name options.
#[cfg(test)]
fn slash_argument_context_with_skills(
    query: &str,
    skills: &[SkillSlashSuggestion],
) -> Option<SlashArgumentContext> {
    slash_argument_context_with_dynamic(query, skills, &[], &[], &[], &[])
}

/// Returns the active argument picker, including dynamic skill/session/MCP/model/branch options.
pub(crate) fn slash_argument_context_with_dynamic(
    query: &str,
    skills: &[SkillSlashSuggestion],
    sessions: &[crate::session_directory::SessionDirectoryItem],
    mcp_servers: &[crate::state::McpServerSummary],
    models: &[String],
    branches: &[String],
) -> Option<SlashArgumentContext> {
    if !query.starts_with('/') || query.contains('\n') {
        return None;
    }
    if let Some(context) = model_name_argument_context(query, models) {
        return Some(context);
    }
    if let Some(context) = skill_name_argument_context(query, skills) {
        return Some(context);
    }
    if let Some(context) = skills_subcommand_argument_context(query) {
        return Some(context);
    }
    if let Some(context) = skills_search_argument_context(query) {
        return Some(context);
    }
    if let Some(context) = session_target_argument_context(query, sessions) {
        return Some(context);
    }
    if let Some(context) = session_subcommand_argument_context(query) {
        return Some(context);
    }
    if let Some(context) = mcp_server_argument_context(query, mcp_servers) {
        return Some(context);
    }
    if let Some(context) = mcp_add_transport_argument_context(query) {
        return Some(context);
    }
    if let Some(context) = branch_subcommand_argument_context(query) {
        return Some(context);
    }
    if let Some(context) = branch_snapshot_argument_context(query, branches) {
        return Some(context);
    }
    if let Some(context) = codemap_continuation_argument_context(query) {
        return Some(context);
    }
    if let Some(context) = goal_control_argument_context(query) {
        return Some(context);
    }
    if let Some(context) = notes_last_argument_context(query) {
        return Some(context);
    }
    if let Some(context) = export_artifact_argument_context(query) {
        return Some(context);
    }
    if let Some(context) = think_alias_argument_context(query) {
        return Some(context);
    }
    if let Some(context) = fast_alias_argument_context(query) {
        return Some(context);
    }
    if let Some(context) = autofix_alias_argument_context(query) {
        return Some(context);
    }
    let spec = slash_command_catalog()
        .iter()
        .filter(|spec| !finite_argument_options(spec).is_empty())
        .filter(|spec| {
            let exact_optional = query == spec.name && finite_argument_hint_is_optional(spec);
            !exact_optional && (query == spec.name || query.starts_with(&format!("{} ", spec.name)))
        })
        .max_by_key(|spec| spec.name.len())?;
    let options = finite_argument_options(spec);
    let rest = query[spec.name.len()..].trim().to_ascii_lowercase();
    if !rest.is_empty()
        && options
            .iter()
            .any(|option| option.eq_ignore_ascii_case(&rest))
    {
        return None;
    }
    let options = if rest.is_empty() {
        options
    } else {
        options
            .into_iter()
            .filter(|option| option.to_ascii_lowercase().starts_with(&rest))
            .collect()
    };
    if options.is_empty() {
        return None;
    }
    Some(SlashArgumentContext {
        command_name: spec.name.to_string(),
        options: options.into_iter().map(str::to_string).collect(),
        append_space: false,
    })
}

fn model_name_argument_context(query: &str, models: &[String]) -> Option<SlashArgumentContext> {
    let command_name = ["/subagent model", "/model"]
        .into_iter()
        .filter(|command| query == *command || query.starts_with(&format!("{command} ")))
        .max_by_key(|command| command.len())?;
    let rest = query[command_name.len()..].trim().to_ascii_lowercase();
    if rest.contains(char::is_whitespace) {
        return None;
    }
    let mut options: Vec<String> = models
        .iter()
        .map(|model| model.trim().to_string())
        .filter(|model| !model.is_empty())
        .collect();
    if command_name == "/subagent model" {
        options.push("reset".to_string());
    }
    options.sort();
    options.dedup();
    if !rest.is_empty() {
        options.retain(|option| option.to_ascii_lowercase().starts_with(&rest));
    }
    if !rest.is_empty()
        && options
            .iter()
            .any(|option| option.eq_ignore_ascii_case(&rest))
    {
        return None;
    }
    if options.is_empty() {
        return None;
    }
    Some(SlashArgumentContext {
        command_name: command_name.to_string(),
        options,
        append_space: false,
    })
}

fn skill_name_argument_context(
    query: &str,
    skills: &[SkillSlashSuggestion],
) -> Option<SlashArgumentContext> {
    let command_name = [
        "/skills show",
        "/skills view",
        "/skills use",
        "/skills pin",
        "/skills unpin",
        "/skills archive",
        "/skills restore",
    ]
    .into_iter()
    .filter(|command| query == *command || query.starts_with(&format!("{command} ")))
    .max_by_key(|command| command.len())?;
    let mut options: Vec<String> = skills
        .iter()
        .filter(|skill| skill_applies_to_command(command_name, skill.archived))
        .map(|skill| skill.name.trim_start_matches('/').trim().to_string())
        .filter(|name| !name.is_empty())
        .collect();
    options.sort();
    options.dedup();
    let rest = query[command_name.len()..]
        .trim()
        .trim_start_matches('/')
        .to_ascii_lowercase();
    if !rest.is_empty()
        && options
            .iter()
            .any(|option| option.eq_ignore_ascii_case(&rest))
    {
        return None;
    }
    if !rest.is_empty() {
        options.retain(|option| option.to_ascii_lowercase().starts_with(&rest));
    }
    if options.is_empty() {
        return None;
    }
    Some(SlashArgumentContext {
        command_name: command_name.to_string(),
        options,
        append_space: false,
    })
}

fn skill_applies_to_command(command_name: &str, archived: bool) -> bool {
    match command_name {
        "/skills restore" => archived,
        "/skills show" | "/skills view" => true,
        _ => !archived,
    }
}

fn skills_subcommand_argument_context(query: &str) -> Option<SlashArgumentContext> {
    static_subcommand_argument_context(
        query,
        "/skills",
        &["show", "view", "use", "pin", "unpin", "archive", "restore"],
        true,
        false,
    )
}

fn skills_search_argument_context(query: &str) -> Option<SlashArgumentContext> {
    static_subcommand_argument_context(query, "/skills", &["search"], true, false)
}

fn mcp_add_transport_argument_context(query: &str) -> Option<SlashArgumentContext> {
    let command_name = "/mcp add";
    if !query.starts_with(&format!("{command_name} ")) {
        return None;
    }
    let rest = query[command_name.len()..].trim_start();
    let has_trailing_space = rest.chars().last().is_some_and(char::is_whitespace);
    let mut parts = rest.split_whitespace();
    let server_name = parts.next()?.trim();
    if server_name.is_empty() {
        return None;
    }
    let transport_prefix = parts.next();
    if parts.next().is_some() {
        return None;
    }
    if transport_prefix.is_none() && !has_trailing_space {
        return None;
    }
    let prefix = transport_prefix.unwrap_or("").to_ascii_lowercase();
    let options = ["stdio", "http"];
    if !prefix.is_empty()
        && options
            .iter()
            .any(|option| option.eq_ignore_ascii_case(&prefix))
    {
        return None;
    }
    let options: Vec<String> = options
        .into_iter()
        .filter(|option| prefix.is_empty() || option.starts_with(&prefix))
        .map(str::to_string)
        .collect();
    if options.is_empty() {
        return None;
    }
    Some(SlashArgumentContext {
        command_name: format!("{command_name} {server_name}"),
        options,
        append_space: true,
    })
}

fn mcp_server_argument_context(
    query: &str,
    mcp_servers: &[crate::state::McpServerSummary],
) -> Option<SlashArgumentContext> {
    let command_name = ["/mcp remove", "/mcp test"]
        .into_iter()
        .filter(|command| query == *command || query.starts_with(&format!("{command} ")))
        .max_by_key(|command| command.len())?;
    let rest = query[command_name.len()..].trim().to_ascii_lowercase();
    if rest.contains(char::is_whitespace) {
        return None;
    }
    let mut options: Vec<String> = mcp_servers
        .iter()
        .map(|server| server.name.trim().to_string())
        .filter(|name| !name.is_empty())
        .filter(|name| rest.is_empty() || name.to_ascii_lowercase().starts_with(&rest))
        .collect();
    options.sort();
    options.dedup();
    if !rest.is_empty()
        && options
            .iter()
            .any(|option| option.eq_ignore_ascii_case(&rest))
    {
        return None;
    }
    if options.is_empty() {
        return None;
    }
    Some(SlashArgumentContext {
        command_name: command_name.to_string(),
        options,
        append_space: false,
    })
}

fn session_target_argument_context(
    query: &str,
    sessions: &[crate::session_directory::SessionDirectoryItem],
) -> Option<SlashArgumentContext> {
    let command_name = [
        "/session switch",
        "/session close",
        "/session delete",
        "/session rename",
    ]
    .into_iter()
    .filter(|command| query == *command || query.starts_with(&format!("{command} ")))
    .max_by_key(|command| command.len())?;
    let rest = query[command_name.len()..].trim();
    if command_name == "/session rename" && rest.contains(char::is_whitespace) {
        return None;
    }
    let rest_lower = rest.to_ascii_lowercase();
    let mut options: Vec<String> = sessions
        .iter()
        .filter(|session| !session.id.trim().is_empty())
        .filter(|session| {
            rest_lower.is_empty()
                || session.id.to_ascii_lowercase().starts_with(&rest_lower)
                || session.title.to_ascii_lowercase().starts_with(&rest_lower)
        })
        .map(|session| session.id.trim().to_string())
        .collect();
    options.sort();
    options.dedup();
    if !rest_lower.is_empty()
        && options
            .iter()
            .any(|option| option.eq_ignore_ascii_case(&rest_lower))
    {
        return None;
    }
    if options.is_empty() {
        return None;
    }
    Some(SlashArgumentContext {
        command_name: command_name.to_string(),
        options,
        append_space: command_name == "/session rename",
    })
}

fn session_subcommand_argument_context(query: &str) -> Option<SlashArgumentContext> {
    const CONTINUATION_OPTIONS: &[&str] = &["new", "switch", "close", "delete", "rename"];
    const TERMINAL_OPTIONS: &[&str] = &["save", "list", "count"];
    let command_name = "/session";
    if !query.starts_with(&format!("{command_name} ")) {
        return None;
    }
    let has_trailing_space = query.chars().last().is_some_and(char::is_whitespace);
    let rest = query[command_name.len()..].trim().to_ascii_lowercase();
    if rest.is_empty() || rest.contains(char::is_whitespace) {
        return None;
    }
    if TERMINAL_OPTIONS
        .iter()
        .any(|option| option.starts_with(&rest))
    {
        return None;
    }
    let options: Vec<String> = CONTINUATION_OPTIONS
        .iter()
        .filter(|option| option.starts_with(&rest))
        .map(|option| (*option).to_string())
        .collect();
    if options.is_empty() {
        return None;
    }
    let exact = options
        .iter()
        .any(|option| option.eq_ignore_ascii_case(&rest));
    if exact && has_trailing_space {
        return None;
    }
    Some(SlashArgumentContext {
        command_name: command_name.to_string(),
        options,
        append_space: true,
    })
}

fn branch_subcommand_argument_context(query: &str) -> Option<SlashArgumentContext> {
    static_subcommand_argument_context(
        query,
        "/branch",
        &["save", "restore", "turn", "switch"],
        true,
        false,
    )
}

fn branch_snapshot_argument_context(
    query: &str,
    branches: &[String],
) -> Option<SlashArgumentContext> {
    let command_name = "/branch restore";
    if query != command_name && !query.starts_with(&format!("{command_name} ")) {
        return None;
    }
    let rest = query[command_name.len()..].trim().to_ascii_lowercase();
    if rest.contains(char::is_whitespace) {
        return None;
    }
    let mut options: Vec<String> = branches
        .iter()
        .map(|branch| branch.trim().to_string())
        .filter(|branch| !branch.is_empty())
        .filter(|branch| rest.is_empty() || branch.to_ascii_lowercase().starts_with(&rest))
        .collect();
    options.sort();
    options.dedup();
    if !rest.is_empty()
        && options
            .iter()
            .any(|option| option.eq_ignore_ascii_case(&rest))
    {
        return None;
    }
    if options.is_empty() {
        return None;
    }
    Some(SlashArgumentContext {
        command_name: command_name.to_string(),
        options,
        append_space: false,
    })
}

fn codemap_continuation_argument_context(query: &str) -> Option<SlashArgumentContext> {
    const CONTINUATION_OPTIONS: &[&str] = &["find", "locate", "outline", "refs"];
    const TERMINAL_OPTIONS: &[&str] = &["status", "refresh"];
    let command_name = "/codemap";
    if !query.starts_with(&format!("{command_name} ")) {
        return None;
    }
    let has_trailing_space = query.chars().last().is_some_and(char::is_whitespace);
    let rest = query[command_name.len()..].trim().to_ascii_lowercase();
    if rest.is_empty() || rest.contains(char::is_whitespace) {
        return None;
    }
    if TERMINAL_OPTIONS
        .iter()
        .any(|option| option.starts_with(&rest))
    {
        return None;
    }
    let options: Vec<String> = CONTINUATION_OPTIONS
        .iter()
        .filter(|option| option.starts_with(&rest))
        .map(|option| (*option).to_string())
        .collect();
    if options.is_empty() {
        return None;
    }
    let exact = options
        .iter()
        .any(|option| option.eq_ignore_ascii_case(&rest));
    if exact && has_trailing_space {
        return None;
    }
    Some(SlashArgumentContext {
        command_name: command_name.to_string(),
        options,
        append_space: true,
    })
}

fn goal_control_argument_context(query: &str) -> Option<SlashArgumentContext> {
    static_subcommand_argument_context(
        query,
        "/goal",
        &["pause", "resume", "clear", "status"],
        false,
        true,
    )
}

fn notes_last_argument_context(query: &str) -> Option<SlashArgumentContext> {
    static_subcommand_argument_context(query, "/notes", &["last"], true, false)
}

fn export_artifact_argument_context(query: &str) -> Option<SlashArgumentContext> {
    let command_name = "/export";
    if !query.starts_with(&format!("{command_name} ")) {
        return None;
    }
    let rest = query[command_name.len()..].trim_start();
    let has_trailing_space = rest.chars().last().is_some_and(char::is_whitespace);
    let mut tokens: Vec<&str> = rest.split_whitespace().collect();
    let prefix = if has_trailing_space {
        ""
    } else {
        tokens.pop().unwrap_or("")
    };
    if tokens
        .iter()
        .any(|token| !EXPORT_ARTIFACT_OPTIONS.contains(token))
    {
        return None;
    }
    if !prefix.is_empty()
        && EXPORT_ARTIFACT_OPTIONS
            .iter()
            .any(|option| option.eq_ignore_ascii_case(prefix))
    {
        return None;
    }
    let prefix_lower = prefix.to_ascii_lowercase();
    let options: Vec<String> = EXPORT_ARTIFACT_OPTIONS
        .iter()
        .filter(|option| {
            !tokens
                .iter()
                .any(|token| token.eq_ignore_ascii_case(option))
        })
        .filter(|option| prefix_lower.is_empty() || option.starts_with(&prefix_lower))
        .map(|option| (*option).to_string())
        .collect();
    if options.is_empty() {
        return None;
    }
    let command_name = if tokens.is_empty() {
        command_name.to_string()
    } else {
        format!("{command_name} {}", tokens.join(" "))
    };
    Some(SlashArgumentContext {
        command_name,
        options,
        append_space: true,
    })
}

const EXPORT_ARTIFACT_OPTIONS: &[&str] = &["attachments", "notes", "timeline", "full"];
const THINK_ALIAS_OPTIONS: &[&str] = &[
    "hard", "harder", "more", "high", "xhigh", "medium", "low", "off", "stop", "less",
];
const FAST_ALIAS_OPTIONS: &[&str] = &["on", "off", "toggle", "true", "false", "1", "0", "standard"];
const AUTOFIX_ALIAS_OPTIONS: &[&str] = &["on", "off", "true", "false", "1", "0"];

fn think_alias_argument_context(query: &str) -> Option<SlashArgumentContext> {
    static_subcommand_argument_context(query, "/think", THINK_ALIAS_OPTIONS, false, true)
}

fn fast_alias_argument_context(query: &str) -> Option<SlashArgumentContext> {
    static_subcommand_argument_context(query, "/fast", FAST_ALIAS_OPTIONS, false, true)
}

fn autofix_alias_argument_context(query: &str) -> Option<SlashArgumentContext> {
    static_subcommand_argument_context(query, "/autofix", AUTOFIX_ALIAS_OPTIONS, false, true)
}

fn static_subcommand_argument_context(
    query: &str,
    command_name: &str,
    options: &[&str],
    append_space: bool,
    close_on_exact: bool,
) -> Option<SlashArgumentContext> {
    if !query.starts_with(&format!("{command_name} ")) {
        return None;
    }
    let has_trailing_space = query.chars().last().is_some_and(char::is_whitespace);
    let rest = query[command_name.len()..].trim().to_ascii_lowercase();
    if rest.contains(char::is_whitespace) {
        return None;
    }
    let exact = !rest.is_empty()
        && options
            .iter()
            .any(|option| option.eq_ignore_ascii_case(&rest));
    if exact && (close_on_exact || has_trailing_space) {
        return None;
    }
    let options: Vec<String> = options
        .iter()
        .filter(|option| rest.is_empty() || option.starts_with(&rest))
        .map(|option| (*option).to_string())
        .collect();
    if options.is_empty() {
        return None;
    }
    Some(SlashArgumentContext {
        command_name: command_name.to_string(),
        options,
        append_space,
    })
}

/// Number of rows the slash picker would render with all dynamic option sets.
pub(crate) fn picker_len_with_dynamic(
    query: &str,
    skills: &[SkillSlashSuggestion],
    sessions: &[crate::session_directory::SessionDirectoryItem],
    mcp_servers: &[crate::state::McpServerSummary],
    models: &[String],
    branches: &[String],
) -> usize {
    slash_argument_context_with_dynamic(query, skills, sessions, mcp_servers, models, branches)
        .map(|context| context.options.len())
        .unwrap_or_else(|| filtered_suggestions(query, skills).len())
}

/// Returns the first match for `query`, if any.
pub fn first_match(query: &str) -> Option<&'static SlashCommandSpec> {
    filtered_specs(query).into_iter().next()
}

fn is_placeholder_option(option: &str) -> bool {
    if option.contains('<') || option.contains('>') {
        return true;
    }
    matches!(
        option.to_ascii_lowercase().as_str(),
        "branch"
            | "command"
            | "id"
            | "index"
            | "name"
            | "objective"
            | "task"
            | "text"
            | "title"
            | "url"
    )
}

fn finite_argument_hint_is_optional(spec: &SlashCommandSpec) -> bool {
    spec.arg_hint
        .map(str::trim)
        .is_some_and(|hint| hint.starts_with('['))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_query_returns_full_catalog() {
        assert_eq!(filtered_specs("").len(), slash_command_catalog().len());
    }

    #[test]
    fn prefix_filters_to_matching_commands() {
        let matches = filtered_specs("/go");
        assert!(matches.iter().any(|spec| spec.name == "/goal"));
        assert!(!matches.iter().any(|spec| spec.name == "/plan"));
    }

    #[test]
    fn context_alias_is_discoverable_next_to_context_top() {
        let matches = filtered_specs("/context");
        assert!(matches.iter().any(|spec| spec.name == "/context"));
        assert!(matches.iter().any(|spec| spec.name == "/context top"));

        let context = slash_command_catalog()
            .iter()
            .find(|spec| spec.name == "/context")
            .expect("context alias");
        assert_eq!(slash_command_surfaces(context), &["tui", "vscode"]);
        assert!(slash_command_arg_options(context).is_empty());
    }

    #[test]
    fn search_matches_descriptions_and_subcommand_words() {
        assert!(
            filtered_specs("/locale")
                .iter()
                .any(|spec| spec.name == "/lang")
        );
        assert!(
            filtered_specs("/switch")
                .iter()
                .any(|spec| spec.name == "/session switch")
        );
    }

    #[test]
    fn filtered_suggestions_include_dynamic_skills() {
        let skills = vec![
            SkillSlashSuggestion {
                name: "auto-fix-parser".to_string(),
                description: "repair parser tests".to_string(),
                ..Default::default()
            },
            SkillSlashSuggestion {
                name: "old-parser".to_string(),
                archived: true,
                ..Default::default()
            },
        ];
        let matches = filtered_suggestions("/auto-f", &skills);
        assert!(matches.iter().any(|entry| {
            entry.name == "/auto-fix-parser"
                && entry.description == "repair parser tests"
                && entry.category == "skill"
                && entry.skill
        }));
        assert!(filtered_suggestions("/old", &skills).is_empty());
    }

    #[test]
    fn built_in_commands_shadow_same_named_skills() {
        let skills = vec![SkillSlashSuggestion {
            name: "plan".to_string(),
            description: "shadowed".to_string(),
            ..Default::default()
        }];
        let matches = filtered_suggestions("/plan", &skills);
        assert_eq!(
            matches.iter().filter(|entry| entry.name == "/plan").count(),
            1
        );
        assert!(
            matches
                .iter()
                .any(|entry| entry.name == "/plan" && !entry.skill)
        );
    }

    #[test]
    fn slash_command_surfaces_mark_tui_only_controls() {
        let collapse = slash_command_catalog()
            .iter()
            .find(|spec| spec.name == "/collapse")
            .expect("collapse command");
        assert_eq!(slash_command_surfaces(collapse), &["tui"]);

        let sidepanel = slash_command_catalog()
            .iter()
            .find(|spec| spec.name == "/sidepanel")
            .expect("sidepanel command");
        assert_eq!(slash_command_surfaces(sidepanel), &["tui"]);

        let status = slash_command_catalog()
            .iter()
            .find(|spec| spec.name == "/status")
            .expect("status command");
        assert_eq!(slash_command_surfaces(status), &["tui", "vscode"]);

        let plan = slash_command_catalog()
            .iter()
            .find(|spec| spec.name == "/plan")
            .expect("plan command");
        assert_eq!(slash_command_surfaces(plan), &["tui", "vscode"]);
    }

    #[test]
    fn slash_command_arg_options_expose_finite_choices() {
        let reasoning = slash_command_catalog()
            .iter()
            .find(|spec| spec.name == "/reasoning")
            .expect("reasoning command");
        assert_eq!(
            slash_command_arg_options(reasoning),
            vec!["off", "low", "medium", "high", "xhigh"]
        );

        let model = slash_command_catalog()
            .iter()
            .find(|spec| spec.name == "/model")
            .expect("model command");
        assert!(slash_command_arg_options(model).is_empty());

        let provider = slash_command_catalog()
            .iter()
            .find(|spec| spec.name == "/provider")
            .expect("provider command");
        assert_eq!(
            slash_command_arg_options(provider),
            vec!["claude-api", "openai-api", "openrouter-api", "openai-oauth"]
        );

        let codemap = slash_command_catalog()
            .iter()
            .find(|spec| spec.name == "/codemap")
            .expect("codemap command");
        assert_eq!(
            slash_command_arg_options(codemap),
            vec!["status", "refresh", "find", "locate", "outline", "refs"]
        );

        let branch_turn = slash_command_catalog()
            .iter()
            .find(|spec| spec.name == "/branch turn")
            .expect("branch turn command");
        assert!(slash_command_arg_options(branch_turn).is_empty());
    }

    #[test]
    fn finite_argument_context_filters_real_options() {
        let context = slash_argument_context("/reasoning ").expect("reasoning options");
        assert_eq!(context.command_name, "/reasoning");
        assert_eq!(
            context.options,
            vec!["off", "low", "medium", "high", "xhigh"]
        );

        let context = slash_argument_context("/reasoning x").expect("filtered option");
        assert_eq!(context.options, vec!["xhigh"]);
        assert!(slash_argument_context("/reasoning xhigh").is_none());

        let context = slash_argument_context("/autofix ").expect("autofix options");
        assert_eq!(
            context.options,
            vec!["on", "off", "true", "false", "1", "0"]
        );
        assert!(slash_argument_context("/autofix").is_none());

        let context = slash_argument_context("/subagent model ").expect("reset option");
        assert_eq!(context.command_name, "/subagent model");
        assert_eq!(context.options, vec!["reset"]);

        let context = slash_argument_context("/provider open").expect("provider options");
        assert_eq!(context.command_name, "/provider");
        assert_eq!(
            context.options,
            vec!["openai-api", "openrouter-api", "openai-oauth"]
        );
        assert!(slash_argument_context("/provider openai-oauth").is_none());

        let context = slash_argument_context("/codemap l").expect("codemap options");
        assert_eq!(context.command_name, "/codemap");
        assert_eq!(context.options, vec!["locate"]);
        assert!(context.append_space);

        let context =
            slash_argument_context("/codemap locate").expect("codemap locate argument slot");
        assert_eq!(context.command_name, "/codemap");
        assert_eq!(context.options, vec!["locate"]);
        assert!(context.append_space);
        assert!(slash_argument_context("/codemap locate ").is_none());

        let context = slash_argument_context("/codemap r").expect("mixed codemap options");
        assert_eq!(context.options, vec!["refresh", "refs"]);
        assert!(!context.append_space);

        assert!(slash_argument_context("/think").is_none());
        let context = slash_argument_context("/think h").expect("think alias options");
        assert_eq!(context.command_name, "/think");
        assert_eq!(context.options, vec!["hard", "harder", "high"]);
        assert!(slash_argument_context("/think hard").is_none());
        assert!(slash_argument_context("/think fix tests").is_none());

        assert!(slash_argument_context("/fast").is_none());
        let context = slash_argument_context("/fast st").expect("fast alias options");
        assert_eq!(context.command_name, "/fast");
        assert_eq!(context.options, vec!["standard"]);
        assert!(slash_argument_context("/fast standard").is_none());

        assert!(slash_argument_context("/autofix").is_none());
        let context = slash_argument_context("/autofix f").expect("autofix alias options");
        assert_eq!(context.command_name, "/autofix");
        assert_eq!(context.options, vec!["false"]);
        assert!(slash_argument_context("/autofix false").is_none());
        assert!(slash_argument_context("/autofix 5").is_none());

        assert!(slash_argument_context("/mcp add local").is_none());
        let context = slash_argument_context("/mcp add local ").expect("mcp transport options");
        assert_eq!(context.command_name, "/mcp add local");
        assert_eq!(context.options, vec!["stdio", "http"]);
        assert!(context.append_space);

        let context = slash_argument_context("/mcp add local h").expect("filtered transport");
        assert_eq!(context.options, vec!["http"]);
        assert!(slash_argument_context("/mcp add local http").is_none());
        assert!(slash_argument_context("/mcp add local http http://localhost").is_none());
    }

    #[test]
    fn skill_name_argument_context_filters_dynamic_skills() {
        let skills = vec![
            SkillSlashSuggestion {
                name: "auto-fix-parser".to_string(),
                description: "repair parser tests".to_string(),
                ..Default::default()
            },
            SkillSlashSuggestion {
                name: "/test-writer".to_string(),
                description: String::new(),
                ..Default::default()
            },
            SkillSlashSuggestion {
                name: "old-skill".to_string(),
                archived: true,
                ..Default::default()
            },
        ];

        let context = slash_argument_context_with_skills("/skills show auto", &skills)
            .expect("skill options");
        assert_eq!(context.command_name, "/skills show");
        assert_eq!(context.options, vec!["auto-fix-parser"]);

        let context =
            slash_argument_context_with_skills("/skills use /test", &skills).expect("slash trim");
        assert_eq!(context.command_name, "/skills use");
        assert_eq!(context.options, vec!["test-writer"]);

        let context =
            slash_argument_context_with_skills("/skills restore old", &skills).expect("restore");
        assert_eq!(context.command_name, "/skills restore");
        assert_eq!(context.options, vec!["old-skill"]);

        assert!(
            slash_argument_context_with_skills("/skills restore auto", &skills).is_none(),
            "restore should only suggest archived skills"
        );
        assert!(
            slash_argument_context_with_skills("/skills archive auto-fix-parser", &skills)
                .is_none()
        );
    }

    #[test]
    fn skills_search_argument_context_leaves_room_for_query() {
        let context = slash_argument_context_with_dynamic("/skills se", &[], &[], &[], &[], &[])
            .expect("skills search option");
        assert_eq!(context.command_name, "/skills");
        assert_eq!(context.options, vec!["search"]);
        assert!(context.append_space);

        let context =
            slash_argument_context_with_dynamic("/skills search", &[], &[], &[], &[], &[])
                .expect("exact search should still complete to a query slot");
        assert_eq!(context.options, vec!["search"]);
        assert!(context.append_space);

        assert!(
            slash_argument_context_with_dynamic("/skills search ", &[], &[], &[], &[], &[])
                .is_none(),
            "query slot is free-form after the trailing space"
        );
    }

    #[test]
    fn skills_subcommand_argument_context_leaves_room_for_skill_name() {
        let context = slash_argument_context_with_dynamic("/skills sh", &[], &[], &[], &[], &[])
            .expect("skills show option");
        assert_eq!(context.command_name, "/skills");
        assert_eq!(context.options, vec!["show"]);
        assert!(context.append_space);

        let context =
            slash_argument_context_with_dynamic("/skills restore", &[], &[], &[], &[], &[])
                .expect("skills restore option");
        assert_eq!(context.options, vec!["restore"]);
        assert!(context.append_space);

        assert!(
            slash_argument_context_with_dynamic("/skills restore ", &[], &[], &[], &[], &[])
                .is_none()
        );
        assert!(
            slash_argument_context_with_dynamic("/skills list", &[], &[], &[], &[], &[]).is_none()
        );
    }

    #[test]
    fn session_target_argument_context_filters_directory_items() {
        let sessions = vec![
            crate::session_directory::SessionDirectoryItem::new("s-1", "parser cleanup"),
            crate::session_directory::SessionDirectoryItem::new("s-2", "release prep"),
        ];

        let context = slash_argument_context_with_dynamic(
            "/session switch release",
            &[],
            &sessions,
            &[],
            &[],
            &[],
        )
        .expect("session target");
        assert_eq!(context.command_name, "/session switch");
        assert_eq!(context.options, vec!["s-2"]);
        assert!(!context.append_space);

        let context = slash_argument_context_with_dynamic(
            "/session rename parser",
            &[],
            &sessions,
            &[],
            &[],
            &[],
        )
        .expect("rename target");
        assert_eq!(context.options, vec!["s-1"]);
        assert!(context.append_space);

        assert!(
            slash_argument_context_with_dynamic(
                "/session switch s-2",
                &[],
                &sessions,
                &[],
                &[],
                &[],
            )
            .is_none()
        );
        assert!(
            slash_argument_context_with_dynamic(
                "/session rename s-1 new title",
                &[],
                &sessions,
                &[],
                &[],
                &[]
            )
            .is_none()
        );
    }

    #[test]
    fn session_subcommand_argument_context_leaves_room_for_required_args() {
        let context = slash_argument_context_with_dynamic("/session sw", &[], &[], &[], &[], &[])
            .expect("session switch option");
        assert_eq!(context.command_name, "/session");
        assert_eq!(context.options, vec!["switch"]);
        assert!(context.append_space);

        let context =
            slash_argument_context_with_dynamic("/session rename", &[], &[], &[], &[], &[])
                .expect("session rename option");
        assert_eq!(context.options, vec!["rename"]);
        assert!(context.append_space);

        assert!(
            slash_argument_context_with_dynamic("/session rename ", &[], &[], &[], &[], &[])
                .is_none()
        );
        assert!(
            slash_argument_context_with_dynamic("/session s", &[], &[], &[], &[], &[]).is_none(),
            "ambiguous save/switch prefixes fall back to command suggestions"
        );
        assert!(
            slash_argument_context_with_dynamic("/session save", &[], &[], &[], &[], &[]).is_none()
        );
    }

    #[test]
    fn mcp_server_argument_context_filters_configured_servers() {
        let servers = vec![
            crate::state::McpServerSummary {
                name: "filesystem".to_string(),
                tool_count: 4,
                connected: true,
            },
            crate::state::McpServerSummary {
                name: "github".to_string(),
                tool_count: 2,
                connected: false,
            },
        ];

        let context =
            slash_argument_context_with_dynamic("/mcp test g", &[], &[], &servers, &[], &[])
                .expect("mcp server");
        assert_eq!(context.command_name, "/mcp test");
        assert_eq!(context.options, vec!["github"]);
        assert!(!context.append_space);

        let context =
            slash_argument_context_with_dynamic("/mcp remove ", &[], &[], &servers, &[], &[])
                .expect("mcp remove");
        assert_eq!(context.options, vec!["filesystem", "github"]);
        assert!(
            slash_argument_context_with_dynamic("/mcp test github", &[], &[], &servers, &[], &[])
                .is_none()
        );
        assert!(
            slash_argument_context_with_dynamic(
                "/mcp test github extra",
                &[],
                &[],
                &servers,
                &[],
                &[],
            )
            .is_none()
        );
    }

    #[test]
    fn model_name_argument_context_filters_configured_models() {
        let models = vec!["claude-sonnet-4-6".to_string(), "gpt-5.1-codex".to_string()];

        let context = slash_argument_context_with_dynamic("/model g", &[], &[], &[], &models, &[])
            .expect("model");
        assert_eq!(context.command_name, "/model");
        assert_eq!(context.options, vec!["gpt-5.1-codex"]);
        assert!(!context.append_space);

        let context =
            slash_argument_context_with_dynamic("/subagent model ", &[], &[], &[], &models, &[])
                .expect("subagent model");
        assert_eq!(
            context.options,
            vec!["claude-sonnet-4-6", "gpt-5.1-codex", "reset"]
        );
        assert!(
            slash_argument_context_with_dynamic(
                "/model gpt-5.1-codex",
                &[],
                &[],
                &[],
                &models,
                &[],
            )
            .is_none()
        );
        assert!(
            slash_argument_context_with_dynamic(
                "/subagent model gpt-5.1-codex extra",
                &[],
                &[],
                &[],
                &models,
                &[],
            )
            .is_none()
        );
    }

    #[test]
    fn branch_snapshot_argument_context_filters_saved_branches() {
        let branches = vec!["parser-snapshot".to_string(), "release-branch".to_string()];

        let context = slash_argument_context_with_dynamic(
            "/branch restore rel",
            &[],
            &[],
            &[],
            &[],
            &branches,
        )
        .expect("branch restore");
        assert_eq!(context.command_name, "/branch restore");
        assert_eq!(context.options, vec!["release-branch"]);
        assert!(!context.append_space);

        assert!(
            slash_argument_context_with_dynamic(
                "/branch restore release-branch",
                &[],
                &[],
                &[],
                &[],
                &branches,
            )
            .is_none()
        );
        assert!(
            slash_argument_context_with_dynamic(
                "/branch restore release-branch extra",
                &[],
                &[],
                &[],
                &[],
                &branches,
            )
            .is_none()
        );
    }

    #[test]
    fn branch_subcommand_argument_context_leaves_room_for_required_args() {
        let context = slash_argument_context_with_dynamic("/branch tu", &[], &[], &[], &[], &[])
            .expect("branch turn");
        assert_eq!(context.command_name, "/branch");
        assert_eq!(context.options, vec!["turn"]);
        assert!(context.append_space);

        let context =
            slash_argument_context_with_dynamic("/branch switch", &[], &[], &[], &[], &[])
                .expect("branch switch");
        assert_eq!(context.options, vec!["switch"]);
        assert!(context.append_space);

        assert!(
            slash_argument_context_with_dynamic("/branch switch ", &[], &[], &[], &[], &[])
                .is_none()
        );
        assert!(
            slash_argument_context_with_dynamic("/branch tree", &[], &[], &[], &[], &[]).is_none()
        );
    }

    #[test]
    fn goal_control_argument_context_filters_subcommands() {
        assert!(
            slash_argument_context_with_dynamic("/goal", &[], &[], &[], &[], &[]).is_none(),
            "bare /goal remains runnable as goal mode"
        );

        let context = slash_argument_context_with_dynamic("/goal p", &[], &[], &[], &[], &[])
            .expect("goal control");
        assert_eq!(context.command_name, "/goal");
        assert_eq!(context.options, vec!["pause"]);
        assert!(!context.append_space);

        let context = slash_argument_context_with_dynamic("/goal ", &[], &[], &[], &[], &[])
            .expect("goal controls");
        assert_eq!(context.options, vec!["pause", "resume", "clear", "status"]);
        assert!(
            slash_argument_context_with_dynamic("/goal pause", &[], &[], &[], &[], &[]).is_none()
        );
        assert!(
            slash_argument_context_with_dynamic("/goal fix tests", &[], &[], &[], &[], &[])
                .is_none()
        );
    }

    #[test]
    fn notes_last_argument_context_leaves_room_for_count() {
        assert!(
            slash_argument_context_with_dynamic("/notes", &[], &[], &[], &[], &[]).is_none(),
            "bare /notes remains runnable"
        );

        let context = slash_argument_context_with_dynamic("/notes l", &[], &[], &[], &[], &[])
            .expect("notes last");
        assert_eq!(context.command_name, "/notes");
        assert_eq!(context.options, vec!["last"]);
        assert!(context.append_space);

        let context = slash_argument_context_with_dynamic("/notes last", &[], &[], &[], &[], &[])
            .expect("notes exact");
        assert_eq!(context.options, vec!["last"]);
        assert!(context.append_space);
        assert!(
            slash_argument_context_with_dynamic("/notes last ", &[], &[], &[], &[], &[]).is_none()
        );
    }

    #[test]
    fn export_artifact_argument_context_filters_remaining_artifacts() {
        assert!(
            slash_argument_context_with_dynamic("/export", &[], &[], &[], &[], &[]).is_none(),
            "bare /export remains runnable"
        );

        let context = slash_argument_context_with_dynamic("/export a", &[], &[], &[], &[], &[])
            .expect("export artifact");
        assert_eq!(context.command_name, "/export");
        assert_eq!(context.options, vec!["attachments"]);
        assert!(context.append_space);

        assert!(
            slash_argument_context_with_dynamic("/export attachments", &[], &[], &[], &[], &[])
                .is_none(),
            "exact single-artifact export remains runnable"
        );

        let context =
            slash_argument_context_with_dynamic("/export attachments ", &[], &[], &[], &[], &[])
                .expect("remaining artifacts");
        assert_eq!(context.command_name, "/export attachments");
        assert_eq!(context.options, vec!["notes", "timeline", "full"]);
        assert!(context.append_space);

        let context =
            slash_argument_context_with_dynamic("/export attachments n", &[], &[], &[], &[], &[])
                .expect("filtered remaining artifact");
        assert_eq!(context.command_name, "/export attachments");
        assert_eq!(context.options, vec!["notes"]);

        assert!(
            slash_argument_context_with_dynamic(
                "/export attachments bad",
                &[],
                &[],
                &[],
                &[],
                &[],
            )
            .is_none()
        );
    }

    #[test]
    fn first_match_returns_some_for_prefix() {
        assert_eq!(
            first_match("/compa").map(|spec| spec.name),
            Some("/compact")
        );
        assert_eq!(
            first_match("/commit").map(|spec| spec.name),
            Some("/committee"),
        );
        assert!(first_match("/does-not-exist").is_none());
    }
}
