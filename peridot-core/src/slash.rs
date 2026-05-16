use std::str::FromStr;

use peridot_common::Locale;
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
        "lang" if !rest.is_empty() => Locale::from_str(rest).ok().map(SlashCommand::Lang),
        "session" if rest == "save" => Some(SlashCommand::SessionSave),
        "plan" if rest == "show" => Some(SlashCommand::PlanShow),
        "goal" => match rest {
            "pause" => Some(SlashCommand::GoalPause),
            "resume" => Some(SlashCommand::GoalResume),
            "clear" => Some(SlashCommand::GoalClear),
            "status" => Some(SlashCommand::GoalStatus),
            "" => None,
            goal => Some(SlashCommand::GoalStart(goal.to_string())),
        },
        _ => None,
    }
}
