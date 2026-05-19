//! Progressive-disclosure skill tools.
//!
//! Skill bodies are no longer pushed into the agent's context by default.
//! Two new tools let the model pull just the metadata first, then load
//! the full body on demand — matching Hermes Agent's L0/L1 disclosure
//! pattern. Loading a body via `skill_view` also stamps the row's
//! `last_used_at_unix`, which is what feeds the Curator's stale/archive
//! decisions.
//!
//! - `skill_list` (L0): name + scope + first-line description + idle days
//!   for every active skill. No body bytes, low token cost.
//! - `skill_view` (L1): full body for one skill name. Also marks the
//!   skill as recently viewed so the Curator's 7-day idle pass keeps
//!   it active.

use std::time::{SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use peridot_common::{PeriError, PeriResult, PermissionLevel, ToolGroup, ToolResult};
use peridot_memory::MemoryStore;
use serde::Serialize;
use serde_json::Value;

use crate::path::required_str;
use crate::{Tool, ToolContext};

/// L0 — metadata-only listing of active skills.
#[derive(Clone, Debug)]
pub struct SkillListTool;

/// L1 — load a specific skill body on demand.
#[derive(Clone, Debug)]
pub struct SkillViewTool;

#[derive(Serialize)]
struct SkillMeta {
    name: String,
    scope: String,
    description: String,
    idle_days: u64,
}

#[async_trait]
impl Tool for SkillListTool {
    fn name(&self) -> &str {
        "skill_list"
    }

    fn group(&self) -> ToolGroup {
        ToolGroup::Agent
    }

    fn description(&self) -> &str {
        "List learned skills as name + one-line description + idle_days. \
         Bodies are NOT loaded; call skill_view to read one."
    }

    fn parameters_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {},
            "additionalProperties": false,
        })
    }

    async fn execute(&self, _params: Value, ctx: &ToolContext) -> PeriResult<ToolResult> {
        let store = MemoryStore::new(ctx.project_root.join(".peridot/memory.db"));
        let records = store.list_skill_records()?;
        let now = unix_now();
        let metas: Vec<SkillMeta> = records
            .iter()
            .filter(|r| r.skill.archived_at_unix == 0)
            .map(|r| {
                let reference = r.skill.last_used_at_unix.max(r.updated_at_unix);
                let idle_days = now.saturating_sub(reference) / (24 * 3600);
                SkillMeta {
                    name: r.skill.name.clone(),
                    scope: r.skill.scope.clone(),
                    description: description_of(&r.skill.body),
                    idle_days,
                }
            })
            .collect();
        Ok(ToolResult::success(
            format!("{} skills available (metadata only)", metas.len()),
            serde_json::json!({ "skills": metas }),
        ))
    }

    fn permission_level(&self) -> PermissionLevel {
        PermissionLevel::Read
    }
}

#[async_trait]
impl Tool for SkillViewTool {
    fn name(&self) -> &str {
        "skill_view"
    }

    fn group(&self) -> ToolGroup {
        ToolGroup::Agent
    }

    fn description(&self) -> &str {
        "Load the full body of one learned skill by name. \
         Marks the skill as recently used so the Curator keeps it active."
    }

    fn parameters_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "name": { "type": "string", "description": "Skill name as returned by skill_list" }
            },
            "required": ["name"],
            "additionalProperties": false,
        })
    }

    async fn execute(&self, params: Value, ctx: &ToolContext) -> PeriResult<ToolResult> {
        let name = required_str(&params, "name")?;
        let store = MemoryStore::new(ctx.project_root.join(".peridot/memory.db"));
        let records = store.list_skill_records()?;
        let record = records
            .into_iter()
            .find(|r| r.skill.name == name && r.skill.archived_at_unix == 0)
            .ok_or_else(|| PeriError::Tool(format!("skill not found or archived: {name}")))?;
        let _ = store.mark_skill_viewed(name, unix_now());
        Ok(ToolResult::success(
            format!(
                "loaded skill `{}` ({} bytes)",
                name,
                record.skill.body.len()
            ),
            serde_json::json!({
                "name": name,
                "body": record.skill.body,
                "scope": record.skill.scope,
            }),
        ))
    }

    fn validate_params(&self, params: &Value) -> PeriResult<()> {
        let _ = required_str(params, "name")?;
        Ok(())
    }

    fn permission_level(&self) -> PermissionLevel {
        PermissionLevel::Read
    }
}

/// Returns a short single-line summary derived from the body. The auto
/// Curator currently writes `# Auto Skill: <task>` as the first line, so
/// stripping the leading `#`s yields the task name.
fn description_of(body: &str) -> String {
    body.lines()
        .next()
        .unwrap_or("")
        .trim_start_matches('#')
        .trim()
        .to_string()
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use peridot_common::PermissionMode;
    use peridot_memory::StoredSkill;

    fn unique_root(label: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!(
            "peridot-tools-skill-{label}-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ))
    }

    #[tokio::test]
    async fn skill_list_returns_metadata_only() {
        let root = unique_root("list");
        let store = MemoryStore::new(root.join(".peridot/memory.db"));
        store
            .save_skill(&StoredSkill {
                name: "auto-fix-parser".into(),
                body: "# Auto Skill: fix parser\n\nstep one\nstep two".into(),
                scope: "auto".into(),
                ..Default::default()
            })
            .unwrap();
        store
            .save_skill(&StoredSkill {
                name: "archived-one".into(),
                body: "old".into(),
                scope: "auto".into(),
                archived_at_unix: 1,
                ..Default::default()
            })
            .unwrap();

        let ctx = ToolContext::new(&root, PermissionMode::Auto);
        let result = SkillListTool
            .execute(serde_json::json!({}), &ctx)
            .await
            .unwrap();

        let skills = result.output["skills"].as_array().unwrap();
        assert_eq!(skills.len(), 1, "archived rows are excluded");
        assert_eq!(skills[0]["name"], "auto-fix-parser");
        assert_eq!(skills[0]["description"], "Auto Skill: fix parser");
        assert!(
            result.output["skills"][0].get("body").is_none(),
            "L0 listing must not carry body bytes"
        );
        std::fs::remove_dir_all(root).ok();
    }

    #[tokio::test]
    async fn skill_view_returns_body_and_marks_viewed() {
        let root = unique_root("view");
        let store = MemoryStore::new(root.join(".peridot/memory.db"));
        store
            .save_skill(&StoredSkill {
                name: "auto-do-thing".into(),
                body: "the full body".into(),
                scope: "auto".into(),
                ..Default::default()
            })
            .unwrap();

        let ctx = ToolContext::new(&root, PermissionMode::Auto);
        let before = unix_now();
        let result = SkillViewTool
            .execute(serde_json::json!({"name": "auto-do-thing"}), &ctx)
            .await
            .unwrap();
        let after = unix_now();

        assert_eq!(result.output["body"], "the full body");
        let record = store
            .list_skill_records()
            .unwrap()
            .into_iter()
            .find(|r| r.skill.name == "auto-do-thing")
            .unwrap();
        assert!(record.skill.last_used_at_unix >= before);
        assert!(record.skill.last_used_at_unix <= after + 1);
        std::fs::remove_dir_all(root).ok();
    }

    #[tokio::test]
    async fn skill_view_errors_for_missing_or_archived_skill() {
        let root = unique_root("missing");
        MemoryStore::new(root.join(".peridot/memory.db"))
            .save_skill(&StoredSkill {
                name: "shelved".into(),
                body: "x".into(),
                scope: "auto".into(),
                archived_at_unix: 1,
                ..Default::default()
            })
            .unwrap();
        let ctx = ToolContext::new(&root, PermissionMode::Auto);
        assert!(
            SkillViewTool
                .execute(serde_json::json!({"name": "shelved"}), &ctx)
                .await
                .is_err()
        );
        assert!(
            SkillViewTool
                .execute(serde_json::json!({"name": "never-existed"}), &ctx)
                .await
                .is_err()
        );
        std::fs::remove_dir_all(root).ok();
    }
}
