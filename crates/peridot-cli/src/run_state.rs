use super::*;

pub(super) fn apply_resume(
    task: String,
    resume_id: Option<&str>,
    project_root: &Path,
) -> Result<String> {
    let Some(resume_id) = resume_id else {
        return Ok(task);
    };
    let resume = commands::session_resume_summary(project_root, resume_id)?;
    Ok(commands::session_resume_task_text(
        &resume.id,
        &resume.summary,
        &task,
    ))
}

pub(super) async fn save_run_session(
    project_root: &Path,
    session_id: &str,
    summary: &AgentRunSummary,
    task: &str,
    memory: &MemoryConfig,
    rewriter: Option<&dyn peridot_llm::LlmProvider>,
    rewriter_model: &str,
) -> Result<()> {
    if !memory.session_history {
        return Ok(());
    }
    let task = compact_summary_text(task, 160);
    let session = SessionSummary {
        id: session_id.to_string(),
        summary: format!(
            "task=\"{}\" stopped={:?} turns={} cost=${:.6}",
            task,
            summary.stopped_reason,
            summary.turns.len(),
            summary.usage.estimated_cost_usd
        ),
    };
    let store = MemoryStore::new(project_root.join(".peridot/memory.db"));
    store.save_session(&session)?;
    if memory.auto_skills
        && summary.stopped_reason == StopReason::Done
        && skill_worth_saving(summary)
    {
        save_auto_skill(
            project_root,
            &store,
            session_id,
            summary,
            &task,
            memory.skills_review,
            rewriter,
            rewriter_model,
        )
        .await?;
    }
    // Persist the tool sequence + bump n-gram counters whenever
    // cross-session reflection is enabled. Runs on EVERY completed
    // session (not just the 4-condition-gated ones) so we observe the
    // patterns that show up across many cheap sessions too — which is
    // exactly what reflection wants to see. Cheap: one INSERT + N
    // UPSERTs gated by the per-session cap.
    if memory.auto_skill_reflection {
        let tool_sequence: Vec<String> = summary
            .turns
            .iter()
            .map(|turn| turn.tool_name.clone())
            .collect();
        let now = unix_timestamp();
        // Best-effort: a failed sequence save doesn't break the run.
        let _ = store.save_tool_sequence(
            session_id,
            &tool_sequence,
            &task,
            memory.ngram_max_length,
            now,
        );
    }
    Ok(())
}

/// Hermes-style 4-condition OR gate. Skip auto-skill capture for trivial
/// sessions ("hi", quick lookups) so `.peridot/skills/auto/` stays
/// signal-rich. A run earns a skill when it shows complexity, recovery,
/// user collaboration, or workflow breadth — any one is enough.
pub(super) fn skill_worth_saving(summary: &AgentRunSummary) -> bool {
    const MIN_TURNS_FOR_COMPLEX: usize = 5;
    const MIN_DISTINCT_TOOLS: usize = 3;

    let turns = &summary.turns;
    if turns.len() >= MIN_TURNS_FOR_COMPLEX {
        return true;
    }
    if turns.iter().any(|t| !t.tool_result.success) {
        return true;
    }
    if turns.iter().any(|t| t.tool_name == "agent_ask_user") {
        return true;
    }
    turns
        .iter()
        .map(|t| t.tool_name.as_str())
        .collect::<std::collections::HashSet<_>>()
        .len()
        >= MIN_DISTINCT_TOOLS
}

// Argument list mirrors what the caller already threads through —
// folding them into a struct would just shuffle the count from the
// function signature into a `SaveSkillArgs` builder. The lint is
// silenced rather than masked.
#[allow(clippy::too_many_arguments)]
pub(super) async fn save_auto_skill(
    project_root: &Path,
    store: &MemoryStore,
    session_id: &str,
    summary: &AgentRunSummary,
    task: &str,
    needs_review: bool,
    rewriter: Option<&dyn peridot_llm::LlmProvider>,
    rewriter_model: &str,
) -> Result<()> {
    let name = format!("auto-{}", slugify_for_branch(task));
    // Prefer the LLM-rewritten body (Hermes-style SKILL.md with YAML
    // frontmatter + procedure/pitfalls/verification sections) when a
    // provider is available. Falls back to the deterministic template
    // when the LLM is missing, errors, or returns something that
    // doesn't look like a SKILL.md — silent fallback beats blocking
    // session save on a flaky provider.
    let rewritten = match rewriter {
        Some(provider) => llm_rewrite_skill_body(provider, rewriter_model, task, summary).await,
        None => None,
    };
    let (body, description) = match rewritten {
        Some(r) => (r.body, r.description),
        None => (
            auto_skill_body(session_id, summary, task, needs_review),
            String::new(),
        ),
    };
    store.save_skill(&StoredSkill {
        name: name.clone(),
        body: body.clone(),
        scope: "auto".to_string(),
        description,
        ..Default::default()
    })?;
    let skills_dir = project_root.join(".peridot/skills/auto");
    fs::create_dir_all(&skills_dir)?;
    fs::write(skills_dir.join(format!("{name}.md")), body)?;
    Ok(())
}

/// Hermes-style template fallback. Kept as the safety net when no
/// provider is wired (mock-response sessions, offline daemons), or
/// when the LLM call fails. Has no frontmatter so `description` stays
/// empty — distinguishable from an LLM-rewritten skill at L0.
pub(super) fn auto_skill_body(
    session_id: &str,
    summary: &AgentRunSummary,
    task: &str,
    needs_review: bool,
) -> String {
    let review = if needs_review { "true" } else { "false" };
    let tools = summary
        .turns
        .iter()
        .map(|turn| format!("- {}: {}", turn.tool_name, turn.tool_result.summary))
        .collect::<Vec<_>>()
        .join("\n");
    format!(
        "# Auto Skill: {task}\n\nreview_required: {review}\nsession: {session_id}\n\n## When To Use\nRepeat this pattern for similar tasks.\n\n## Observed Steps\n{tools}\n"
    )
}

/// LLM-rewritten skill body. Returns `None` whenever the call fails or
/// the output doesn't parse as a SKILL.md with YAML frontmatter —
/// silent failure is preferred over a half-formed skill polluting the
/// store, since the deterministic template is always a safe fallback.
pub(super) struct RewrittenSkill {
    pub body: String,
    pub description: String,
}

pub(super) async fn llm_rewrite_skill_body(
    provider: &dyn peridot_llm::LlmProvider,
    model: &str,
    task: &str,
    summary: &AgentRunSummary,
) -> Option<RewrittenSkill> {
    // Compact the turn log to ~25 entries so we don't blow the prompt
    // window on long sessions. Each entry is "name: summary".
    let tool_log: String = summary
        .turns
        .iter()
        .take(25)
        .map(|t| {
            let one_line = t.tool_result.summary.replace('\n', " ");
            format!(
                "- {}: {}",
                t.tool_name,
                compact_summary_text(&one_line, 200)
            )
        })
        .collect::<Vec<_>>()
        .join("\n");

    let system = SKILL_REWRITER_SYSTEM_PROMPT.to_string();
    let user = format!(
        "Task description:\n{task}\n\nTool log from the completed session:\n{tool_log}\n\nWrite the SKILL.md now."
    );
    let request = peridot_llm::CompletionRequest {
        model: model.to_string(),
        system: Some(system),
        messages: vec![peridot_llm::LlmMessage::new(
            peridot_llm::MessageRole::User,
            user,
        )],
        max_tokens: Some(1200),
        thinking: false,
        reasoning_effort: peridot_common::ReasoningEffort::Off,
        service_tier: None,
        tools: Vec::new(),
        tool_choice: Default::default(),
    };
    let response = provider.complete(request).await.ok()?;
    let body = response.text.trim().to_string();
    if !looks_like_skill_md(&body) {
        return None;
    }
    let description = extract_yaml_field(&body, "description").unwrap_or_default();
    Some(RewrittenSkill { body, description })
}

const SKILL_REWRITER_SYSTEM_PROMPT: &str = "\
You write reusable agent SKILL.md files in the Hermes-style format. \
Given a task and the tool log from a session that solved it, produce \
exactly one Markdown document with these sections:\n\
\n\
1. YAML frontmatter (between `---` fences) carrying `name`, \
`description` (one sentence, what the skill is for), `version` (`1`), \
`tags` (array of 1-4 short kebab-case tags).\n\
2. `## When to Use` — 1-3 bullet points naming the situations this \
skill applies to.\n\
3. `## Procedure` — numbered steps that a future agent can replay. \
Abstract away project-specific details (paths, branch names, PR \
numbers) — phrase steps so they reuse, not just describe.\n\
4. `## Pitfalls` — 1-3 bullets on what to watch out for.\n\
5. `## Verification` — one sentence on how to confirm the skill \
worked.\n\
\n\
Reply with ONLY the SKILL.md content, starting at the opening `---`. \
No commentary, no code fences around the whole output.";

/// Cheap structural check to reject obviously broken LLM output before
/// we store it. We require: starts with a YAML frontmatter fence,
/// contains a `name:` line in the frontmatter, and contains at least
/// one `##` section header. Anything else falls back to the template.
fn looks_like_skill_md(body: &str) -> bool {
    let trimmed = body.trim_start();
    if !trimmed.starts_with("---") {
        return false;
    }
    // Take just the frontmatter (between first two `---` fences).
    let after_first = match trimmed.find("---").map(|i| &trimmed[i + 3..]) {
        Some(t) => t,
        None => return false,
    };
    let Some(close_idx) = after_first.find("\n---") else {
        return false;
    };
    let frontmatter = &after_first[..close_idx];
    frontmatter.contains("name:") && body.contains("##")
}

/// Pull a single top-level YAML field value out of the frontmatter.
/// Intentionally a minimal parser — we only want one or two fields and
/// pulling in a YAML dep just for this is overkill. Returns the trimmed
/// value with surrounding quotes stripped; multi-line or list values
/// are returned as-is (good enough for the description case).
fn extract_yaml_field(body: &str, key: &str) -> Option<String> {
    let trimmed = body.trim_start_matches("---").trim_start();
    for line in trimmed.lines() {
        let line = line.trim_end();
        if line == "---" {
            break;
        }
        if let Some(rest) = line.strip_prefix(&format!("{key}:")) {
            let value = rest.trim().trim_matches('"').trim_matches('\'').to_string();
            if value.is_empty() {
                return None;
            }
            return Some(value);
        }
    }
    None
}

pub(super) fn auto_commit_run(
    project_root: &Path,
    config: &PeridotConfig,
    summary: &AgentRunSummary,
    task: &str,
) -> Result<Option<String>> {
    if !config.git.auto_commit || summary.stopped_reason != StopReason::Done {
        return Ok(None);
    }
    let manager = GitManager::new(project_root);
    if !manager.is_repository() {
        return Ok(None);
    }
    let status = manager.status()?;
    if status.changed_files.is_empty() {
        return Ok(None);
    }
    if config.git.auto_branch {
        ensure_auto_branch(&manager, &config.git.branch_prefix, task)?;
    }
    let message = commit_message_for_task(task, &config.git.commit_message_style);
    manager.commit_all(&message)?;
    Ok(Some(message))
}

pub(super) fn ensure_auto_branch(
    manager: &GitManager,
    branch_prefix: &str,
    task: &str,
) -> Result<()> {
    let status = manager.status()?;
    let current = status.branch.unwrap_or_default();
    if current.starts_with(branch_prefix) {
        return Ok(());
    }
    let branch = format!(
        "{}{}-{}",
        branch_prefix,
        slugify_for_branch(task),
        unix_timestamp()
    );
    manager.create_branch(&branch)?;
    Ok(())
}

pub(super) fn commit_message_for_task(task: &str, style: &str) -> String {
    let subject = compact_summary_text(task, 64)
        .trim_matches('"')
        .trim()
        .to_string();
    if style == "conventional" {
        format!("chore(agent): {}", fallback_subject(&subject))
    } else {
        fallback_subject(&subject).to_string()
    }
}

pub(super) fn fallback_subject(subject: &str) -> &str {
    if subject.is_empty() {
        "complete agent task"
    } else {
        subject
    }
}

pub(super) fn slugify_for_branch(task: &str) -> String {
    let mut slug = String::new();
    let mut last_dash = false;
    for ch in task.chars().flat_map(char::to_lowercase) {
        if ch.is_ascii_alphanumeric() {
            slug.push(ch);
            last_dash = false;
        } else if !last_dash && !slug.is_empty() {
            slug.push('-');
            last_dash = true;
        }
        if slug.len() >= 40 {
            break;
        }
    }
    let slug = slug.trim_matches('-');
    if slug.is_empty() {
        "task".to_string()
    } else {
        slug.to_string()
    }
}

pub(super) fn compact_summary_text(value: &str, max_chars: usize) -> String {
    let value = value.split_whitespace().collect::<Vec<_>>().join(" ");
    if value.chars().count() <= max_chars {
        return value;
    }
    let mut compact = value
        .chars()
        .take(max_chars.saturating_sub(3))
        .collect::<String>();
    compact.push_str("...");
    compact
}

pub(super) fn unix_timestamp() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use peridot_common::ToolResult;
    use peridot_core::AgentTurnOutcome;
    use peridot_llm::Usage;
    use serde_json::json;

    fn turn(name: &str, success: bool) -> AgentTurnOutcome {
        AgentTurnOutcome {
            tool_name: name.to_string(),
            tool_result: if success {
                ToolResult::success("ok", json!(null))
            } else {
                ToolResult::failure("err")
            },
            usage: Usage::default(),
            done: false,
        }
    }

    fn summary_for(turns: Vec<AgentTurnOutcome>) -> AgentRunSummary {
        AgentRunSummary {
            turns,
            usage: Usage::default(),
            stopped_reason: StopReason::Done,
            duration_ms: 0,
        }
    }

    #[test]
    fn trivial_two_turn_session_is_not_saved() {
        let s = summary_for(vec![turn("file_read", true), turn("file_read", true)]);
        assert!(!skill_worth_saving(&s));
    }

    #[test]
    fn five_turn_session_passes_complexity_gate() {
        let s = summary_for((0..5).map(|_| turn("file_read", true)).collect());
        assert!(skill_worth_saving(&s));
    }

    #[test]
    fn any_failed_turn_unlocks_recovery_gate() {
        let s = summary_for(vec![turn("file_read", true), turn("shell_exec", false)]);
        assert!(skill_worth_saving(&s));
    }

    #[test]
    fn ask_user_call_unlocks_correction_gate() {
        let s = summary_for(vec![turn("file_read", true), turn("agent_ask_user", true)]);
        assert!(skill_worth_saving(&s));
    }

    #[test]
    fn three_distinct_tools_unlocks_breadth_gate() {
        let s = summary_for(vec![
            turn("file_read", true),
            turn("file_write", true),
            turn("shell_exec", true),
        ]);
        assert!(skill_worth_saving(&s));
    }

    #[test]
    fn looks_like_skill_md_accepts_well_formed_output() {
        let valid = "\
---
name: ship-daily
description: Daily release flow
version: 1
tags: [release, daily]
---

## When to Use
- Friday afternoons

## Procedure
1. step
";
        assert!(super::looks_like_skill_md(valid));
    }

    #[test]
    fn looks_like_skill_md_rejects_missing_frontmatter() {
        let invalid = "# Auto Skill: foo\n\n## When to Use\n- bar\n";
        assert!(!super::looks_like_skill_md(invalid));
    }

    #[test]
    fn looks_like_skill_md_rejects_missing_name_field() {
        let invalid = "---\ndescription: x\n---\n\n## Section\n";
        assert!(!super::looks_like_skill_md(invalid));
    }

    #[test]
    fn extract_yaml_field_returns_description() {
        let body = "---\nname: ship-daily\ndescription: \"Daily release flow\"\n---\n\nrest";
        assert_eq!(
            super::extract_yaml_field(body, "description"),
            Some("Daily release flow".to_string())
        );
    }

    #[test]
    fn extract_yaml_field_returns_none_when_absent() {
        let body = "---\nname: x\n---\n\nrest";
        assert!(super::extract_yaml_field(body, "description").is_none());
    }
}
