//! Hermes-style LLM Curator.
//!
//! Sub-agent that periodically reviews `scope='auto'` skills produced by
//! the harness. The CLI command (`peridot skill curate --llm`) and the
//! 7-day idle auto-trigger both call into `run_llm_curator` — the wiring
//! lands in the next commit, so dead-code warnings are suppressed here
//! for now rather than scattering `#[allow]`s on each helper.
//!
//! Each pass picks one of four actions per skill:
//!
//! - `keep` — skill is still useful, leave it alone.
//! - `patch` — rewrite the body for clarity/correctness; metadata stays.
//! - `consolidate` — merge this skill into another (target gets the
//!   combined body, source is archived).
//! - `archive` — skill is stale or redundant; hide it from the auto pool.
//!
//! The Curator never touches non-`auto` rows. A single run evaluates at
//! most [`MAX_SKILLS_PER_RUN`] skills so the LLM cost stays bounded —
//! this mirrors Hermes Agent's 8-iteration cap. Older entries come first
//! so the long tail eventually gets cleaned up across multiple runs.

#![allow(dead_code)]

use anyhow::{Context, Result, anyhow};

use peridot_common::ReasoningEffort;
use peridot_llm::{CompletionRequest, LlmMessage, LlmProvider, MessageRole, ToolChoice};
use peridot_memory::{MemoryStore, SkillRecord};
use serde::Deserialize;

/// Per-run cap on skills sent to the LLM. Hermes Agent's Curator caps
/// iterations at 8; we match it because the latency / cost of one LLM
/// call grows with batch size and stale skills can wait for the next
/// 7-day idle trigger.
pub const MAX_SKILLS_PER_RUN: usize = 8;

/// Result of one Curator pass — what was sent to the LLM and what was
/// applied. Caller renders this for the operator.
#[derive(Debug, Default)]
pub struct CuratorReport {
    /// Skill names actually sent to the model in this batch.
    pub evaluated: Vec<String>,
    /// (name, applied_action) for each successful action.
    pub applied: Vec<(String, String)>,
    /// Skill names the LLM mentioned that no longer match the batch.
    pub ignored: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct CuratorResponse {
    actions: Vec<CuratorAction>,
}

#[derive(Debug, Deserialize)]
struct CuratorAction {
    name: String,
    action: String,
    #[serde(default)]
    body: Option<String>,
    #[serde(default)]
    merge_into: Option<String>,
}

const SYSTEM_PROMPT: &str = "\
You are the Peridot Curator. You review agent-authored skill notes that\n\
the harness saved automatically. For each skill, decide one action:\n\
- keep: the skill is correct, useful, and distinct. No changes.\n\
- patch: the skill is mostly useful but the body needs a rewrite for\n\
  clarity, correctness, or to drop dead references. Return the full\n\
  rewritten body in `body`.\n\
- consolidate: the skill duplicates another in the batch. Pick a target\n\
  via `merge_into` (must be another `name` from the batch) and return\n\
  the merged body in `body`. The original is archived; the target\n\
  receives the merged body.\n\
- archive: the skill is stale, obsolete, or never produced value. Drop.\n\
\n\
Respond with strict JSON:\n\
{\"actions\":[{\"name\":\"<skill name>\",\"action\":\"keep|patch|consolidate|archive\",\
\"body\":\"<full new body, only for patch/consolidate>\",\
\"merge_into\":\"<target name, only for consolidate>\"}]}\n\
No prose outside the JSON object. No code fences.\n\
Be conservative: prefer `keep` when unsure. Prefer `archive` only when\n\
the skill is clearly broken or obsolete.";

/// Runs one Curator pass. Returns a report; never panics.
pub async fn run_llm_curator(
    provider: &dyn LlmProvider,
    model: &str,
    store: &MemoryStore,
    now_unix: u64,
) -> Result<CuratorReport> {
    let batch = select_batch(store)?;
    if batch.is_empty() {
        return Ok(CuratorReport::default());
    }
    let prompt = build_user_prompt(&batch, now_unix);
    let request = CompletionRequest {
        model: model.to_string(),
        system: Some(SYSTEM_PROMPT.to_string()),
        messages: vec![LlmMessage::new(MessageRole::User, prompt)],
        max_tokens: Some(4096),
        thinking: false,
        reasoning_effort: ReasoningEffort::Off,
        tools: Vec::new(),
        tool_choice: ToolChoice::None,
    };
    let response = provider
        .complete(request)
        .await
        .with_context(|| "Curator LLM call failed")?;
    let parsed = parse_curator_response(&response.text)
        .with_context(|| format!("invalid Curator JSON: {}", response.text))?;
    apply_actions(store, &batch, parsed, now_unix)
}

fn select_batch(store: &MemoryStore) -> Result<Vec<SkillRecord>> {
    let mut records = store
        .list_skill_records()
        .map_err(|err| anyhow!("list_skill_records: {err}"))?;
    records.retain(|record| record.skill.scope == "auto" && record.skill.archived_at_unix == 0);
    // Oldest last-used first; never-used rows (last_used_at_unix == 0)
    // come ahead of everything via the natural u64 ordering.
    records.sort_by_key(|record| record.skill.last_used_at_unix);
    records.truncate(MAX_SKILLS_PER_RUN);
    Ok(records)
}

fn build_user_prompt(batch: &[SkillRecord], now_unix: u64) -> String {
    let mut prompt = String::with_capacity(batch.len() * 512);
    prompt.push_str("Review these auto-skills and emit one action per name. ");
    prompt.push_str("Skills not represented in your JSON are treated as `keep`.\n\n");
    for record in batch {
        let idle_days = (now_unix
            .saturating_sub(record.skill.last_used_at_unix.max(record.updated_at_unix)))
            / (24 * 3600);
        prompt.push_str(&format!(
            "### {}\nlast_used_days_ago: {}\n---\n{}\n\n",
            record.skill.name,
            idle_days,
            record.skill.body.trim()
        ));
    }
    prompt
}

fn parse_curator_response(text: &str) -> Result<CuratorResponse> {
    let trimmed = text.trim();
    let body = trimmed
        .strip_prefix("```json")
        .or_else(|| trimmed.strip_prefix("```"))
        .map(str::trim_start)
        .unwrap_or(trimmed);
    let body = body.trim_end_matches("```").trim();
    serde_json::from_str(body).map_err(|err| anyhow!("JSON parse: {err}"))
}

fn apply_actions(
    store: &MemoryStore,
    batch: &[SkillRecord],
    response: CuratorResponse,
    now_unix: u64,
) -> Result<CuratorReport> {
    let mut report = CuratorReport {
        evaluated: batch.iter().map(|r| r.skill.name.clone()).collect(),
        ..Default::default()
    };
    for action in response.actions {
        let Some(record) = batch.iter().find(|r| r.skill.name == action.name) else {
            report.ignored.push(action.name);
            continue;
        };
        match action.action.as_str() {
            "keep" => report.applied.push((action.name, "keep".into())),
            "patch" => {
                if let Some(body) = action.body {
                    let mut updated = record.skill.clone();
                    updated.body = body;
                    store
                        .save_skill(&updated)
                        .map_err(|err| anyhow!("patch save_skill: {err}"))?;
                    report.applied.push((action.name, "patch".into()));
                } else {
                    report.ignored.push(action.name);
                }
            }
            "consolidate" => {
                let Some(target_name) = action.merge_into.as_deref() else {
                    report.ignored.push(action.name);
                    continue;
                };
                let Some(target) = batch.iter().find(|r| r.skill.name == target_name) else {
                    report.ignored.push(action.name);
                    continue;
                };
                if let Some(body) = action.body {
                    let mut updated = target.skill.clone();
                    updated.body = body;
                    store
                        .save_skill(&updated)
                        .map_err(|err| anyhow!("consolidate target save_skill: {err}"))?;
                }
                store
                    .set_skill_archived(&action.name, now_unix)
                    .map_err(|err| anyhow!("consolidate archive: {err}"))?;
                report
                    .applied
                    .push((action.name, format!("consolidate→{target_name}")));
            }
            "archive" => {
                store
                    .set_skill_archived(&action.name, now_unix)
                    .map_err(|err| anyhow!("archive: {err}"))?;
                report.applied.push((action.name, "archive".into()));
            }
            _ => report.ignored.push(action.name),
        }
    }
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use peridot_memory::StoredSkill;

    fn rec(name: &str, body: &str) -> SkillRecord {
        SkillRecord {
            skill: StoredSkill {
                name: name.into(),
                body: body.into(),
                scope: "auto".into(),
                ..Default::default()
            },
            updated_at_unix: 0,
        }
    }

    #[test]
    fn parses_well_formed_response() {
        let raw = r#"{"actions":[{"name":"a","action":"keep"},{"name":"b","action":"archive"}]}"#;
        let parsed = parse_curator_response(raw).unwrap();
        assert_eq!(parsed.actions.len(), 2);
        assert_eq!(parsed.actions[1].action, "archive");
    }

    #[test]
    fn parses_response_wrapped_in_code_fence() {
        let raw = "```json\n{\"actions\":[{\"name\":\"a\",\"action\":\"keep\"}]}\n```";
        let parsed = parse_curator_response(raw).unwrap();
        assert_eq!(parsed.actions.len(), 1);
    }

    #[test]
    fn invalid_json_returns_error() {
        assert!(parse_curator_response("not json").is_err());
    }

    #[test]
    fn apply_actions_handles_each_action() {
        let root = std::env::temp_dir().join(format!(
            "peridot-curator-actions-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or_default()
        ));
        let store = MemoryStore::new(root.join("memory.db"));
        store
            .save_skill(&StoredSkill {
                name: "a".into(),
                body: "old a".into(),
                scope: "auto".into(),
                ..Default::default()
            })
            .unwrap();
        store
            .save_skill(&StoredSkill {
                name: "b".into(),
                body: "old b".into(),
                scope: "auto".into(),
                ..Default::default()
            })
            .unwrap();
        store
            .save_skill(&StoredSkill {
                name: "c".into(),
                body: "old c".into(),
                scope: "auto".into(),
                ..Default::default()
            })
            .unwrap();
        let batch = vec![rec("a", "old a"), rec("b", "old b"), rec("c", "old c")];
        let response = CuratorResponse {
            actions: vec![
                CuratorAction {
                    name: "a".into(),
                    action: "patch".into(),
                    body: Some("rewritten a".into()),
                    merge_into: None,
                },
                CuratorAction {
                    name: "b".into(),
                    action: "consolidate".into(),
                    body: Some("merged".into()),
                    merge_into: Some("a".into()),
                },
                CuratorAction {
                    name: "c".into(),
                    action: "archive".into(),
                    body: None,
                    merge_into: None,
                },
                CuratorAction {
                    name: "ghost".into(),
                    action: "keep".into(),
                    body: None,
                    merge_into: None,
                },
            ],
        };
        let report = apply_actions(&store, &batch, response, 9_999).unwrap();
        assert_eq!(report.evaluated.len(), 3);
        assert_eq!(report.applied.len(), 3, "patch + consolidate + archive");
        assert!(report.ignored.iter().any(|n| n == "ghost"));

        // a got the consolidated body (consolidate target overrides patch).
        let active = store.list_skills().unwrap();
        let a = active.iter().find(|s| s.name == "a").unwrap();
        assert_eq!(a.body, "merged");
        // b and c are archived (excluded from list_skills).
        assert!(active.iter().all(|s| s.name != "b"));
        assert!(active.iter().all(|s| s.name != "c"));

        std::fs::remove_dir_all(root).unwrap();
    }
}
