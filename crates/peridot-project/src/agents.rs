use std::fs;
use std::path::{Path, PathBuf};

use peridot_common::{ExecutionMode, PeriError, PeriResult, PermissionMode};

use crate::types::ProjectPreferences;

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct ParsedAgents {
    pub(crate) overrides: Vec<String>,
    pub(crate) preferences: ProjectPreferences,
    pub(crate) boundaries: Vec<String>,
}

pub(crate) fn find_agents_file(root: &Path) -> Option<PathBuf> {
    [
        ".peridot/AGENTS.md",
        "AGENTS.md",
        "CLAUDE.md",
        ".github/copilot-instructions.md",
    ]
    .iter()
    .map(|path| root.join(path))
    .find(|path| path.exists())
}

pub(crate) fn parse_agents_file(root: &Path) -> PeriResult<ParsedAgents> {
    let Some(path) = find_agents_file(root) else {
        return Ok(ParsedAgents::default());
    };
    let content = fs::read_to_string(&path)
        .map_err(|err| peridot_common::PeriError::Parse(format!("{}: {err}", path.display())))?;
    let mut parsed = ParsedAgents::default();
    let mut section = String::new();
    for line in content.lines() {
        if let Some(stripped) = line.strip_prefix("## ") {
            section = stripped.trim().to_ascii_lowercase();
            continue;
        }
        if matches!(section.as_str(), "commands" | "boundaries" | "preferences") {
            let trimmed = line.trim();
            if !trimmed.is_empty() {
                parsed.overrides.push(format!("{section}: {trimmed}"));
                if section == "boundaries"
                    && let Some(boundary) = parse_do_not_modify_boundary(trimmed)
                {
                    parsed.boundaries.push(boundary);
                }
                if section == "preferences" {
                    parse_preference_line(trimmed, &mut parsed.preferences)?;
                }
            }
        }
    }
    Ok(parsed)
}

fn parse_preference_line(line: &str, preferences: &mut ProjectPreferences) -> PeriResult<()> {
    let line = strip_list_marker(line);
    let Some((key, value)) = line.split_once(':') else {
        return Ok(());
    };
    let key = key.trim().to_ascii_lowercase();
    let value = value.trim().trim_matches(['"', '\'']);
    if value.is_empty() {
        return Ok(());
    }
    match key.as_str() {
        "default_mode" => {
            preferences.default_mode = Some(parse_execution_mode(value)?);
        }
        "default_permission" => {
            preferences.default_permission = Some(parse_permission_mode(value)?);
        }
        "ask_before_install" => {
            preferences.ask_before_install = Some(parse_bool_preference(key.as_str(), value)?);
        }
        "ask_before_delete" => {
            preferences.ask_before_delete = Some(parse_bool_preference(key.as_str(), value)?);
        }
        "auto_commit" => {
            preferences.auto_commit = Some(parse_bool_preference(key.as_str(), value)?);
        }
        "commit_frequency" => {
            preferences.commit_frequency = Some(value.to_string());
        }
        "branch_prefix" => {
            preferences.branch_prefix = Some(value.to_string());
        }
        _ => {}
    }
    Ok(())
}

fn strip_list_marker(line: &str) -> &str {
    line.trim_start_matches(['-', '*', ' ']).trim()
}

fn parse_execution_mode(value: &str) -> PeriResult<ExecutionMode> {
    match value.to_ascii_lowercase().as_str() {
        "plan" => Ok(ExecutionMode::Plan),
        "execute" => Ok(ExecutionMode::Execute),
        "goal" => Ok(ExecutionMode::Goal),
        _ => Err(PeriError::Parse(format!(
            "unsupported AGENTS default_mode: {value}"
        ))),
    }
}

fn parse_permission_mode(value: &str) -> PeriResult<PermissionMode> {
    match value.to_ascii_lowercase().as_str() {
        "safe" => Ok(PermissionMode::Safe),
        "auto" => Ok(PermissionMode::Auto),
        "yolo" => Ok(PermissionMode::Yolo),
        _ => Err(PeriError::Parse(format!(
            "unsupported AGENTS default_permission: {value}"
        ))),
    }
}

fn parse_bool_preference(key: &str, value: &str) -> PeriResult<bool> {
    match value.to_ascii_lowercase().as_str() {
        "true" | "yes" | "y" | "1" | "on" => Ok(true),
        "false" | "no" | "n" | "0" | "off" => Ok(false),
        _ => Err(PeriError::Parse(format!(
            "unsupported AGENTS boolean value for {key}: {value}"
        ))),
    }
}

fn parse_do_not_modify_boundary(line: &str) -> Option<String> {
    let line = strip_list_marker(line);
    let prefix = "DO NOT modify ";
    line.strip_prefix(prefix)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| value.trim_end_matches('.').to_string())
}
