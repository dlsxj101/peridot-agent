//! Hermes-style LLM Curator.
//!
//! Sub-agent that periodically reviews `scope='auto'` skills produced by
//! the harness. The CLI command (`peridot skill curate --llm`) and the
//! 7-day idle auto-trigger both call into `run_llm_curator`. Each pass
//! picks one of four actions per skill:
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
//!
//! Archived rows have their `.md` files moved from `.peridot/skills/auto/`
//! to `.peridot/skills/archive/` so the operator can restore by hand if
//! the Curator made a bad call.

use std::path::Path;

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
    project_root: &Path,
    now_unix: u64,
) -> Result<CuratorReport> {
    let batch = select_batch(store)?;
    if batch.is_empty() {
        return Ok(CuratorReport::default());
    }
    // Snapshot the on-disk skill files BEFORE the LLM gets to rewrite
    // them. If a `consolidate` action goes sideways and merges two
    // unrelated skills together, the operator can recover by copying
    // out of `.peridot/skills/.snapshots/<unix>/`. Best-effort: a
    // snapshot failure logs to stderr and lets the Curator continue
    // — refusing to run the Curator because of a snapshot copy error
    // would be a worse failure mode.
    if let Err(err) = snapshot_skills_dir(project_root, now_unix) {
        eprintln!("warning: curator snapshot failed: {err}");
    }
    // While we're here, prune snapshots older than the configured
    // retention window so we don't accumulate them forever.
    let _ = prune_old_skill_snapshots(project_root, now_unix, SNAPSHOT_RETENTION_SECS);

    let prompt = build_user_prompt(&batch, now_unix);
    let request = CompletionRequest {
        model: model.to_string(),
        system: Some(SYSTEM_PROMPT.to_string()),
        messages: vec![LlmMessage::new(MessageRole::User, prompt)],
        max_tokens: Some(4096),
        thinking: false,
        reasoning_effort: ReasoningEffort::Off,
        service_tier: None,
        tools: Vec::new(),
        tool_choice: ToolChoice::None,
    };
    let response = provider
        .complete(request)
        .await
        .with_context(|| "Curator LLM call failed")?;
    let parsed = parse_curator_response(&response.text)
        .with_context(|| format!("invalid Curator JSON: {}", response.text))?;
    apply_actions(store, project_root, &batch, parsed, now_unix)
}

/// Archives a single skill atomically: stamps the DB row and moves
/// `.peridot/skills/auto/<name>.md` to `.peridot/skills/archive/<name>.md`
/// when the file exists. fs operations are best-effort — a missing
/// source file is fine (manual cleanup), but a rename failure surfaces
/// as an error so the caller can decide whether to roll back the DB.
pub(crate) fn archive_skill_with_file(
    store: &MemoryStore,
    project_root: &Path,
    name: &str,
    now_unix: u64,
) -> Result<()> {
    store
        .set_skill_archived(name, now_unix)
        .map_err(|err| anyhow!("set_skill_archived({name}): {err}"))?;
    let source = project_root
        .join(".peridot/skills/auto")
        .join(format!("{name}.md"));
    if !source.exists() {
        return Ok(());
    }
    let archive_dir = project_root.join(".peridot/skills/archive");
    std::fs::create_dir_all(&archive_dir)
        .with_context(|| format!("creating {}", archive_dir.display()))?;
    let target = archive_dir.join(format!("{name}.md"));
    std::fs::rename(&source, &target)
        .with_context(|| format!("renaming {} -> {}", source.display(), target.display()))?;
    Ok(())
}

/// Curator snapshot retention. 30 days mirrors the Hermes "stale"
/// threshold — anything older than that is well past the window where
/// a rollback would still be useful, since the LLM Curator has
/// touched the skills several more times in the meantime.
const SNAPSHOT_RETENTION_SECS: u64 = 30 * 24 * 3600;

/// Copy `.peridot/skills/auto/` into a timestamped subdirectory under
/// `.peridot/skills/.snapshots/<now_unix>/`. Used as a rollback point
/// before the Curator's LLM-driven rewrite phase. Missing source dir
/// is treated as a no-op (fresh project with no auto-skills yet);
/// other I/O errors bubble up so the caller can log them.
fn snapshot_skills_dir(project_root: &Path, now_unix: u64) -> Result<()> {
    let source = project_root.join(".peridot/skills/auto");
    if !source.is_dir() {
        return Ok(());
    }
    let target = project_root
        .join(".peridot/skills/.snapshots")
        .join(format!("{now_unix}"));
    std::fs::create_dir_all(&target)
        .with_context(|| format!("create snapshot dir {}", target.display()))?;
    copy_dir_recursive(&source, &target)
        .with_context(|| format!("copy {} -> {}", source.display(), target.display()))?;
    Ok(())
}

/// Drop any snapshot directories whose name (unix seconds) is older
/// than `retention_secs` before `now_unix`. Quietly skips files
/// whose name doesn't parse as a u64 — those are operator-renamed
/// keepsakes (e.g. `.snapshots/before-big-merge/`) and we shouldn't
/// delete them just because they don't look like our timestamps.
fn prune_old_skill_snapshots(
    project_root: &Path,
    now_unix: u64,
    retention_secs: u64,
) -> Result<()> {
    let snapshots = project_root.join(".peridot/skills/.snapshots");
    if !snapshots.is_dir() {
        return Ok(());
    }
    let cutoff = now_unix.saturating_sub(retention_secs);
    for entry in std::fs::read_dir(&snapshots)? {
        let entry = entry?;
        let name = entry.file_name();
        let name_str = name.to_string_lossy();
        let Ok(ts) = name_str.parse::<u64>() else {
            continue;
        };
        if ts < cutoff {
            let _ = std::fs::remove_dir_all(entry.path());
        }
    }
    Ok(())
}

/// Recursive directory copy. Stdlib has no equivalent; this is a
/// minimal walker that creates the target tree and `copy()`s each
/// file. Used by `snapshot_skills_dir` — too small to justify pulling
/// in `walkdir`.
fn copy_dir_recursive(from: &Path, to: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(to)?;
    for entry in std::fs::read_dir(from)? {
        let entry = entry?;
        let path = entry.path();
        let dest = to.join(entry.file_name());
        if path.is_dir() {
            copy_dir_recursive(&path, &dest)?;
        } else {
            std::fs::copy(&path, &dest)?;
        }
    }
    Ok(())
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
    project_root: &Path,
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
                archive_skill_with_file(store, project_root, &action.name, now_unix)?;
                report
                    .applied
                    .push((action.name, format!("consolidate→{target_name}")));
            }
            "archive" => {
                archive_skill_with_file(store, project_root, &action.name, now_unix)?;
                report.applied.push((action.name, "archive".into()));
            }
            _ => report.ignored.push(action.name),
        }
    }
    Ok(report)
}

// ============================================================
// Cross-session reflection — n-gram promotion.
// ============================================================
//
// While `run_llm_curator` reviews skills the harness already created
// (one skill per qualifying session), `run_ngram_reflection` watches
// the `tool_ngrams` table and promotes patterns the operator runs
// repeatedly across many sessions into their own skill.
//
// This is the second half of Hermes' Self-Improvement Loop: not "did
// this one session look skill-worthy?" but "is the operator doing X
// over and over across many sessions?"

/// Result of one reflection pass.
#[derive(Debug, Default)]
pub struct ReflectionReport {
    /// (skill_name, ngram_tools_joined) pairs that were created.
    pub promoted: Vec<(String, String)>,
    /// N-grams considered but skipped (collision, parse failure, …).
    pub skipped: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct ReflectionItem {
    /// Pipe-joined tool names, must match the candidate's
    /// `ngram_tools` line exactly so the LLM cannot promote a pattern
    /// the operator never ran.
    tools: String,
    /// Whether to promote this pattern at all. Allows the LLM to skip
    /// junk patterns ("file_list, file_list, file_read" is just
    /// browsing) without us doing the filtering.
    #[serde(default = "default_true_bool")]
    promote: bool,
    /// Title for the skill heading. Forced kebab-case below to keep
    /// filenames safe.
    title: String,
    /// Markdown body — the model writes "when to use", "what it does",
    /// "watch out for" prose grounded in the supplied task summaries.
    body: String,
}

fn default_true_bool() -> bool {
    true
}

#[derive(Debug, Deserialize)]
struct ReflectionResponse {
    items: Vec<ReflectionItem>,
}

const REFLECTION_SYSTEM_PROMPT: &str = "\
You are the Peridot Reflection sub-agent. The harness has been recording\n\
tool-call sequences across many sessions. You receive a small batch of\n\
n-grams (length 2-3) that have shown up at least N times along with the\n\
task summaries from those sessions. For each pattern, decide:\n\
\n\
- promote=true: this is a real, repeated workflow worth saving as a\n\
  skill so the next agent recognises it faster. Write a useful body\n\
  (when to use, what it does, edges to watch). Keep the body under\n\
  ~400 words.\n\
- promote=false: this pattern is a coincidence, exploration noise, or\n\
  too generic to be useful. Skip it.\n\
\n\
Respond with strict JSON, no prose, no code fences:\n\
{\"items\":[{\"tools\":\"<verbatim pipe-joined names>\",\"promote\":true|false,\
\"title\":\"<short kebab-case skill title>\",\"body\":\"<markdown body>\"}]}\n\
\n\
The `tools` value MUST match the candidate's tools line exactly so the\n\
harness can correlate; do not paraphrase or reorder. Prefer promote=false\n\
when a pattern is purely informational (file_read pairs), and prefer\n\
promote=true when there's a clear write/verify/commit story.";

/// Runs one reflection pass: pulls promotion candidates from the
/// store, asks the LLM whether each is skill-worthy, and saves the
/// promoted ones as `scope='auto'` skills marked for human review.
///
/// Caller is responsible for gating on
/// `MemoryConfig::auto_skill_reflection`. Best-effort: any failure is
/// surfaced as a `ReflectionReport` skip line, never an error, so a
/// 7-day idle trigger never blocks startup on Curator's account.
#[allow(clippy::too_many_arguments)]
pub async fn run_ngram_reflection(
    provider: &dyn LlmProvider,
    model: &str,
    store: &MemoryStore,
    project_root: &Path,
    min_count: u32,
    batch_cap: usize,
    now_unix: u64,
    needs_review: bool,
) -> Result<ReflectionReport> {
    let candidates = store
        .list_promotion_candidates(min_count, batch_cap)
        .map_err(|err| anyhow!("list_promotion_candidates: {err}"))?;
    let (candidates, noisy_candidates) = split_reflection_candidates(candidates);
    let mut report = ReflectionReport::default();
    for candidate in noisy_candidates {
        let _ = store.mark_ngram_promoted(&candidate.hash, now_unix);
        report
            .skipped
            .push(format!("noise n-gram: {}", candidate.tools.join("|")));
    }
    if candidates.is_empty() {
        return Ok(report);
    }
    let prompt = build_reflection_prompt(&candidates);
    let request = CompletionRequest {
        model: model.to_string(),
        system: Some(REFLECTION_SYSTEM_PROMPT.to_string()),
        messages: vec![LlmMessage::new(MessageRole::User, prompt)],
        max_tokens: Some(4096),
        thinking: false,
        reasoning_effort: ReasoningEffort::Off,
        service_tier: None,
        tools: Vec::new(),
        tool_choice: ToolChoice::None,
    };
    let response = provider
        .complete(request)
        .await
        .with_context(|| "Reflection LLM call failed")?;
    let parsed = parse_reflection_response(&response.text)
        .with_context(|| format!("invalid Reflection JSON: {}", response.text))?;
    let llm_report = apply_reflection_items(
        store,
        project_root,
        &candidates,
        parsed,
        now_unix,
        needs_review,
    )?;
    report.promoted.extend(llm_report.promoted);
    report.skipped.extend(llm_report.skipped);
    Ok(report)
}

fn split_reflection_candidates(
    candidates: Vec<peridot_memory::ToolNgram>,
) -> (
    Vec<peridot_memory::ToolNgram>,
    Vec<peridot_memory::ToolNgram>,
) {
    candidates
        .into_iter()
        .partition(|candidate| !is_repetitive_ngram(candidate))
}

fn is_repetitive_ngram(candidate: &peridot_memory::ToolNgram) -> bool {
    candidate
        .tools
        .iter()
        .collect::<std::collections::HashSet<_>>()
        .len()
        <= 1
}

fn build_reflection_prompt(candidates: &[peridot_memory::ToolNgram]) -> String {
    let mut prompt = String::with_capacity(candidates.len() * 256);
    prompt.push_str(
        "Here are the candidate n-grams. Each shows the exact tool sequence \
         (pipe-joined), how many times it appeared, and the task summary from \
         the most recent session that produced it.\n\n",
    );
    for ngram in candidates {
        prompt.push_str(&format!(
            "### tools: {}\noccurrences: {}\nlast_task: {}\n\n",
            ngram.tools.join("|"),
            ngram.occurrence_count,
            ngram.last_task_summary.trim(),
        ));
    }
    prompt
}

fn parse_reflection_response(text: &str) -> Result<ReflectionResponse> {
    let trimmed = text.trim();
    let body = trimmed
        .strip_prefix("```json")
        .or_else(|| trimmed.strip_prefix("```"))
        .map(str::trim_start)
        .unwrap_or(trimmed);
    let body = body.trim_end_matches("```").trim();
    serde_json::from_str(body).map_err(|err| anyhow!("JSON parse: {err}"))
}

fn apply_reflection_items(
    store: &MemoryStore,
    project_root: &Path,
    candidates: &[peridot_memory::ToolNgram],
    response: ReflectionResponse,
    now_unix: u64,
    needs_review: bool,
) -> Result<ReflectionReport> {
    let mut report = ReflectionReport::default();
    for item in response.items {
        // Correlate the LLM's "tools" string back to the candidate
        // sent in. We don't trust the model to invent a new pattern.
        let Some(candidate) = candidates.iter().find(|c| c.tools.join("|") == item.tools) else {
            report
                .skipped
                .push(format!("unknown tools: {}", item.tools));
            continue;
        };
        if !item.promote {
            // Promote=false on a candidate still counts as "we've seen
            // it" — stamp promoted_at_unix so future passes don't ask
            // the LLM about it again. Caller still archives via
            // skill_curate if they want to reconsider.
            let _ = store.mark_ngram_promoted(&candidate.hash, now_unix);
            continue;
        }
        let title_slug = kebab_case(&item.title);
        let name = if title_slug.is_empty() {
            format!("pattern-{}", candidate.hash)
        } else {
            format!("pattern-{title_slug}")
        };
        // Collision guard: if a skill with this name already exists,
        // skip this item (don't overwrite curated content).
        let existing = store
            .list_skills()
            .map_err(|err| anyhow!("list_skills: {err}"))?;
        if existing.iter().any(|skill| skill.name == name) {
            report.skipped.push(format!("name collision: {name}"));
            continue;
        }
        let body = format_skill_body(&item, candidate, needs_review, now_unix);
        store
            .save_skill(&peridot_memory::StoredSkill {
                name: name.clone(),
                body: body.clone(),
                scope: "auto".to_string(),
                ..Default::default()
            })
            .map_err(|err| anyhow!("save_skill({name}): {err}"))?;
        let skills_dir = project_root.join(".peridot/skills/auto");
        std::fs::create_dir_all(&skills_dir)
            .with_context(|| format!("creating {}", skills_dir.display()))?;
        std::fs::write(skills_dir.join(format!("{name}.md")), &body)
            .with_context(|| format!("writing {name}.md"))?;
        let _ = store.mark_ngram_promoted(&candidate.hash, now_unix);
        report.promoted.push((name, candidate.tools.join("|")));
    }
    Ok(report)
}

fn kebab_case(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    let mut last_dash = false;
    for ch in value.chars().flat_map(char::to_lowercase) {
        if ch.is_ascii_alphanumeric() {
            out.push(ch);
            last_dash = false;
        } else if !last_dash && !out.is_empty() {
            out.push('-');
            last_dash = true;
        }
        if out.len() >= 48 {
            break;
        }
    }
    out.trim_matches('-').to_string()
}

fn format_skill_body(
    item: &ReflectionItem,
    candidate: &peridot_memory::ToolNgram,
    needs_review: bool,
    now_unix: u64,
) -> String {
    let review_flag = if needs_review { "true" } else { "false" };
    format!(
        "# {}\n\nreview_required: {review_flag}\nsource: reflection\noccurrences: {}\npromoted_at_unix: {}\ntool_pattern: {}\nlast_session: {}\n\n{}\n",
        item.title.trim(),
        candidate.occurrence_count,
        now_unix,
        candidate.tools.join(" → "),
        candidate.last_session_id,
        item.body.trim(),
    )
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

    fn ngram(hash: &str, tools: &[&str]) -> peridot_memory::ToolNgram {
        peridot_memory::ToolNgram {
            hash: hash.into(),
            tools: tools.iter().map(|tool| (*tool).into()).collect(),
            occurrence_count: 5,
            last_seen_unix: 1_700_000_000,
            promoted_at_unix: 0,
            last_session_id: "session-1".into(),
            last_task_summary: "task".into(),
        }
    }

    #[test]
    fn snapshot_skills_dir_copies_files_to_timestamped_subdir() {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "peridot-curator-snap-{}-{nanos}",
            std::process::id()
        ));
        std::fs::create_dir_all(root.join(".peridot/skills/auto")).unwrap();
        std::fs::write(
            root.join(".peridot/skills/auto/ship-daily.md"),
            "# Ship Daily\nstep1",
        )
        .unwrap();
        std::fs::write(
            root.join(".peridot/skills/auto/fix-parser.md"),
            "# Fix Parser",
        )
        .unwrap();

        snapshot_skills_dir(&root, 12_345).unwrap();

        let snap = root.join(".peridot/skills/.snapshots/12345");
        assert!(snap.join("ship-daily.md").is_file());
        assert!(snap.join("fix-parser.md").is_file());
        let body = std::fs::read_to_string(snap.join("ship-daily.md")).unwrap();
        assert!(body.contains("step1"));

        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn snapshot_no_op_when_skills_dir_missing() {
        // A fresh project with no auto-skills should not crash the
        // Curator just because there's nothing to snapshot.
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "peridot-curator-snap-noop-{}-{nanos}",
            std::process::id()
        ));
        std::fs::create_dir_all(&root).unwrap();
        snapshot_skills_dir(&root, 99).unwrap();
        assert!(!root.join(".peridot/skills/.snapshots/99").exists());
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn prune_old_snapshots_drops_only_aged_timestamp_dirs() {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "peridot-curator-prune-{}-{nanos}",
            std::process::id()
        ));
        let snaps = root.join(".peridot/skills/.snapshots");
        std::fs::create_dir_all(snaps.join("100")).unwrap(); // very old
        std::fs::create_dir_all(snaps.join("1000000")).unwrap(); // recent
        std::fs::create_dir_all(snaps.join("before-big-merge")).unwrap(); // operator-tagged

        // now = 1_000_500, retention = 1000 → drop anything before 999_500
        prune_old_skill_snapshots(&root, 1_000_500, 1000).unwrap();

        assert!(!snaps.join("100").exists(), "old snapshot should be pruned");
        assert!(
            snaps.join("1000000").exists(),
            "recent snapshot must remain"
        );
        assert!(
            snaps.join("before-big-merge").exists(),
            "non-numeric snapshot names must not be touched"
        );

        std::fs::remove_dir_all(root).ok();
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
        let report = apply_actions(&store, &root, &batch, response, 9_999).unwrap();
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

    #[test]
    fn reflection_response_parses_and_filters() {
        let raw = "{\"items\":[\
            {\"tools\":\"verify_build|git_commit|git_push\",\"promote\":true,\
             \"title\":\"ship daily\",\
             \"body\":\"## When to use\\n\\nWhen tests pass and you want to publish.\\n\"},\
            {\"tools\":\"file_read|file_read\",\"promote\":false,\
             \"title\":\"browse\",\"body\":\"\"}\
        ]}";
        let parsed = parse_reflection_response(raw).unwrap();
        assert_eq!(parsed.items.len(), 2);
        assert!(parsed.items[0].promote);
        assert!(!parsed.items[1].promote);
        assert_eq!(parsed.items[0].tools, "verify_build|git_commit|git_push");
    }

    #[test]
    fn reflection_candidates_drop_single_tool_noise_before_llm() {
        let candidates = vec![
            ngram("read-repeat", &["file_read", "file_read", "file_read"]),
            ngram("mixed", &["file_read", "file_write", "verify_build"]),
            ngram("shell-repeat", &["shell_exec", "shell_exec"]),
        ];

        let (eligible, noisy) = split_reflection_candidates(candidates);

        assert_eq!(eligible.len(), 1);
        assert_eq!(eligible[0].hash, "mixed");
        assert_eq!(
            noisy
                .iter()
                .map(|candidate| candidate.hash.as_str())
                .collect::<Vec<_>>(),
            vec!["read-repeat", "shell-repeat"]
        );
    }

    #[test]
    fn kebab_case_handles_spaces_and_punctuation() {
        assert_eq!(kebab_case("Ship Daily"), "ship-daily");
        assert_eq!(kebab_case("Build & test & push"), "build-test-push");
        assert_eq!(kebab_case("   leading   "), "leading");
        assert_eq!(kebab_case("!!@@##"), "");
    }

    #[test]
    fn apply_reflection_items_creates_skill_and_marks_promoted() {
        let root = std::env::temp_dir().join(format!(
            "peridot-reflection-apply-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or_default()
        ));
        let store = MemoryStore::new(root.join("memory.db"));
        store.initialize().unwrap();
        // Seed the n-gram so the candidate has a hash we can correlate.
        let tools: Vec<String> = vec!["verify_build", "git_commit", "git_push"]
            .into_iter()
            .map(String::from)
            .collect();
        for i in 0..5 {
            store
                .save_tool_sequence(
                    &format!("s-{i}"),
                    &tools,
                    "release the v0.8",
                    3,
                    1_700_000_000 + i as u64,
                )
                .unwrap();
        }
        let candidates = store.list_promotion_candidates(5, 10).unwrap();
        // The full trigram should be one of the candidates.
        let trigram = candidates
            .iter()
            .find(|c| c.tools.len() == 3)
            .expect("trigram candidate present")
            .clone();

        let response = ReflectionResponse {
            items: vec![ReflectionItem {
                tools: trigram.tools.join("|"),
                promote: true,
                title: "Ship Daily".into(),
                body: "Run verify_build, then commit, then push.".into(),
            }],
        };
        let report = apply_reflection_items(
            &store,
            &root,
            std::slice::from_ref(&trigram),
            response,
            1_700_001_000,
            true,
        )
        .unwrap();
        assert_eq!(report.promoted.len(), 1);
        assert_eq!(report.promoted[0].0, "pattern-ship-daily");
        assert!(report.skipped.is_empty());

        // Skill row exists, file written, n-gram stamped promoted.
        let skills = store.list_skills().unwrap();
        assert!(skills.iter().any(|s| s.name == "pattern-ship-daily"));
        let md_path = root.join(".peridot/skills/auto/pattern-ship-daily.md");
        assert!(md_path.exists());
        let leftover = store.list_promotion_candidates(5, 10).unwrap();
        assert!(
            leftover.iter().all(|c| c.hash != trigram.hash),
            "promoted ngram should not reappear as a candidate"
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn apply_reflection_items_skips_unknown_tools() {
        let root = std::env::temp_dir().join(format!(
            "peridot-reflection-unknown-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or_default()
        ));
        let store = MemoryStore::new(root.join("memory.db"));
        store.initialize().unwrap();
        let response = ReflectionResponse {
            items: vec![ReflectionItem {
                tools: "fabricated|tool|chain".into(),
                promote: true,
                title: "fake".into(),
                body: "should not land".into(),
            }],
        };
        let report =
            apply_reflection_items(&store, &root, &[], response, 1_700_000_000, true).unwrap();
        assert!(report.promoted.is_empty());
        assert_eq!(report.skipped.len(), 1);
        assert!(report.skipped[0].starts_with("unknown tools"));
        std::fs::remove_dir_all(root).unwrap();
    }
}
