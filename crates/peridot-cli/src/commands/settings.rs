//! `peridot setting` interactive command + cross-surface settings registry.
//!
//! Loads the project config (creating it if needed), renders an
//! interactive list of the toggleable / cycleable / numeric options
//! via [`peridot_tui::run_settings_screen`], then writes the mutated
//! values back to `.peridot/config.toml` when the operator saves.
//!
//! The same [`settings_registry`] feeds the daemon's `settings.list`
//! RPC so the VS Code webview shares a single definition; group, label,
//! and help strings come from [`super::settings_i18n`] so swapping
//! languages just means rebuilding the registry under a different
//! locale. `surfaces` on each item lets non-TUI clients filter out rows
//! that wouldn't do anything if toggled there (`tui.show_mascot`, …).
//!
//! Adding a new option: (1) extend [`settings_registry`] with the
//! field's id, surfaces, and a [`SettingValue`] seed; (2) extend
//! [`apply_settings_to_config`] with a match arm that copies the new
//! value back into `PeridotConfig`; (3) add a translation entry in
//! [`super::settings_i18n`]. The screen UI never needs to change.

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
use super::settings_i18n::{LocalizedSetting, fallback as i18n_fallback, lookup as i18n_lookup};

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

/// Surface tags. Centralised so registry callsites don't drift between
/// `"vscode"` and `"vs-code"` etc. Renderers compare by string anyway,
/// but keeping these constants makes a typo a compile error rather
/// than a silently-empty webview.
const SURFACE_TUI: &str = "tui";
const SURFACE_VSCODE: &str = "vscode";

fn surfaces_all() -> Vec<String> {
    vec![SURFACE_TUI.to_string(), SURFACE_VSCODE.to_string()]
}

fn surfaces_tui_only() -> Vec<String> {
    vec![SURFACE_TUI.to_string()]
}

/// Build a [`SettingItem`] with translated group/label/help fetched
/// from the i18n table. Falls back to the id as the label when no
/// translation entry exists — this keeps the page rendering even when
/// a freshly-added field hasn't been translated yet.
fn make_item(id: &str, surfaces: Vec<String>, value: SettingValue, locale: Locale) -> SettingItem {
    // Two-step lookup: try the id in the requested locale; if that
    // returns None (newly added setting whose translation is still
    // missing), call into `fallback` for the static "(missing
    // translation)" placeholder. The label is then overridden with
    // the id so operators can see *which* field is missing in
    // production without crashing — surfacing the gap is more useful
    // than hiding it.
    let (strings, translated) = match i18n_lookup(id, locale) {
        Some(s) => (s, true),
        None => (i18n_fallback(id), false),
    };
    let label = if translated {
        strings.label.to_string()
    } else {
        id.to_string()
    };
    SettingItem {
        id: id.to_string(),
        group: strings.group.to_string(),
        label,
        help: strings.help.map(str::to_string),
        value,
        surfaces,
    }
}

/// Builds the full list of toggleable settings, seeded with the
/// current values from `config`. Label/help strings come from the
/// effective locale (`config.effective_language()`), so toggling
/// `ui.language` and re-listing flips every row to the new language.
///
/// Group order matches the TUI screen's mental hierarchy: autonomy
/// first, defaults next, then committee / models / security / git /
/// surfaces / updates / locale.
#[allow(clippy::vec_init_then_push)] // 20+ items — sequential pushes are easier to scan than one giant vec![]
pub(crate) fn settings_registry(config: &PeridotConfig) -> Vec<SettingItem> {
    let mut items = Vec::new();
    let locale = config.effective_language();

    // === Autonomy loops ===
    items.push(make_item(
        "defaults.auto_verify_after_mutation",
        surfaces_all(),
        SettingValue::Bool(config.defaults.auto_verify_after_mutation),
        locale,
    ));
    items.push(make_item(
        "defaults.auto_grade_on_done",
        surfaces_all(),
        SettingValue::Bool(config.defaults.auto_grade_on_done),
        locale,
    ));

    // === Defaults ===
    items.push(make_item(
        "defaults.mode",
        surfaces_all(),
        SettingValue::Choice {
            options: vec!["plan".into(), "execute".into(), "goal".into()],
            selected: match config.defaults.mode {
                ExecutionMode::Plan => 0,
                ExecutionMode::Execute => 1,
                ExecutionMode::Goal => 2,
            },
        },
        locale,
    ));
    items.push(make_item(
        "defaults.permission",
        surfaces_all(),
        SettingValue::Choice {
            options: vec!["safe".into(), "auto".into(), "yolo".into()],
            selected: match config.defaults.permission {
                PermissionMode::Safe => 0,
                PermissionMode::Auto => 1,
                PermissionMode::Yolo => 2,
            },
        },
        locale,
    ));
    items.push(make_item(
        "defaults.max_turns",
        surfaces_all(),
        SettingValue::U32 {
            value: config.defaults.max_turns,
            min: 1,
            max: 1000,
            step: 10,
        },
        locale,
    ));
    items.push(make_item(
        "defaults.budget_usd",
        surfaces_all(),
        SettingValue::F64 {
            value: config.defaults.budget_usd,
            min: 0.0,
            max: 100.0,
            step: 0.5,
        },
        locale,
    ));
    items.push(make_item(
        "defaults.budget_warning_pct",
        surfaces_all(),
        SettingValue::U32 {
            value: config.defaults.budget_warning_pct as u32,
            min: 0,
            max: 100,
            step: 5,
        },
        locale,
    ));

    // === Committee (multi-agent) ===
    items.push(make_item(
        "committee.mode",
        surfaces_all(),
        SettingValue::Choice {
            options: vec!["off".into(), "planner".into(), "full".into()],
            selected: match config.committee.mode {
                CommitteeMode::Off => 0,
                CommitteeMode::Planner => 1,
                CommitteeMode::Full => 2,
            },
        },
        locale,
    ));
    items.push(make_item(
        "committee.min_task_chars",
        surfaces_all(),
        SettingValue::Usize {
            value: config.committee.min_task_chars,
            min: 0,
            max: 2000,
            step: 25,
        },
        locale,
    ));
    items.push(make_item(
        "committee.max_review_passes",
        surfaces_all(),
        SettingValue::U32 {
            value: config.committee.max_review_passes,
            min: 1,
            max: 10,
            step: 1,
        },
        locale,
    ));
    items.push(make_item(
        "committee.use_llm_complexity_gate",
        surfaces_all(),
        SettingValue::Bool(config.committee.use_llm_complexity_gate),
        locale,
    ));

    // === Models ===
    items.push(make_item(
        "models.reasoning_effort",
        surfaces_all(),
        SettingValue::Choice {
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
        locale,
    ));

    // === Security ===
    items.push(make_item(
        "security.sandbox",
        surfaces_all(),
        SettingValue::Choice {
            options: vec!["none".into(), "docker".into(), "firejail".into()],
            selected: match config.security.sandbox {
                SandboxMode::None => 0,
                SandboxMode::Docker => 1,
                SandboxMode::Firejail => 2,
            },
        },
        locale,
    ));
    items.push(make_item(
        "security.ask_before_install",
        surfaces_all(),
        SettingValue::Bool(config.security.ask_before_install),
        locale,
    ));
    items.push(make_item(
        "security.ask_before_delete",
        surfaces_all(),
        SettingValue::Bool(config.security.ask_before_delete),
        locale,
    ));

    // === Git ===
    items.push(make_item(
        "git.auto_commit",
        surfaces_all(),
        SettingValue::Bool(config.git.auto_commit),
        locale,
    ));
    items.push(make_item(
        "git.auto_branch",
        surfaces_all(),
        SettingValue::Bool(config.git.auto_branch),
        locale,
    ));

    // === TUI (terminal-only — VS Code webview filters by surfaces) ===
    items.push(make_item(
        "tui.show_thinking",
        surfaces_tui_only(),
        SettingValue::Bool(config.tui.show_thinking),
        locale,
    ));
    items.push(make_item(
        "tui.show_token_count",
        surfaces_tui_only(),
        SettingValue::Bool(config.tui.show_token_count),
        locale,
    ));
    items.push(make_item(
        "tui.show_cost",
        surfaces_tui_only(),
        SettingValue::Bool(config.tui.show_cost),
        locale,
    ));
    items.push(make_item(
        "tui.show_mascot",
        surfaces_tui_only(),
        SettingValue::Bool(config.tui.show_mascot),
        locale,
    ));
    items.push(make_item(
        "tui.mouse_capture",
        surfaces_tui_only(),
        SettingValue::Bool(config.tui.mouse_capture),
        locale,
    ));

    // === UI (cross-surface locale — replaces the legacy tui.language row) ===
    items.push(make_item(
        "ui.language",
        surfaces_all(),
        SettingValue::Choice {
            options: vec!["en".into(), "ko".into()],
            selected: match locale {
                Locale::En => 0,
                Locale::Ko => 1,
            },
        },
        locale,
    ));

    // === Updates ===
    items.push(make_item(
        "updates.auto_check",
        surfaces_all(),
        SettingValue::Bool(config.updates.auto_check),
        locale,
    ));

    items
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
        ("tui.mouse_capture", SettingValue::Bool(v)) => {
            config.tui.mouse_capture = *v;
        }
        ("ui.language", SettingValue::Choice { options, selected }) => {
            if let Some(label) = options.get(*selected) {
                let locale = match label.as_str() {
                    "ko" => Locale::Ko,
                    _ => Locale::En,
                };
                config.ui.language = Some(locale);
                // Mirror to the legacy `tui.language` knob so old
                // readers (anything still consulting `config.tui.language`
                // directly instead of `effective_language()`) see the
                // new value too. New code should read
                // `effective_language()`.
                config.tui.language = locale;
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
            surfaces: SettingItem::default_surfaces(),
        }];
        // Should not panic and should not touch defaults.
        let snapshot = config.clone();
        apply_settings_to_config(&items, &mut config);
        assert_eq!(
            config.defaults.auto_verify_after_mutation,
            snapshot.defaults.auto_verify_after_mutation
        );
    }

    #[test]
    fn registry_switches_language_via_ui_language() {
        let mut config = PeridotConfig::default();
        config.ui.language = Some(Locale::Ko);
        let items = settings_registry(&config);
        let verify = items
            .iter()
            .find(|i| i.id == "defaults.auto_verify_after_mutation")
            .expect("auto-verify item present");
        assert!(
            verify.label.contains("자동 검증"),
            "expected Korean label, got {:?}",
            verify.label
        );
    }

    #[test]
    fn registry_falls_back_to_tui_language_when_ui_unset() {
        // Mirror an older config file that pre-dates the [ui] section:
        // `ui.language` is None, `tui.language` is the only knob set.
        let mut config = PeridotConfig::default();
        config.ui.language = None;
        config.tui.language = Locale::Ko;
        let items = settings_registry(&config);
        let verify = items
            .iter()
            .find(|i| i.id == "defaults.auto_verify_after_mutation")
            .unwrap();
        assert!(
            verify.label.contains("자동 검증"),
            "expected Korean label via tui.language fallback, got {:?}",
            verify.label
        );
    }

    #[test]
    fn tui_only_items_carry_tui_surface() {
        let config = PeridotConfig::default();
        let items = settings_registry(&config);
        let mascot = items
            .iter()
            .find(|i| i.id == "tui.show_mascot")
            .expect("mascot item present");
        assert_eq!(mascot.surfaces, vec!["tui".to_string()]);
        // ui.language is cross-surface — sanity check the contrast.
        let language = items
            .iter()
            .find(|i| i.id == "ui.language")
            .expect("ui.language item present");
        assert!(language.surfaces.contains(&"vscode".to_string()));
    }

    #[test]
    fn applying_ui_language_mirrors_to_tui_language() {
        // Old code paths still read `tui.language` directly — make
        // sure `apply_settings_to_config` keeps both fields in sync so
        // nothing sees a stale value when the user flips locales.
        let mut config = PeridotConfig::default();
        let mut items = settings_registry(&config);
        for item in items.iter_mut() {
            if item.id == "ui.language"
                && let SettingValue::Choice {
                    options, selected, ..
                } = &mut item.value
            {
                *selected = options.iter().position(|o| o == "ko").unwrap();
            }
        }
        apply_settings_to_config(&items, &mut config);
        assert_eq!(config.ui.language, Some(Locale::Ko));
        assert_eq!(config.tui.language, Locale::Ko);
    }

    #[test]
    fn settings_i18n_covers_registry() {
        // Every id surfaced by the registry must have a translation
        // entry, otherwise the page silently falls back to the id
        // string which looks broken in production. Catching this at
        // test time forces translators to update the table when a new
        // field lands.
        let config = PeridotConfig::default();
        let items = settings_registry(&config);
        for item in &items {
            let en = i18n_lookup(&item.id, Locale::En)
                .unwrap_or_else(|| panic!("missing en for {}", item.id));
            let ko = i18n_lookup(&item.id, Locale::Ko)
                .unwrap_or_else(|| panic!("missing ko for {}", item.id));
            assert!(!en.label.is_empty());
            assert!(!ko.label.is_empty());
        }
    }
}
