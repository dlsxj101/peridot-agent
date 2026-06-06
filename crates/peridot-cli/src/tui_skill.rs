//! Interactive `/skill` slash-command handlers.
//!
//! Load / list / show / search / pin / archive / restore stored skills from the
//! per-project skill store (`MemoryStore`). Split out of `main.rs`; the
//! `apply_session_command` dispatcher calls these. `skill_description` is also
//! used by `main`'s auto-skill suggestion loader, so it is `pub(crate)`; the
//! shared `append_plan_reminder_to_context` / `load_auto_skill_suggestions`
//! helpers stay in `main.rs`.

use std::path::Path;

use peridot_memory::{MemoryStore, StoredSkill};
use peridot_tui::TuiState;

use crate::commands::{move_auto_skill_to_archive, restore_archived_skill};
use crate::run_state::unix_timestamp;
use crate::{append_plan_reminder_to_context, load_auto_skill_suggestions};

/// A short one-line description of a stored skill: its explicit description if
/// set, else the first non-heading body line (capped), else a placeholder.
pub(crate) fn skill_description(skill: &StoredSkill) -> String {
    if !skill.description.trim().is_empty() {
        return skill.description.trim().to_string();
    }
    skill
        .body
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty() && !line.starts_with('#'))
        .unwrap_or("stored auto-skill")
        .chars()
        .take(120)
        .collect()
}

fn skill_plan_reminder(skill: &StoredSkill, args: &str) -> String {
    let trimmed_args = args.trim();
    if trimmed_args.is_empty() {
        format!("[skill:{}]\n{}", skill.name, skill.body)
    } else {
        format!(
            "[skill:{}]\nOperator passed args: {}\n\n{}",
            skill.name, trimmed_args, skill.body
        )
    }
}

pub(crate) fn handle_skill_load(state: &mut TuiState, project_root: &Path, name: &str, args: &str) {
    let session_id = state.current_session_id.clone();
    if session_id.is_empty() {
        state.push_error("skill: no active session".to_string());
        return;
    }
    let store = MemoryStore::new(project_root.join(".peridot/memory.db"));
    let active = match store.list_skills() {
        Ok(skills) => skills,
        Err(err) => {
            state.push_error(format!("skill `{name}`: failed to read skill store: {err}"));
            return;
        }
    };
    let Some(skill) = active.into_iter().find(|skill| skill.name == name) else {
        state.push_error(format!(
            "skill not found: {name}. Run `peridot run \"...\"` once to build relevant auto-skills, or type `/help`."
        ));
        return;
    };
    if let Err(err) = append_plan_reminder_to_context(
        project_root,
        &session_id,
        skill_plan_reminder(&skill, args),
        Vec::new(),
    ) {
        state.push_error(format!("skill `{name}`: failed to update context: {err}"));
        return;
    }
    let _ = store.mark_skill_viewed(&skill.name, unix_timestamp());
    state.set_skill_suggestions(load_auto_skill_suggestions(project_root));
    let args_note = if args.trim().is_empty() {
        String::new()
    } else {
        format!(" with args `{}`", args.trim())
    };
    state.push_transcript(format!("Loaded skill `{}`{args_note}", skill.name));
}

pub(crate) fn handle_skill_list(state: &mut TuiState, project_root: &Path) {
    let store = MemoryStore::new(project_root.join(".peridot/memory.db"));
    let mut active = match store.list_skills() {
        Ok(skills) => skills,
        Err(err) => {
            state.push_error(format!("skills: failed to read skill store: {err}"));
            return;
        }
    };
    active.sort_by(|a, b| a.scope.cmp(&b.scope).then_with(|| a.name.cmp(&b.name)));
    if active.is_empty() {
        state.push_transcript("skills: <none>");
        return;
    }
    let mut lines = vec![format!("skills: {} active", active.len())];
    for skill in active {
        let pinned = if skill.pinned_at_unix > 0 {
            " · pinned"
        } else {
            ""
        };
        lines.push(format!(
            "  /{}  ·  {} [{}{}]",
            skill.name,
            skill_description(&skill),
            skill.scope,
            pinned,
        ));
    }
    state.push_transcript(lines.join("\n"));
}

pub(crate) fn handle_skill_show(state: &mut TuiState, project_root: &Path, name: &str) {
    let store = MemoryStore::new(project_root.join(".peridot/memory.db"));
    let records = match store.list_skill_records() {
        Ok(records) => records,
        Err(err) => {
            state.push_error(format!("skills: failed to read skill store: {err}"));
            return;
        }
    };
    let Some(skill) = records
        .into_iter()
        .map(|record| record.skill)
        .find(|skill| skill.name == name)
    else {
        state.push_error(format!("skill not found: {name}"));
        return;
    };
    let pinned = if skill.pinned_at_unix > 0 {
        " · pinned"
    } else {
        ""
    };
    let archived = if skill.archived_at_unix > 0 {
        format!(" · archived {}", skill.archived_at_unix)
    } else {
        String::new()
    };
    state.push_transcript(format!(
        "skill `{}`\nscope: {}{}{}\ndescription: {}\n\n{}",
        skill.name,
        skill.scope,
        pinned,
        archived,
        skill_description(&skill),
        skill.body.trim()
    ));
}

pub(crate) fn handle_skill_search(state: &mut TuiState, project_root: &Path, query: &str) {
    let store = MemoryStore::new(project_root.join(".peridot/memory.db"));
    let mut matches = match store.search_skills(query) {
        Ok(skills) => skills,
        Err(err) => {
            state.push_error(format!("skills: failed to search skill store: {err}"));
            return;
        }
    };
    matches.sort_by(|a, b| a.scope.cmp(&b.scope).then_with(|| a.name.cmp(&b.name)));
    if matches.is_empty() {
        state.push_transcript(format!("skills: no matches for `{}`", query.trim()));
        return;
    }
    let mut lines = vec![format!(
        "skills: {} match(es) for `{}`",
        matches.len(),
        query.trim()
    )];
    for skill in matches {
        let pinned = if skill.pinned_at_unix > 0 {
            " · pinned"
        } else {
            ""
        };
        lines.push(format!(
            "  /{}  ·  {} [{}{}]",
            skill.name,
            skill_description(&skill),
            skill.scope,
            pinned,
        ));
    }
    state.push_transcript(lines.join("\n"));
}

pub(crate) fn handle_skill_archived(state: &mut TuiState, project_root: &Path, query: &str) {
    let store = MemoryStore::new(project_root.join(".peridot/memory.db"));
    let mut archived: Vec<_> = match store.list_skill_records() {
        Ok(records) => records
            .into_iter()
            .filter(|record| record.skill.archived_at_unix > 0)
            .filter(|record| {
                let query = query.trim();
                query.is_empty()
                    || record.skill.name.contains(query)
                    || record.skill.body.contains(query)
                    || record.skill.description.contains(query)
            })
            .collect(),
        Err(err) => {
            state.push_error(format!("skills: failed to read skill store: {err}"));
            return;
        }
    };
    archived.sort_by(|a, b| {
        a.skill
            .scope
            .cmp(&b.skill.scope)
            .then_with(|| a.skill.name.cmp(&b.skill.name))
    });
    if archived.is_empty() {
        if query.trim().is_empty() {
            state.push_transcript("skills: no archived skills");
        } else {
            state.push_transcript(format!(
                "skills: no archived matches for `{}`",
                query.trim()
            ));
        }
        return;
    }
    let mut lines = vec![format!("skills: {} archived", archived.len())];
    for record in archived {
        lines.push(format!(
            "  /{}  ·  {} [{} · archived {}]",
            record.skill.name,
            skill_description(&record.skill),
            record.skill.scope,
            record.skill.archived_at_unix,
        ));
    }
    state.push_transcript(lines.join("\n"));
}

pub(crate) fn handle_skill_pin(
    state: &mut TuiState,
    project_root: &Path,
    name: &str,
    pinned: bool,
) {
    let store = MemoryStore::new(project_root.join(".peridot/memory.db"));
    let ts = if pinned { unix_timestamp() } else { 0 };
    match store.set_skill_pinned(name, ts) {
        Ok(true) => {
            state.set_skill_suggestions(load_auto_skill_suggestions(project_root));
            let verb = if pinned { "pinned" } else { "unpinned" };
            state.push_transcript(format!("{verb} skill `{name}`"));
        }
        Ok(false) => state.push_error(format!("skill not found: {name}")),
        Err(err) => {
            let verb = if pinned { "pin" } else { "unpin" };
            state.push_error(format!("skills: failed to {verb} `{name}`: {err}"));
        }
    }
}

pub(crate) fn handle_skill_archive(state: &mut TuiState, project_root: &Path, name: &str) {
    let store = MemoryStore::new(project_root.join(".peridot/memory.db"));
    match store.set_skill_archived(name, unix_timestamp()) {
        Ok(true) => {
            if let Err(err) = move_auto_skill_to_archive(project_root, name) {
                state.push_error(format!(
                    "skills: archived `{name}` but failed to move file: {err}"
                ));
                return;
            }
            state.set_skill_suggestions(load_auto_skill_suggestions(project_root));
            state.push_transcript(format!("archived skill `{name}`"));
        }
        Ok(false) => state.push_error(format!("skill not found: {name}")),
        Err(err) => state.push_error(format!("skills: failed to archive `{name}`: {err}")),
    }
}

pub(crate) fn handle_skill_restore(state: &mut TuiState, project_root: &Path, name: &str) {
    let store = MemoryStore::new(project_root.join(".peridot/memory.db"));
    match restore_archived_skill(&store, project_root, name) {
        Ok(_) => {
            state.set_skill_suggestions(load_auto_skill_suggestions(project_root));
            state.push_transcript(format!("restored skill `{name}`"));
        }
        Err(err) => state.push_error(format!("skills: failed to restore `{name}`: {err}")),
    }
}
