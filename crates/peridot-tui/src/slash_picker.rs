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

/// Dynamic auto-skill entry surfaced as a slash suggestion.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct SkillSlashSuggestion {
    /// Skill name without the leading slash.
    pub name: String,
    /// Short description shown in the picker.
    #[serde(default)]
    pub description: String,
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
            description: "switch the active provider (claude-api, openai-api, openrouter-api, ...)",
            arg_hint: Some("<name>"),
            category: "session",
        },
        SlashCommandSpec {
            name: "/note",
            description: "attach an operator note to the current session",
            arg_hint: Some("<text>"),
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
            description: "show, refresh, or search workspace public symbols and TODO markers",
            arg_hint: Some("[refresh|find <query>]"),
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
    /// Command whose first argument is being selected.
    pub spec: &'static SlashCommandSpec,
    /// Filtered options matching the current typed argument prefix.
    pub options: Vec<&'static str>,
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

/// Returns the active finite-argument picker, if the input is inside one.
pub(crate) fn slash_argument_context(query: &str) -> Option<SlashArgumentContext> {
    if !query.starts_with('/') || query.contains('\n') {
        return None;
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
    Some(SlashArgumentContext { spec, options })
}

/// Number of rows the slash picker would render for this query and skill list.
pub(crate) fn picker_len_with_skills(query: &str, skills: &[SkillSlashSuggestion]) -> usize {
    slash_argument_context(query)
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
        let skills = vec![SkillSlashSuggestion {
            name: "auto-fix-parser".to_string(),
            description: "repair parser tests".to_string(),
        }];
        let matches = filtered_suggestions("/auto-f", &skills);
        assert!(matches.iter().any(|entry| {
            entry.name == "/auto-fix-parser"
                && entry.description == "repair parser tests"
                && entry.category == "skill"
                && entry.skill
        }));
    }

    #[test]
    fn built_in_commands_shadow_same_named_skills() {
        let skills = vec![SkillSlashSuggestion {
            name: "plan".to_string(),
            description: "shadowed".to_string(),
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
    fn finite_argument_context_filters_real_options() {
        let context = slash_argument_context("/reasoning ").expect("reasoning options");
        assert_eq!(context.spec.name, "/reasoning");
        assert_eq!(
            context.options,
            vec!["off", "low", "medium", "high", "xhigh"]
        );

        let context = slash_argument_context("/reasoning x").expect("filtered option");
        assert_eq!(context.options, vec!["xhigh"]);
        assert!(slash_argument_context("/reasoning xhigh").is_none());

        let context = slash_argument_context("/autofix ").expect("autofix options");
        assert_eq!(context.options, vec!["on", "off"]);
        assert!(slash_argument_context("/autofix").is_none());

        let context = slash_argument_context("/subagent model ").expect("reset option");
        assert_eq!(context.spec.name, "/subagent model");
        assert_eq!(context.options, vec!["reset"]);
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
