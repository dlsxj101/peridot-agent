//! `peridot setting` interactive command.
//!
//! Loads the project config (creating it if needed), renders an
//! interactive list of the toggleable / cycleable / numeric options
//! via [`peridot_tui::run_settings_screen`], then writes the mutated
//! values back to `.peridot/config.toml` when the operator saves.
//!
//! Adding a new option means: (1) extend [`settings_registry`] with
//! the field's id, label, group, help text, and a starting
//! [`SettingValue`]; (2) extend [`apply_settings_to_config`] with a
//! match arm on the id that copies the new value back into
//! `PeridotConfig`. The screen UI never needs to change.

use std::fs;
use std::path::Path;

use anyhow::{Context, Result};
use peridot_common::{
    CommitteeMode, ExecutionMode, Locale, PeridotConfig, PermissionMode, ReasoningEffort,
    SandboxMode,
};
use peridot_tui::{SettingItem, SettingValue, SettingsOutcome, run_settings_screen};

use super::OutputFormat;
use super::config::init_project_config_value;
use super::output::print_json_or_text_result;

/// Entry point for `peridot setting`. Reads the project config, opens
/// the interactive screen, and persists the result on save.
pub(crate) fn run_setting_command(project_root: &Path, output: OutputFormat) -> Result<()> {
    let result = init_project_config_value(project_root)?;
    let mut config: PeridotConfig = toml::from_str(
        &fs::read_to_string(&result.config_path)
            .with_context(|| format!("failed to read {}", result.config_path.display()))?,
    )
    .with_context(|| format!("failed to parse {}", result.config_path.display()))?;
    let mut items = settings_registry(&config);
    let outcome =
        run_settings_screen(&mut items).with_context(|| "failed to run settings screen")?;
    match outcome {
        SettingsOutcome::Save => {
            apply_settings_to_config(&items, &mut config);
            fs::write(&result.config_path, toml::to_string_pretty(&config)?)
                .with_context(|| format!("failed to write {}", result.config_path.display()))?;
            print_json_or_text_result(
                serde_json::json!({
                    "config_path": result.config_path,
                    "saved": true,
                }),
                format!("saved {}", result.config_path.display()),
                output,
            )
        }
        SettingsOutcome::Cancel => print_json_or_text_result(
            serde_json::json!({ "saved": false }),
            "no changes saved".to_string(),
            output,
        ),
    }
}

/// Builds the full list of toggleable settings, seeded with the
/// current values from `config`. Stable order is alphabetical within
/// each group; groups are ordered by importance (autonomy first,
/// then defaults, committee, security, git, TUI).
#[allow(clippy::vec_init_then_push)] // 20+ items — sequential pushes are easier to scan than one giant vec![]
pub(crate) fn settings_registry(config: &PeridotConfig) -> Vec<SettingItem> {
    let mut items = Vec::new();

    // === Autonomy loops ===
    items.push(SettingItem {
        id: "defaults.auto_verify_after_mutation".into(),
        group: "Autonomy".into(),
        label: "Auto-verify after every file change".into(),
        help: Some(
            "After every file_write / file_patch, run verify_build automatically so compile errors surface immediately."
                .into(),
        ),
        value: SettingValue::Bool(config.defaults.auto_verify_after_mutation),
    });
    items.push(SettingItem {
        id: "defaults.auto_grade_on_done".into(),
        group: "Autonomy".into(),
        label: "Auto-grade on agent_done".into(),
        help: Some(
            "Before declaring the task done, ask an LLM to grade the change. If it fails, the loop continues with the recommendations injected."
                .into(),
        ),
        value: SettingValue::Bool(config.defaults.auto_grade_on_done),
    });

    // === Defaults ===
    items.push(SettingItem {
        id: "defaults.mode".into(),
        group: "Defaults".into(),
        label: "Default execution mode".into(),
        help: Some(
            "Plan = read-only planning; Execute = normal coding; Goal = long autonomous run".into(),
        ),
        value: SettingValue::Choice {
            options: vec!["plan".into(), "execute".into(), "goal".into()],
            selected: match config.defaults.mode {
                ExecutionMode::Plan => 0,
                ExecutionMode::Execute => 1,
                ExecutionMode::Goal => 2,
            },
        },
    });
    items.push(SettingItem {
        id: "defaults.permission".into(),
        group: "Defaults".into(),
        label: "Default permission posture".into(),
        help: Some(
            "Safe = confirm every write; Auto = confirm only destructive; Yolo = no prompts".into(),
        ),
        value: SettingValue::Choice {
            options: vec!["safe".into(), "auto".into(), "yolo".into()],
            selected: match config.defaults.permission {
                PermissionMode::Safe => 0,
                PermissionMode::Auto => 1,
                PermissionMode::Yolo => 2,
            },
        },
    });
    items.push(SettingItem {
        id: "defaults.max_turns".into(),
        group: "Defaults".into(),
        label: "Max turns per run".into(),
        help: Some("Hard cap on how many model→tool cycles a single task can take.".into()),
        value: SettingValue::U32 {
            value: config.defaults.max_turns,
            min: 1,
            max: 1000,
            step: 10,
        },
    });
    items.push(SettingItem {
        id: "defaults.budget_usd".into(),
        group: "Defaults".into(),
        label: "Budget per run (USD)".into(),
        help: Some("Cost ceiling. 0 disables the cap.".into()),
        value: SettingValue::F64 {
            value: config.defaults.budget_usd,
            min: 0.0,
            max: 100.0,
            step: 0.5,
        },
    });
    items.push(SettingItem {
        id: "defaults.budget_warning_pct".into(),
        group: "Defaults".into(),
        label: "Budget warning at (%)".into(),
        help: Some("Fire a hook when this share of the budget is consumed.".into()),
        value: SettingValue::U32 {
            value: config.defaults.budget_warning_pct as u32,
            min: 0,
            max: 100,
            step: 5,
        },
    });

    // === Committee (multi-agent) ===
    items.push(SettingItem {
        id: "committee.mode".into(),
        group: "Committee".into(),
        label: "Multi-agent committee".into(),
        help: Some(
            "Off = single agent; Planner = run a planner preflight; Full = planner + reviewer per mutating turn."
                .into(),
        ),
        value: SettingValue::Choice {
            options: vec!["off".into(), "planner".into(), "full".into()],
            selected: match config.committee.mode {
                CommitteeMode::Off => 0,
                CommitteeMode::Planner => 1,
                CommitteeMode::Full => 2,
            },
        },
    });
    items.push(SettingItem {
        id: "committee.min_task_chars".into(),
        group: "Committee".into(),
        label: "Skip planner below N chars".into(),
        help: Some("Tasks shorter than this skip the planner preflight. 0 = always run.".into()),
        value: SettingValue::Usize {
            value: config.committee.min_task_chars,
            min: 0,
            max: 2000,
            step: 25,
        },
    });
    items.push(SettingItem {
        id: "committee.max_review_passes".into(),
        group: "Committee".into(),
        label: "Max reviewer re-passes".into(),
        help: Some("After this many consecutive RequestChanges, auto-block the run.".into()),
        value: SettingValue::U32 {
            value: config.committee.max_review_passes,
            min: 1,
            max: 10,
            step: 1,
        },
    });
    items.push(SettingItem {
        id: "committee.use_llm_complexity_gate".into(),
        group: "Committee".into(),
        label: "Let the model decide task complexity".into(),
        help: Some(
            "Classify the task with a capped-output call to the main model before the planner. Skips planning for chat / simple tasks, fires for complex / architectural."
                .into(),
        ),
        value: SettingValue::Bool(config.committee.use_llm_complexity_gate),
    });

    // === Models ===
    items.push(SettingItem {
        id: "models.reasoning_effort".into(),
        group: "Models".into(),
        label: "Reasoning effort".into(),
        help: Some(
            "How hard the model thinks: off / low / medium / high / xhigh (cost grows with depth)."
                .into(),
        ),
        value: SettingValue::Choice {
            options: vec![
                "off".into(),
                "low".into(),
                "medium".into(),
                "high".into(),
                "xhigh".into(),
            ],
            selected: match config.models.reasoning_effort {
                ReasoningEffort::Off => 0,
                ReasoningEffort::Low => 1,
                ReasoningEffort::Medium => 2,
                ReasoningEffort::High => 3,
                ReasoningEffort::XHigh => 4,
            },
        },
    });

    // === Security ===
    items.push(SettingItem {
        id: "security.sandbox".into(),
        group: "Security".into(),
        label: "Sandbox mode".into(),
        help: Some("None = run tools directly; Docker / Firejail = isolate tool execution.".into()),
        value: SettingValue::Choice {
            options: vec!["none".into(), "docker".into(), "firejail".into()],
            selected: match config.security.sandbox {
                SandboxMode::None => 0,
                SandboxMode::Docker => 1,
                SandboxMode::Firejail => 2,
            },
        },
    });
    items.push(SettingItem {
        id: "security.ask_before_install".into(),
        group: "Security".into(),
        label: "Confirm before installing dependencies".into(),
        help: Some("Block `cargo add`, `npm install`, etc. until the operator approves.".into()),
        value: SettingValue::Bool(config.security.ask_before_install),
    });
    items.push(SettingItem {
        id: "security.ask_before_delete".into(),
        group: "Security".into(),
        label: "Confirm before destructive shell commands".into(),
        help: Some(
            "Block `rm`, `git clean`, `git reset --hard`, etc. until the operator approves.".into(),
        ),
        value: SettingValue::Bool(config.security.ask_before_delete),
    });

    // === Git ===
    items.push(SettingItem {
        id: "git.auto_commit".into(),
        group: "Git".into(),
        label: "Auto-commit after each run".into(),
        help: Some("Create a git commit at the end of every successful run.".into()),
        value: SettingValue::Bool(config.git.auto_commit),
    });
    items.push(SettingItem {
        id: "git.auto_branch".into(),
        group: "Git".into(),
        label: "Auto-branch before changes".into(),
        help: Some("Create a topic branch before the agent starts editing.".into()),
        value: SettingValue::Bool(config.git.auto_branch),
    });

    // === TUI ===
    items.push(SettingItem {
        id: "tui.show_thinking".into(),
        group: "TUI".into(),
        label: "Show model thinking".into(),
        help: Some("Display extended-thinking output inline (Goal mode only).".into()),
        value: SettingValue::Bool(config.tui.show_thinking),
    });
    items.push(SettingItem {
        id: "tui.show_token_count".into(),
        group: "TUI".into(),
        label: "Show token count".into(),
        help: Some("Surface running token totals in the header.".into()),
        value: SettingValue::Bool(config.tui.show_token_count),
    });
    items.push(SettingItem {
        id: "tui.show_cost".into(),
        group: "TUI".into(),
        label: "Show running cost".into(),
        help: Some("Surface the estimated USD spend in the header.".into()),
        value: SettingValue::Bool(config.tui.show_cost),
    });
    items.push(SettingItem {
        id: "tui.show_mascot".into(),
        group: "TUI".into(),
        label: "Show the deer mascot".into(),
        help: Some("Toggle the idle-state pixel mascot in the side panel.".into()),
        value: SettingValue::Bool(config.tui.show_mascot),
    });
    items.push(SettingItem {
        id: "tui.language".into(),
        group: "TUI".into(),
        label: "UI language".into(),
        help: Some("Locale used for TUI labels and status text.".into()),
        value: SettingValue::Choice {
            options: vec!["en".into(), "ko".into()],
            selected: tui_language_index(config),
        },
    });

    // === Updates ===
    items.push(SettingItem {
        id: "updates.auto_check".into(),
        group: "Updates".into(),
        label: "Check for updates on launch".into(),
        help: Some("Phone home once at startup to see if a newer Peridot is available.".into()),
        value: SettingValue::Bool(config.updates.auto_check),
    });

    items
}

fn tui_language_index(config: &PeridotConfig) -> usize {
    match config.tui.language {
        Locale::Ko => 1,
        Locale::En => 0,
    }
}

/// Copies values from the mutated `items` slice back into `config`.
/// Unknown ids (from a stale registry, or a future-version config) are
/// silently ignored so the screen stays forward-compatible.
pub(crate) fn apply_settings_to_config(items: &[SettingItem], config: &mut PeridotConfig) {
    for item in items {
        apply_one(item, config);
    }
}

fn apply_one(item: &SettingItem, config: &mut PeridotConfig) {
    match (item.id.as_str(), &item.value) {
        ("defaults.auto_verify_after_mutation", SettingValue::Bool(v)) => {
            config.defaults.auto_verify_after_mutation = *v;
        }
        ("defaults.auto_grade_on_done", SettingValue::Bool(v)) => {
            config.defaults.auto_grade_on_done = *v;
        }
        ("defaults.mode", SettingValue::Choice { options, selected }) => {
            if let Some(label) = options.get(*selected) {
                config.defaults.mode = match label.as_str() {
                    "plan" => ExecutionMode::Plan,
                    "goal" => ExecutionMode::Goal,
                    _ => ExecutionMode::Execute,
                };
            }
        }
        ("defaults.permission", SettingValue::Choice { options, selected }) => {
            if let Some(label) = options.get(*selected) {
                config.defaults.permission = match label.as_str() {
                    "safe" => PermissionMode::Safe,
                    "yolo" => PermissionMode::Yolo,
                    _ => PermissionMode::Auto,
                };
            }
        }
        ("defaults.max_turns", SettingValue::U32 { value, .. }) => {
            config.defaults.max_turns = *value;
        }
        ("defaults.budget_usd", SettingValue::F64 { value, .. }) => {
            config.defaults.budget_usd = *value;
        }
        ("defaults.budget_warning_pct", SettingValue::U32 { value, .. }) => {
            config.defaults.budget_warning_pct = (*value).min(100) as u8;
        }
        ("committee.mode", SettingValue::Choice { options, selected }) => {
            if let Some(label) = options.get(*selected) {
                config.committee.mode = match label.as_str() {
                    "planner" => CommitteeMode::Planner,
                    "full" => CommitteeMode::Full,
                    _ => CommitteeMode::Off,
                };
            }
        }
        ("committee.min_task_chars", SettingValue::Usize { value, .. }) => {
            config.committee.min_task_chars = *value;
        }
        ("committee.max_review_passes", SettingValue::U32 { value, .. }) => {
            config.committee.max_review_passes = *value;
        }
        ("committee.use_llm_complexity_gate", SettingValue::Bool(v)) => {
            config.committee.use_llm_complexity_gate = *v;
        }
        ("models.reasoning_effort", SettingValue::Choice { options, selected }) => {
            if let Some(label) = options.get(*selected) {
                config.models.reasoning_effort =
                    ReasoningEffort::parse(label).unwrap_or(ReasoningEffort::Off);
            }
        }
        ("security.sandbox", SettingValue::Choice { options, selected }) => {
            if let Some(label) = options.get(*selected) {
                config.security.sandbox = match label.as_str() {
                    "docker" => SandboxMode::Docker,
                    "firejail" => SandboxMode::Firejail,
                    _ => SandboxMode::None,
                };
            }
        }
        ("security.ask_before_install", SettingValue::Bool(v)) => {
            config.security.ask_before_install = *v;
        }
        ("security.ask_before_delete", SettingValue::Bool(v)) => {
            config.security.ask_before_delete = *v;
        }
        ("git.auto_commit", SettingValue::Bool(v)) => {
            config.git.auto_commit = *v;
        }
        ("git.auto_branch", SettingValue::Bool(v)) => {
            config.git.auto_branch = *v;
        }
        ("tui.show_thinking", SettingValue::Bool(v)) => {
            config.tui.show_thinking = *v;
        }
        ("tui.show_token_count", SettingValue::Bool(v)) => {
            config.tui.show_token_count = *v;
        }
        ("tui.show_cost", SettingValue::Bool(v)) => {
            config.tui.show_cost = *v;
        }
        ("tui.show_mascot", SettingValue::Bool(v)) => {
            config.tui.show_mascot = *v;
        }
        ("tui.language", SettingValue::Choice { options, selected }) => {
            if let Some(label) = options.get(*selected) {
                config.tui.language = match label.as_str() {
                    "ko" => Locale::Ko,
                    _ => Locale::En,
                };
            }
        }
        ("updates.auto_check", SettingValue::Bool(v)) => {
            config.updates.auto_check = *v;
        }
        _ => {} // unknown id or type mismatch — leave config untouched.
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_seeds_from_current_config() {
        let mut config = PeridotConfig::default();
        config.defaults.auto_verify_after_mutation = true;
        config.committee.mode = CommitteeMode::Full;
        config.defaults.max_turns = 42;
        let items = settings_registry(&config);

        let verify = items
            .iter()
            .find(|i| i.id == "defaults.auto_verify_after_mutation")
            .expect("auto-verify item present");
        assert_eq!(verify.value, SettingValue::Bool(true));

        let committee = items
            .iter()
            .find(|i| i.id == "committee.mode")
            .expect("committee.mode item present");
        if let SettingValue::Choice { selected, options } = &committee.value {
            assert_eq!(options.get(*selected).map(String::as_str), Some("full"));
        } else {
            panic!("expected Choice for committee.mode");
        }

        let turns = items
            .iter()
            .find(|i| i.id == "defaults.max_turns")
            .expect("max_turns item present");
        if let SettingValue::U32 { value, .. } = turns.value {
            assert_eq!(value, 42);
        } else {
            panic!("expected U32 for max_turns");
        }
    }

    #[test]
    fn apply_round_trips_mutations() {
        let mut config = PeridotConfig::default();
        let mut items = settings_registry(&config);

        for item in items.iter_mut() {
            match (item.id.as_str(), &mut item.value) {
                ("defaults.auto_verify_after_mutation", SettingValue::Bool(v)) => *v = true,
                ("defaults.auto_grade_on_done", SettingValue::Bool(v)) => *v = true,
                (
                    "committee.mode",
                    SettingValue::Choice {
                        options, selected, ..
                    },
                ) => {
                    *selected = options.iter().position(|o| o == "planner").unwrap();
                }
                ("defaults.max_turns", SettingValue::U32 { value, .. }) => *value = 7,
                ("defaults.budget_usd", SettingValue::F64 { value, .. }) => *value = 12.5,
                ("security.ask_before_install", SettingValue::Bool(v)) => *v = false,
                _ => {}
            }
        }
        apply_settings_to_config(&items, &mut config);

        assert!(config.defaults.auto_verify_after_mutation);
        assert!(config.defaults.auto_grade_on_done);
        assert_eq!(config.committee.mode, CommitteeMode::Planner);
        assert_eq!(config.defaults.max_turns, 7);
        assert!((config.defaults.budget_usd - 12.5).abs() < 1e-9);
        assert!(!config.security.ask_before_install);
    }

    #[test]
    fn apply_ignores_unknown_ids() {
        let mut config = PeridotConfig::default();
        let items = vec![SettingItem {
            id: "totally.bogus.field".into(),
            group: "x".into(),
            label: "x".into(),
            help: None,
            value: SettingValue::Bool(true),
        }];
        // Should not panic and should not touch defaults.
        let snapshot = config.clone();
        apply_settings_to_config(&items, &mut config);
        assert_eq!(
            config.defaults.auto_verify_after_mutation,
            snapshot.defaults.auto_verify_after_mutation
        );
    }
}
