use std::str::FromStr;

use peridot_common::{CommitteeMode, ExecutionMode, Locale, PermissionMode, ReasoningEffort};
use serde::{Deserialize, Serialize};

/// Slash commands supported by Peridot's interactive surfaces.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum SlashCommand {
    /// Invoke a stored auto-skill by its kebab-case name (Hermes-style
    /// `/skill-name [args]`). The dispatcher looks up the skill body
    /// in the project's `MemoryStore` and injects it as a
    /// `PlanReminder` context entry; if no such skill exists, the
    /// dispatcher surfaces a "skill not found" message. The slash
    /// parser turns *any* otherwise-unknown kebab-case command into
    /// this variant, so typos and unrecognised commands now hit the
    /// skill store lookup before being rejected.
    Skill {
        /// Skill name without the leading slash — the
        /// `store.search_skills` query key.
        name: String,
        /// Free-form trailing text after the skill name. Passed
        /// through to the dispatcher so commands like `/ship-daily
        /// --dry` keep their arguments.
        args: String,
    },
    /// List active stored skills available to slash invocation.
    SkillList,
    /// Show details for an active stored skill.
    SkillShow(String),
    /// Pin an active stored skill so automated curation cannot archive it.
    SkillPin(String),
    /// Clear the pinned marker from an active stored skill.
    SkillUnpin(String),
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
    /// Attach a workspace file to the current session context.
    Attach(String),
    /// Print a one-shot summary of the current session (model, provider,
    /// workspace, session id, mode, permission, turn, tokens, cost).
    Info,
    /// Show which context entries currently consume the most estimated tokens.
    ContextTop,
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
    /// Delete a session by id or 1-based index.
    SessionDelete(String),
    /// Rename a session.
    SessionRename {
        /// Session id, title, or index to rename.
        target: String,
        /// New display title.
        title: String,
    },
    /// List all known sessions in the transcript.
    SessionList,
    /// Show persisted session lifecycle counts.
    SessionCount,
    /// Override the default model used when spawning sub-agents. `reset`
    /// clears the override so future spawns inherit the caller's main model.
    SubagentModel(SubagentModelChange),
    /// Change the reasoning intensity applied to every model request.
    /// Cheap models without a reasoning channel ignore the setting.
    Reasoning(ReasoningEffort),
    /// Toggle provider fast/priority service tier for the current session.
    Fast(Option<bool>),
    /// List MCP server entries currently configured in `config.toml`.
    McpList,
    /// Append a new MCP server entry to `config.toml`. The host loop
    /// persists the new config and asks the user to restart the session
    /// (or peridot) for the change to take effect.
    McpAdd {
        /// Server name (must be unique in the config).
        name: String,
        /// Transport kind: `stdio` or `http`.
        transport: String,
        /// Free-form connection target — interpreted per transport. For
        /// `stdio` this is the command (optionally with `arg arg ...`); for
        /// `http` it is the SSE / HTTP endpoint URL.
        target: String,
    },
    /// Remove the named MCP server entry from `config.toml`.
    McpRemove(String),
    /// Spawn a one-shot connectivity test against the named MCP server,
    /// reporting tool count / failure in the transcript.
    McpTest(String),
    /// Scan the project for TODO / FIXME / HACK / XXX / BUG comments and
    /// list every hit in the transcript. Ad-hoc — no persistent index.
    Todos,
    /// Show a lightweight workspace code map: public symbols plus TODO markers.
    CodeMap,
    /// Show whether the persisted workspace code map index is present or stale.
    CodeMapStatus,
    /// Rebuild the persisted workspace code map index.
    CodeMapRefresh,
    /// Search the persisted workspace code map index.
    CodeMapFind(String),
    /// Locate symbol definitions from the persisted workspace code map index.
    CodeMapLocate(String),
    /// List indexed symbols in one workspace file.
    CodeMapOutline(String),
    /// Find textual references for an indexed symbol.
    CodeMapRefs(String),
    /// List attachment artifacts already loaded into the current session context.
    Attachments,
    /// Remove attachment artifacts from the current session context by path.
    Detach(String),
    /// Export session artifacts into `.peridot/exports`.
    Export(Vec<ExportArtifact>),
    /// Pop the last user-agent exchange off the visible transcript and
    /// reload the user's previous prompt into the input buffer so the
    /// operator can edit and re-submit. Context is NOT rolled back — the
    /// model still sees the prior turns on the next call. A pragmatic
    /// "let me try that again" gesture, not a semantic rewind.
    Rewind,
    /// Save the current session context snapshot under `name` so it can
    /// be restored later via `/branch restore <name>`. Snapshots live
    /// under `.peridot/branches/<name>/`.
    BranchSave(String),
    /// Restore a previously-saved branch by name. Only valid when the
    /// agent is idle — overwrites the working context snapshot.
    BranchRestore(String),
    /// List every named branch saved under `.peridot/branches/`.
    BranchList,
    /// Fork the conversation at a specific past turn id. Truncates the
    /// context to entries from turns `<= turn_id` and records the
    /// lineage so future turns are stamped with `parent_turn_id =
    /// turn_id`. The dropped entries are surfaced to the caller so the
    /// session DAG can persist the abandoned limb.
    BranchTurn(u64),
    /// `/branch tree` — print the DAG journal showing all abandoned limbs.
    BranchTree,
    /// `/branch switch <index>` — swap the active path with a saved limb
    /// from the DAG journal. The current path becomes a new limb and the
    /// selected limb's entries are appended to the snapshot.
    BranchSwitch(usize),
    /// `/branch` with no args — open the interactive branch picker.
    /// The TUI overlay calls back into BranchTurn once the operator
    /// picks a turn.
    BranchPicker,
    /// `/sidepanel` — toggle the right-hand Status panel on/off. A
    /// terminal-agnostic alternative to `Ctrl+]` / `F2` for the same
    /// action; useful when the host terminal (WSL conpty has been the
    /// case in practice) doesn't deliver Ctrl+] reliably.
    SidepanelToggle,
    /// `/collapse` — toggle global collapse of tool/diff transcript blocks.
    Collapse,
    /// `/autofix` — toggle or configure the auto-fix loop.
    /// `on` / `off` enables or disables; a bare number sets max attempts.
    AutoFix(AutoFixAction),
}

/// Artifact classes accepted by `/export`.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum ExportArtifact {
    /// Full persisted session directory.
    Full,
    /// Reconstructed session attachments.
    Attachments,
    /// Operator notes.
    Notes,
    /// Unified replay timeline.
    Timeline,
}

impl FromStr for ExportArtifact {
    type Err = ();

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value.to_ascii_lowercase().as_str() {
            "full" => Ok(Self::Full),
            "attachments" | "attachment" => Ok(Self::Attachments),
            "notes" | "note" => Ok(Self::Notes),
            "timeline" | "replay" => Ok(Self::Timeline),
            _ => Err(()),
        }
    }
}

/// Canonical state mutation implied by a slash command.
///
/// Front-ends should derive run-option changes from this value instead of
/// re-parsing command strings locally. Commands that only render local UI
/// or enqueue host actions return an empty delta.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct SlashStateDelta {
    /// Execution mode override.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mode: Option<ExecutionMode>,
    /// Permission mode override.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub permission: Option<PermissionMode>,
    /// Main model override.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// Provider override.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    /// Reasoning effort override.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning_effort: Option<ReasoningEffort>,
    /// Service tier override. `Some(None)` means reset to standard/default.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub service_tier: Option<Option<String>>,
    /// Committee mode override.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub committee_mode: Option<CommitteeMode>,
    /// Locale override.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub locale: Option<Locale>,
    /// Subagent default model override. `Some(None)` means reset/inherit.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subagent_default_model: Option<Option<String>>,
}

impl SlashStateDelta {
    /// Returns true when the delta contains no state changes.
    pub fn is_empty(&self) -> bool {
        self == &Self::default()
    }
}

/// Computes the canonical state delta for `command`.
pub fn slash_state_delta(
    command: &SlashCommand,
    current_service_tier: Option<&str>,
) -> SlashStateDelta {
    match command {
        SlashCommand::Plan => SlashStateDelta {
            mode: Some(ExecutionMode::Plan),
            ..SlashStateDelta::default()
        },
        SlashCommand::Execute => SlashStateDelta {
            mode: Some(ExecutionMode::Execute),
            ..SlashStateDelta::default()
        },
        SlashCommand::GoalStart(_) => SlashStateDelta {
            mode: Some(ExecutionMode::Goal),
            ..SlashStateDelta::default()
        },
        SlashCommand::Safe => SlashStateDelta {
            permission: Some(PermissionMode::Safe),
            ..SlashStateDelta::default()
        },
        SlashCommand::Auto => SlashStateDelta {
            permission: Some(PermissionMode::Auto),
            ..SlashStateDelta::default()
        },
        SlashCommand::Yolo => SlashStateDelta {
            permission: Some(PermissionMode::Yolo),
            ..SlashStateDelta::default()
        },
        SlashCommand::Model(model) => SlashStateDelta {
            model: Some(model.clone()),
            ..SlashStateDelta::default()
        },
        SlashCommand::Provider(provider) => SlashStateDelta {
            provider: Some(provider.clone()),
            ..SlashStateDelta::default()
        },
        SlashCommand::Reasoning(effort) => SlashStateDelta {
            reasoning_effort: Some(*effort),
            ..SlashStateDelta::default()
        },
        SlashCommand::Fast(change) => {
            let enabled = change.unwrap_or_else(|| current_service_tier != Some("fast"));
            SlashStateDelta {
                service_tier: Some(enabled.then(|| "fast".to_string())),
                ..SlashStateDelta::default()
            }
        }
        SlashCommand::Committee(mode) => SlashStateDelta {
            committee_mode: Some(*mode),
            ..SlashStateDelta::default()
        },
        SlashCommand::Lang(locale) => SlashStateDelta {
            locale: Some(*locale),
            ..SlashStateDelta::default()
        },
        SlashCommand::SubagentModel(change) => SlashStateDelta {
            subagent_default_model: Some(match change {
                SubagentModelChange::Set(model) => Some(model.clone()),
                SubagentModelChange::Reset => None,
            }),
            ..SlashStateDelta::default()
        },
        _ => SlashStateDelta::default(),
    }
}

/// Payload for `/autofix`.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum AutoFixAction {
    /// Toggle on (with default max attempts).
    On,
    /// Toggle off.
    Off,
    /// Set max attempts (and enable).
    MaxAttempts(u32),
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
        "skills" if rest.is_empty() || rest == "list" => Some(SlashCommand::SkillList),
        "skills" if rest.starts_with("show ") => {
            let name = rest.strip_prefix("show ")?.trim();
            (!name.is_empty()).then(|| SlashCommand::SkillShow(name.to_string()))
        }
        "skills" if rest.starts_with("view ") => {
            let name = rest.strip_prefix("view ")?.trim();
            (!name.is_empty()).then(|| SlashCommand::SkillShow(name.to_string()))
        }
        "skills" if rest.starts_with("pin ") => {
            let name = rest.strip_prefix("pin ")?.trim();
            (!name.is_empty()).then(|| SlashCommand::SkillPin(name.to_string()))
        }
        "skills" if rest.starts_with("unpin ") => {
            let name = rest.strip_prefix("unpin ")?.trim();
            (!name.is_empty()).then(|| SlashCommand::SkillUnpin(name.to_string()))
        }
        "skills" => None,
        "cost" if rest.is_empty() => Some(SlashCommand::Cost),
        "compact" if rest.is_empty() => Some(SlashCommand::Compact),
        "sidepanel" if rest.is_empty() => Some(SlashCommand::SidepanelToggle),
        "status" if rest.is_empty() => Some(SlashCommand::SidepanelToggle),
        "diff" if rest.is_empty() => Some(SlashCommand::Diff),
        "undo" if rest.is_empty() => Some(SlashCommand::Undo),
        "model" if !rest.is_empty() => Some(SlashCommand::Model(rest.to_string())),
        "provider" if !rest.is_empty() => Some(SlashCommand::Provider(rest.to_string())),
        "committee" if !rest.is_empty() => CommitteeMode::from_str(rest)
            .ok()
            .map(SlashCommand::Committee),
        "note" if !rest.is_empty() => Some(SlashCommand::Note(rest.to_string())),
        "attach" if !rest.is_empty() => Some(SlashCommand::Attach(rest.to_string())),
        "attach" => None,
        "detach" if !rest.is_empty() => Some(SlashCommand::Detach(rest.to_string())),
        "detach" => None,
        "info" if rest.is_empty() => Some(SlashCommand::Info),
        "context" if rest.is_empty() || rest == "top" => Some(SlashCommand::ContextTop),
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
        "session" if rest == "count" => Some(SlashCommand::SessionCount),
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
        "session" if rest.starts_with("delete") => {
            let target = rest.strip_prefix("delete").unwrap_or("").trim();
            if target.is_empty() {
                None
            } else {
                Some(SlashCommand::SessionDelete(target.to_string()))
            }
        }
        "session" if rest.starts_with("rename") => {
            let payload = rest.strip_prefix("rename").unwrap_or("").trim();
            let mut parts = payload.splitn(2, char::is_whitespace);
            let target = parts.next().unwrap_or("").trim();
            let title = parts.next().unwrap_or("").trim();
            if target.is_empty() || title.is_empty() {
                None
            } else {
                Some(SlashCommand::SessionRename {
                    target: target.to_string(),
                    title: title.to_string(),
                })
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
        "fast" => match rest {
            "" | "on" | "true" | "1" => Some(SlashCommand::Fast(Some(true))),
            "off" | "false" | "0" | "standard" => Some(SlashCommand::Fast(Some(false))),
            "toggle" => Some(SlashCommand::Fast(None)),
            _ => None,
        },
        // `/think` and `/think hard` map to the High reasoning tier; `/think
        // off` clears it. A convenient alias for users who think in terms of
        // "make the model think harder" instead of the dial vocabulary.
        "think" => match rest {
            "" | "hard" | "harder" | "more" => Some(SlashCommand::Reasoning(ReasoningEffort::High)),
            "off" | "stop" | "less" => Some(SlashCommand::Reasoning(ReasoningEffort::Off)),
            other => ReasoningEffort::parse(other).map(SlashCommand::Reasoning),
        },
        "collapse" if rest.is_empty() => Some(SlashCommand::Collapse),
        "autofix" => match rest {
            "" | "on" | "true" | "1" => Some(SlashCommand::AutoFix(AutoFixAction::On)),
            "off" | "false" | "0" => Some(SlashCommand::AutoFix(AutoFixAction::Off)),
            n => n
                .parse::<u32>()
                .ok()
                .map(|max| SlashCommand::AutoFix(AutoFixAction::MaxAttempts(max))),
        },
        "todos" if rest.is_empty() => Some(SlashCommand::Todos),
        "codemap" if rest.is_empty() => Some(SlashCommand::CodeMap),
        "codemap" if rest == "status" => Some(SlashCommand::CodeMapStatus),
        "codemap" if rest == "refresh" => Some(SlashCommand::CodeMapRefresh),
        "codemap" if rest.starts_with("find ") => {
            let query = rest.strip_prefix("find ").unwrap_or("").trim();
            if query.is_empty() {
                None
            } else {
                Some(SlashCommand::CodeMapFind(query.to_string()))
            }
        }
        "codemap" if rest.starts_with("locate ") => {
            let query = rest.strip_prefix("locate ").unwrap_or("").trim();
            if query.is_empty() {
                None
            } else {
                Some(SlashCommand::CodeMapLocate(query.to_string()))
            }
        }
        "codemap" if rest.starts_with("outline ") => {
            let path = rest.strip_prefix("outline ").unwrap_or("").trim();
            if path.is_empty() {
                None
            } else {
                Some(SlashCommand::CodeMapOutline(path.to_string()))
            }
        }
        "codemap" if rest.starts_with("refs ") => {
            let query = rest.strip_prefix("refs ").unwrap_or("").trim();
            if query.is_empty() {
                None
            } else {
                Some(SlashCommand::CodeMapRefs(query.to_string()))
            }
        }
        "codemap" => None,
        "attachments" if rest.is_empty() => Some(SlashCommand::Attachments),
        "attachments" => None,
        "export" => parse_export_artifacts(rest).map(SlashCommand::Export),
        "rewind" if rest.is_empty() => Some(SlashCommand::Rewind),
        "branch" => match rest {
            "" => Some(SlashCommand::BranchPicker),
            "list" => Some(SlashCommand::BranchList),
            "tree" => Some(SlashCommand::BranchTree),
            other if other.starts_with("save ") => {
                let name = other.strip_prefix("save ").unwrap_or("").trim();
                if name.is_empty() {
                    None
                } else {
                    Some(SlashCommand::BranchSave(name.to_string()))
                }
            }
            other if other.starts_with("restore ") => {
                let name = other.strip_prefix("restore ").unwrap_or("").trim();
                if name.is_empty() {
                    None
                } else {
                    Some(SlashCommand::BranchRestore(name.to_string()))
                }
            }
            other if other.starts_with("turn ") => {
                let id = other.strip_prefix("turn ").unwrap_or("").trim();
                id.parse::<u64>().ok().map(SlashCommand::BranchTurn)
            }
            other if other.starts_with("switch ") => {
                let idx = other.strip_prefix("switch ").unwrap_or("").trim();
                idx.parse::<usize>().ok().map(SlashCommand::BranchSwitch)
            }
            _ => None,
        },
        "mcp" => match rest {
            "list" => Some(SlashCommand::McpList),
            "" => None,
            other if other.starts_with("add ") => {
                // `/mcp add <name> <transport> <target...>` — split once
                // after the leading "add ", then once more on name boundary,
                // then once more on transport boundary so the remainder
                // (which may itself contain spaces) becomes `target`.
                let rest = other.strip_prefix("add ").unwrap_or("").trim();
                let mut parts = rest.splitn(3, char::is_whitespace);
                let name = parts.next().unwrap_or("").trim().to_string();
                let transport = parts.next().unwrap_or("").trim().to_string();
                let target = parts.next().unwrap_or("").trim().to_string();
                if name.is_empty() || transport.is_empty() || target.is_empty() {
                    None
                } else {
                    Some(SlashCommand::McpAdd {
                        name,
                        transport,
                        target,
                    })
                }
            }
            other if other.starts_with("remove ") => {
                let name = other.strip_prefix("remove ").unwrap_or("").trim();
                if name.is_empty() {
                    None
                } else {
                    Some(SlashCommand::McpRemove(name.to_string()))
                }
            }
            other if other.starts_with("test ") => {
                let name = other.strip_prefix("test ").unwrap_or("").trim();
                if name.is_empty() {
                    None
                } else {
                    Some(SlashCommand::McpTest(name.to_string()))
                }
            }
            _ => None,
        },
        // Fall-through: any otherwise-unrecognised command whose name
        // looks like a kebab-case identifier (lowercase letters /
        // digits / hyphens, starting with a letter, length >= 2) is
        // treated as a stored-skill invocation. The dispatcher decides
        // whether such a skill actually exists; the parser only
        // commits to the *shape*. Typed garbage like `/!@#` still
        // returns `None` so the existing "invalid slash" path
        // continues to handle malformed input.
        other if looks_like_skill_name(other) => Some(SlashCommand::Skill {
            name: other.to_string(),
            args: rest.to_string(),
        }),
        _ => None,
    }
}

fn parse_export_artifacts(rest: &str) -> Option<Vec<ExportArtifact>> {
    if rest.is_empty() {
        return Some(vec![
            ExportArtifact::Attachments,
            ExportArtifact::Notes,
            ExportArtifact::Timeline,
        ]);
    }
    let mut artifacts = Vec::new();
    for token in rest.split_whitespace() {
        let artifact = ExportArtifact::from_str(token).ok()?;
        if !artifacts.contains(&artifact) {
            artifacts.push(artifact);
        }
    }
    Some(artifacts)
}

/// Returns true when `name` looks like a kebab-case skill identifier.
/// Required because the parser uses it as the fallback gate — only
/// strings that match this shape can be routed to skill lookup. Live
/// kebab-case examples from `save_auto_skill`:
/// `auto-fix-parser-tests`, `auto-add-cli-flag`.
fn looks_like_skill_name(s: &str) -> bool {
    if s.len() < 2 {
        return false;
    }
    let mut chars = s.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if !first.is_ascii_lowercase() {
        return false;
    }
    s.chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        && !s.ends_with('-')
        && !s.contains("--")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn looks_like_skill_name_accepts_kebab_case() {
        assert!(looks_like_skill_name("auto-fix-parser"));
        assert!(looks_like_skill_name("ship-daily"));
        assert!(looks_like_skill_name("a1"));
    }

    #[test]
    fn looks_like_skill_name_rejects_invalid_shapes() {
        assert!(!looks_like_skill_name("")); // empty
        assert!(!looks_like_skill_name("x")); // too short
        assert!(!looks_like_skill_name("X-foo")); // uppercase
        assert!(!looks_like_skill_name("foo_bar")); // underscore
        assert!(!looks_like_skill_name("foo--bar")); // double dash
        assert!(!looks_like_skill_name("trailing-")); // trailing dash
        assert!(!looks_like_skill_name("9foo")); // leading digit
    }

    #[test]
    fn parse_unknown_kebab_command_resolves_to_skill() {
        let cmd = parse_slash_command("/ship-daily").unwrap();
        assert!(matches!(cmd, SlashCommand::Skill { ref name, ref args }
            if name == "ship-daily" && args.is_empty()));
    }

    #[test]
    fn parse_unknown_kebab_with_args_preserves_args() {
        let cmd = parse_slash_command("/ship-daily --dry").unwrap();
        let SlashCommand::Skill { name, args } = cmd else {
            panic!("expected Skill variant");
        };
        assert_eq!(name, "ship-daily");
        assert_eq!(args, "--dry");
    }

    #[test]
    fn parse_built_in_takes_priority_over_skill_lookup() {
        // `/help` is a built-in; it must NOT be returned as
        // Skill { name: "help" } even though the name technically
        // satisfies `looks_like_skill_name`.
        let cmd = parse_slash_command("/help").unwrap();
        assert!(matches!(cmd, SlashCommand::Help));
    }

    #[test]
    fn parse_skills_builtin_takes_priority_over_skill_lookup() {
        assert_eq!(
            parse_slash_command("/skills"),
            Some(SlashCommand::SkillList)
        );
        assert_eq!(
            parse_slash_command("/skills list"),
            Some(SlashCommand::SkillList)
        );
        assert_eq!(
            parse_slash_command("/skills show auto-fix-parser"),
            Some(SlashCommand::SkillShow("auto-fix-parser".to_string()))
        );
        assert_eq!(
            parse_slash_command("/skills view auto-fix-parser"),
            Some(SlashCommand::SkillShow("auto-fix-parser".to_string()))
        );
        assert_eq!(
            parse_slash_command("/skills pin auto-fix-parser"),
            Some(SlashCommand::SkillPin("auto-fix-parser".to_string()))
        );
        assert_eq!(
            parse_slash_command("/skills unpin auto-fix-parser"),
            Some(SlashCommand::SkillUnpin("auto-fix-parser".to_string()))
        );
        assert_eq!(parse_slash_command("/skills bogus"), None);
        assert_eq!(parse_slash_command("/skills show"), None);
        assert_eq!(parse_slash_command("/skills pin"), None);
    }

    #[test]
    fn parses_codemap_builtin() {
        assert_eq!(parse_slash_command("/codemap"), Some(SlashCommand::CodeMap));
        assert_eq!(
            parse_slash_command("/codemap status"),
            Some(SlashCommand::CodeMapStatus)
        );
        assert_eq!(
            parse_slash_command("/codemap refresh"),
            Some(SlashCommand::CodeMapRefresh)
        );
        assert_eq!(
            parse_slash_command("/codemap find runner"),
            Some(SlashCommand::CodeMapFind("runner".to_string()))
        );
        assert_eq!(
            parse_slash_command("/codemap locate Runner"),
            Some(SlashCommand::CodeMapLocate("Runner".to_string()))
        );
        assert_eq!(
            parse_slash_command("/codemap outline src/lib.rs"),
            Some(SlashCommand::CodeMapOutline("src/lib.rs".to_string()))
        );
        assert_eq!(
            parse_slash_command("/codemap refs Runner"),
            Some(SlashCommand::CodeMapRefs("Runner".to_string()))
        );
        assert_eq!(parse_slash_command("/codemap find   "), None);
        assert_eq!(parse_slash_command("/codemap locate   "), None);
        assert_eq!(parse_slash_command("/codemap outline   "), None);
        assert_eq!(parse_slash_command("/codemap refs   "), None);
        assert_eq!(parse_slash_command("/codemap src"), None);
    }

    #[test]
    fn parses_attach_with_path() {
        assert_eq!(
            parse_slash_command("/attach src/lib.rs"),
            Some(SlashCommand::Attach("src/lib.rs".to_string()))
        );
        assert_eq!(parse_slash_command("/attach"), None);
        assert_eq!(
            parse_slash_command("/detach src/lib.rs"),
            Some(SlashCommand::Detach("src/lib.rs".to_string()))
        );
        assert_eq!(parse_slash_command("/detach"), None);
    }

    #[test]
    fn parses_attachments_builtin() {
        assert_eq!(
            parse_slash_command("/attachments"),
            Some(SlashCommand::Attachments)
        );
        assert_eq!(parse_slash_command("/attachments now"), None);
    }

    #[test]
    fn parses_export_builtin() {
        assert_eq!(
            parse_slash_command("/export"),
            Some(SlashCommand::Export(vec![
                ExportArtifact::Attachments,
                ExportArtifact::Notes,
                ExportArtifact::Timeline,
            ]))
        );
        assert_eq!(
            parse_slash_command("/export full notes notes"),
            Some(SlashCommand::Export(vec![
                ExportArtifact::Full,
                ExportArtifact::Notes,
            ]))
        );
        assert_eq!(parse_slash_command("/export bad"), None);
    }

    #[test]
    fn parse_malformed_garbage_still_returns_none() {
        // Non-skill-shaped strings must keep returning None so the
        // existing "invalid slash command" error path still fires.
        assert!(parse_slash_command("/Foo").is_none());
        assert!(parse_slash_command("/!@#").is_none());
        assert!(parse_slash_command("/").is_none());
    }

    #[test]
    fn parses_branch_turn_with_valid_id() {
        assert_eq!(
            parse_slash_command("/branch turn 42"),
            Some(SlashCommand::BranchTurn(42))
        );
    }

    #[test]
    fn rejects_branch_turn_with_non_numeric_id() {
        assert_eq!(parse_slash_command("/branch turn abc"), None);
    }

    #[test]
    fn rejects_branch_turn_with_missing_id() {
        assert_eq!(parse_slash_command("/branch turn"), None);
    }

    #[test]
    fn parses_branch_tree() {
        assert_eq!(
            parse_slash_command("/branch tree"),
            Some(SlashCommand::BranchTree)
        );
    }

    #[test]
    fn parses_branch_switch_with_valid_index() {
        assert_eq!(
            parse_slash_command("/branch switch 2"),
            Some(SlashCommand::BranchSwitch(2))
        );
    }

    #[test]
    fn rejects_branch_switch_with_non_numeric() {
        assert_eq!(parse_slash_command("/branch switch abc"), None);
    }

    #[test]
    fn parses_session_delete_and_rename() {
        assert_eq!(
            parse_slash_command("/session count"),
            Some(SlashCommand::SessionCount)
        );
        assert_eq!(
            parse_slash_command("/session delete s1"),
            Some(SlashCommand::SessionDelete("s1".to_string()))
        );
        assert_eq!(
            parse_slash_command("/session rename s1 release prep"),
            Some(SlashCommand::SessionRename {
                target: "s1".to_string(),
                title: "release prep".to_string(),
            })
        );
        assert_eq!(parse_slash_command("/session rename s1"), None);
    }

    #[test]
    fn parses_autofix_on_off_max() {
        assert_eq!(
            parse_slash_command("/autofix"),
            Some(SlashCommand::AutoFix(AutoFixAction::On))
        );
        assert_eq!(
            parse_slash_command("/autofix off"),
            Some(SlashCommand::AutoFix(AutoFixAction::Off))
        );
        assert_eq!(
            parse_slash_command("/autofix 5"),
            Some(SlashCommand::AutoFix(AutoFixAction::MaxAttempts(5)))
        );
    }

    #[test]
    fn parses_xhigh_reasoning_aliases() {
        assert_eq!(
            parse_slash_command("/reasoning xhigh"),
            Some(SlashCommand::Reasoning(ReasoningEffort::XHigh))
        );
        assert_eq!(
            parse_slash_command("/think x-high"),
            Some(SlashCommand::Reasoning(ReasoningEffort::XHigh))
        );
    }

    #[test]
    fn computes_fast_toggle_from_current_tier() {
        assert_eq!(
            slash_state_delta(&SlashCommand::Fast(None), None).service_tier,
            Some(Some("fast".to_string()))
        );
        assert_eq!(
            slash_state_delta(&SlashCommand::Fast(None), Some("fast")).service_tier,
            Some(None)
        );
    }
}
