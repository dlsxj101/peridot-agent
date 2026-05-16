use std::str::FromStr;

use peridot_common::{CommitteeMode, Locale, ReasoningEffort};
use serde::{Deserialize, Serialize};

/// Slash commands supported by Peridot's interactive surfaces.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum SlashCommand {
    /// Switch to plan mode.
    Plan,
    /// Switch to execute mode.
    Execute,
    /// Start goal mode with an objective.
    GoalStart(String),
    /// Pause goal execution.
    GoalPause,
    /// Resume goal execution.
    GoalResume,
    /// Clear the active goal.
    GoalClear,
    /// Show goal status.
    GoalStatus,
    /// Switch to safe permission mode.
    Safe,
    /// Switch to auto permission mode.
    Auto,
    /// Switch to yolo permission mode.
    Yolo,
    /// Clear the visible transcript.
    Clear,
    /// Show help for interactive commands.
    Help,
    /// Show cost and token accounting.
    Cost,
    /// Show the current plan.
    PlanShow,
    /// Switch the active model.
    Model(String),
    /// Switch the active provider (claude-api, openai-api, openrouter-api, ...).
    Provider(String),
    /// Toggle the multi-LLM committee mode (off / planner / full).
    Committee(CommitteeMode),
    /// Append a free-form note to the current session's notes.ndjson.
    Note(String),
    /// Print a one-shot summary of the current session (model, provider,
    /// workspace, session id, mode, permission, turn, tokens, cost).
    Info,
    /// Request context compaction.
    Compact,
    /// Save the current session.
    SessionSave,
    /// Show working-tree diff.
    Diff,
    /// Undo the last change.
    Undo,
    /// Change the display locale for TUI strings.
    Lang(Locale),
    /// Spawn a Fork subagent in the same workspace (single-turn, inline result).
    Fork(String),
    /// Spawn a long-running Teammate subagent into a worktree-isolated session.
    Teammate(String),
    /// Spawn an explicit worktree-isolated fork on the named branch.
    Worktree {
        /// Git branch to materialize as a worktree.
        branch: String,
        /// Task text to dispatch to the new session.
        task: String,
    },
    /// Open a new session, optionally with an initial task.
    SessionNew(Option<String>),
    /// Switch the foreground session by id or 1-based index.
    SessionSwitch(String),
    /// Close a session by id or 1-based index.
    SessionClose(String),
    /// List all known sessions in the transcript.
    SessionList,
    /// Override the default model used when spawning sub-agents. `reset`
    /// clears the override so future spawns inherit the caller's main model.
    SubagentModel(SubagentModelChange),
    /// Change the reasoning intensity applied to every model request.
    /// Cheap models without a reasoning channel ignore the setting.
    Reasoning(ReasoningEffort),
}

/// Payload for `/subagent model <name|reset>`. Wrapped in a dedicated enum so
/// the parser distinguishes "set to specific name" from "clear override".
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum SubagentModelChange {
    /// Set the sub-agent default model name to the wrapped string.
    Set(String),
    /// Clear the override; sub-agents fall back to caller's main model.
    Reset,
}

/// Parses a user slash command.
pub fn parse_slash_command(input: &str) -> Option<SlashCommand> {
    let input = input.trim();
    let body = input.strip_prefix('/')?;
    let mut parts = body.splitn(2, char::is_whitespace);
    let command = parts.next()?.trim();
    let rest = parts.next().unwrap_or("").trim();

    match command {
        "plan" if rest.is_empty() => Some(SlashCommand::Plan),
        "execute" if rest.is_empty() => Some(SlashCommand::Execute),
        "safe" if rest.is_empty() => Some(SlashCommand::Safe),
        "auto" if rest.is_empty() => Some(SlashCommand::Auto),
        "yolo" if rest.is_empty() => Some(SlashCommand::Yolo),
        "clear" if rest.is_empty() => Some(SlashCommand::Clear),
        "help" if rest.is_empty() => Some(SlashCommand::Help),
        "cost" if rest.is_empty() => Some(SlashCommand::Cost),
        "compact" if rest.is_empty() => Some(SlashCommand::Compact),
        "diff" if rest.is_empty() => Some(SlashCommand::Diff),
        "undo" if rest.is_empty() => Some(SlashCommand::Undo),
        "model" if !rest.is_empty() => Some(SlashCommand::Model(rest.to_string())),
        "provider" if !rest.is_empty() => Some(SlashCommand::Provider(rest.to_string())),
        "committee" if !rest.is_empty() => CommitteeMode::from_str(rest)
            .ok()
            .map(SlashCommand::Committee),
        "note" if !rest.is_empty() => Some(SlashCommand::Note(rest.to_string())),
        "info" if rest.is_empty() => Some(SlashCommand::Info),
        "lang" if !rest.is_empty() => Locale::from_str(rest).ok().map(SlashCommand::Lang),
        "fork" if !rest.is_empty() => Some(SlashCommand::Fork(rest.to_string())),
        "teammate" if !rest.is_empty() => Some(SlashCommand::Teammate(rest.to_string())),
        "worktree" if !rest.is_empty() => {
            let mut parts = rest.splitn(2, char::is_whitespace);
            let branch = parts.next().unwrap_or("").trim();
            let task = parts.next().unwrap_or("").trim();
            if branch.is_empty() || task.is_empty() {
                None
            } else {
                Some(SlashCommand::Worktree {
                    branch: branch.to_string(),
                    task: task.to_string(),
                })
            }
        }
        "session" if rest == "save" => Some(SlashCommand::SessionSave),
        "session" if rest == "list" => Some(SlashCommand::SessionList),
        "session" if rest.starts_with("new") => {
            let task = rest.strip_prefix("new").unwrap_or("").trim();
            Some(SlashCommand::SessionNew(if task.is_empty() {
                None
            } else {
                Some(task.to_string())
            }))
        }
        "session" if rest.starts_with("switch") => {
            let target = rest.strip_prefix("switch").unwrap_or("").trim();
            if target.is_empty() {
                None
            } else {
                Some(SlashCommand::SessionSwitch(target.to_string()))
            }
        }
        "session" if rest.starts_with("close") => {
            let target = rest.strip_prefix("close").unwrap_or("").trim();
            if target.is_empty() {
                None
            } else {
                Some(SlashCommand::SessionClose(target.to_string()))
            }
        }
        "plan" if rest == "show" => Some(SlashCommand::PlanShow),
        "goal" => match rest {
            "pause" => Some(SlashCommand::GoalPause),
            "resume" => Some(SlashCommand::GoalResume),
            "clear" => Some(SlashCommand::GoalClear),
            "status" => Some(SlashCommand::GoalStatus),
            "" => None,
            goal => Some(SlashCommand::GoalStart(goal.to_string())),
        },
        "subagent" if rest.starts_with("model") => {
            let target = rest.strip_prefix("model").unwrap_or("").trim();
            match target {
                "" => None,
                "reset" => Some(SlashCommand::SubagentModel(SubagentModelChange::Reset)),
                name => Some(SlashCommand::SubagentModel(SubagentModelChange::Set(
                    name.to_string(),
                ))),
            }
        }
        "reasoning" if !rest.is_empty() => {
            ReasoningEffort::parse(rest).map(SlashCommand::Reasoning)
        }
        _ => None,
    }
}
