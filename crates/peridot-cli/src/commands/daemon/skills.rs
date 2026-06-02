//! Skill inventory slash command handlers (list/show/search/archived/
//! pin/archive/restore) and their result-rendering helpers, split out of
//! the daemon module. Shared helpers such as `skill_description` and
//! `append_plan_reminder_to_context` stay in the parent and are reached
//! via `use super::*`.

use peridot_memory::MemoryStore;
use serde_json::Value;

use super::*;

pub(super) fn handle_command_skill_list(
    state: &DaemonState,
    raw_command: &str,
) -> Result<Value, String> {
    command_skill_list_result(state, raw_command, None)
}

pub(super) fn handle_command_skill_show(
    state: &DaemonState,
    raw_command: &str,
    name: &str,
) -> Result<Value, String> {
    let store = peridot_memory::MemoryStore::new(state.project_root.join(".peridot/memory.db"));
    let records = store
        .list_skill_records()
        .map_err(|err| format!("skills: failed to read skill store: {err}"))?;
    let skill = records
        .into_iter()
        .map(|record| record.skill)
        .find(|skill| skill.name == name)
        .ok_or_else(|| format!("skill not found: {name}"))?;
    let description = skill_description(&skill);
    let label = format!("/{}", skill.name);
    let archived = skill.archived_at_unix > 0;
    Ok(serde_json::json!({
        "kind": "skill_detail",
        "title": format!("Skill: {}", skill.name),
        "message": description.clone(),
        "severity": "info",
        "command": raw_command,
        "name": skill.name,
        "label": label,
        "detail": description,
        "scope": skill.scope,
        "pinned": skill.pinned_at_unix > 0,
        "archived": archived,
        "archived_at_unix": skill.archived_at_unix,
        "last_used_at_unix": skill.last_used_at_unix,
        "body": skill.body,
    }))
}

pub(super) fn handle_command_skill_search(
    state: &DaemonState,
    raw_command: &str,
    query: &str,
) -> Result<Value, String> {
    let store = peridot_memory::MemoryStore::new(state.project_root.join(".peridot/memory.db"));
    let mut skills = store
        .search_skills(query)
        .map_err(|err| format!("skills: failed to search skill store: {err}"))?;
    skills.sort_by(|a, b| a.scope.cmp(&b.scope).then_with(|| a.name.cmp(&b.name)));
    let rows = skill_inventory_rows(&skills);
    Ok(serde_json::json!({
        "kind": "skills",
        "title": "Skills",
        "message": if rows.is_empty() {
            format!("skills: no matches for `{}`", query.trim())
        } else {
            format!("skills: {} match(es) for `{}`", rows.len(), query.trim())
        },
        "severity": "info",
        "command": raw_command,
        "query": query.trim(),
        "total": rows.len(),
        "items": rows,
    }))
}

pub(super) fn handle_command_skill_archived(
    state: &DaemonState,
    raw_command: &str,
    query: &str,
) -> Result<Value, String> {
    let query = query.trim();
    let store = peridot_memory::MemoryStore::new(state.project_root.join(".peridot/memory.db"));
    let mut archived: Vec<_> = store
        .list_skill_records()
        .map_err(|err| format!("skills: failed to read skill store: {err}"))?
        .into_iter()
        .filter(|record| record.skill.archived_at_unix > 0)
        .filter(|record| {
            query.is_empty()
                || record.skill.name.contains(query)
                || record.skill.body.contains(query)
                || record.skill.description.contains(query)
        })
        .collect();
    archived.sort_by(|a, b| {
        a.skill
            .scope
            .cmp(&b.skill.scope)
            .then_with(|| a.skill.name.cmp(&b.skill.name))
    });
    let rows = archived_skill_inventory_rows(&archived);
    let message = if rows.is_empty() {
        if query.is_empty() {
            "skills: no archived skills".to_string()
        } else {
            format!("skills: no archived matches for `{query}`")
        }
    } else if query.is_empty() {
        format!("skills: {} archived", rows.len())
    } else {
        format!("skills: {} archived match(es) for `{query}`", rows.len())
    };
    Ok(serde_json::json!({
        "kind": "skills",
        "title": "Archived Skills",
        "message": message,
        "severity": "info",
        "command": raw_command,
        "query": query,
        "archived": true,
        "total": rows.len(),
        "items": rows,
    }))
}

pub(super) fn handle_command_skill_pin(
    state: &DaemonState,
    raw_command: &str,
    name: &str,
    pinned: bool,
) -> Result<Value, String> {
    let store = peridot_memory::MemoryStore::new(state.project_root.join(".peridot/memory.db"));
    let ts = if pinned {
        crate::run_state::unix_timestamp()
    } else {
        0
    };
    let updated = store.set_skill_pinned(name, ts).map_err(|err| {
        let verb = if pinned { "pin" } else { "unpin" };
        format!("skills: failed to {verb} `{name}`: {err}")
    })?;
    if !updated {
        return Err(format!("skill not found: {name}"));
    }
    let verb = if pinned { "pinned" } else { "unpinned" };
    command_skill_list_result(state, raw_command, Some(format!("{verb} skill `{name}`")))
}

pub(super) fn handle_command_skill_archive(
    state: &DaemonState,
    raw_command: &str,
    name: &str,
) -> Result<Value, String> {
    let store = peridot_memory::MemoryStore::new(state.project_root.join(".peridot/memory.db"));
    let updated = store
        .set_skill_archived(name, crate::run_state::unix_timestamp())
        .map_err(|err| format!("skills: failed to archive `{name}`: {err}"))?;
    if !updated {
        return Err(format!("skill not found: {name}"));
    }
    move_auto_skill_to_archive(&state.project_root, name)
        .map_err(|err| format!("skills: archived `{name}` but failed to move file: {err}"))?;
    command_skill_list_result(state, raw_command, Some(format!("archived skill `{name}`")))
}

pub(super) fn handle_command_skill_restore(
    state: &DaemonState,
    raw_command: &str,
    name: &str,
) -> Result<Value, String> {
    let store = peridot_memory::MemoryStore::new(state.project_root.join(".peridot/memory.db"));
    restore_archived_skill(&store, &state.project_root, name)
        .map_err(|err| format!("skills: failed to restore `{name}`: {err}"))?;
    command_skill_list_result(state, raw_command, Some(format!("restored skill `{name}`")))
}

fn command_skill_list_result(
    state: &DaemonState,
    raw_command: &str,
    message: Option<String>,
) -> Result<Value, String> {
    let store = peridot_memory::MemoryStore::new(state.project_root.join(".peridot/memory.db"));
    let mut skills = store
        .list_skills()
        .map_err(|err| format!("skills: failed to read skill store: {err}"))?;
    skills.sort_by(|a, b| a.scope.cmp(&b.scope).then_with(|| a.name.cmp(&b.name)));
    let rows = skill_inventory_rows(&skills);
    let default_message = if rows.is_empty() {
        "skills: <none>".to_string()
    } else {
        format!("skills: {} active", rows.len())
    };
    Ok(serde_json::json!({
        "kind": "skills",
        "title": "Skills",
        "message": message.unwrap_or(default_message),
        "severity": "info",
        "command": raw_command,
        "total": rows.len(),
        "items": rows,
    }))
}

fn skill_inventory_rows(skills: &[peridot_memory::StoredSkill]) -> Vec<Value> {
    skills
        .iter()
        .map(|skill| {
            serde_json::json!({
                "label": format!("/{}", skill.name),
                "detail": skill_description(skill),
                "source": "skill",
                "scope": skill.scope,
                "last_used_at_unix": skill.last_used_at_unix,
                "pinned": skill.pinned_at_unix > 0,
            })
        })
        .collect()
}

fn archived_skill_inventory_rows(records: &[peridot_memory::SkillRecord]) -> Vec<Value> {
    records
        .iter()
        .map(|record| {
            let skill = &record.skill;
            serde_json::json!({
                "label": format!("/{}", skill.name),
                "detail": skill_description(skill),
                "source": "skill",
                "scope": skill.scope,
                "last_used_at_unix": skill.last_used_at_unix,
                "archived_at_unix": skill.archived_at_unix,
                "archived": true,
                "pinned": skill.pinned_at_unix > 0,
            })
        })
        .collect()
}
