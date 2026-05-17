//! Floating slash-command suggestions.
//!
//! Provides static metadata (`SlashCommandSpec`) for every slash command, plus
//! prefix-matching helpers used by the input handler and the autocompletion
//! overlay.

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
            description: "toggle multi-LLM committee mode (off|planner|full)",
            arg_hint: Some("<mode>"),
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
            arg_hint: Some("en|ko"),
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
            description: "set reasoning intensity (off / low / medium / high)",
            arg_hint: Some("<off|low|medium|high>"),
            category: "session",
        },
        SlashCommandSpec {
            name: "/think",
            description: "shortcut for /reasoning high (use `/think off` to disable)",
            arg_hint: Some("[off]"),
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
            name: "/session list",
            description: "list all sessions",
            arg_hint: None,
            category: "session",
        },
    ]
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
            spec.name
                .trim_start_matches('/')
                .to_ascii_lowercase()
                .starts_with(&needle)
        })
        .collect()
}

/// Returns the first match for `query`, if any.
pub fn first_match(query: &str) -> Option<&'static SlashCommandSpec> {
    filtered_specs(query).into_iter().next()
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
