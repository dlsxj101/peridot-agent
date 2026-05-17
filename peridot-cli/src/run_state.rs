use super::*;

pub(super) fn apply_resume(
    task: String,
    resume_id: Option<&str>,
    project_root: &Path,
) -> Result<String> {
    let Some(resume_id) = resume_id else {
        return Ok(task);
    };
    let store = MemoryStore::new(project_root.join(".peridot/memory.db"));
    let session = store
        .get_session(resume_id)?
        .with_context(|| format!("session not found: {resume_id}"))?;
    Ok(resume_task_text(&session.id, &session.summary, &task))
}

pub(super) fn resume_task_text(id: &str, summary: &str, task: &str) -> String {
    let task = task.trim();
    if task.is_empty() {
        format!("Resume session {id} from this summary: {summary}")
    } else {
        format!("Resume session {id} from this summary: {summary}\n\nCurrent task: {task}")
    }
}

pub(super) fn save_run_session(
    project_root: &Path,
    session_id: &str,
    summary: &AgentRunSummary,
    task: &str,
    memory: &MemoryConfig,
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
        )?;
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

pub(super) fn save_auto_skill(
    project_root: &Path,
    store: &MemoryStore,
    session_id: &str,
    summary: &AgentRunSummary,
    task: &str,
    needs_review: bool,
) -> Result<()> {
    let name = format!("auto-{}", slugify_for_branch(task));
    let body = auto_skill_body(session_id, summary, task, needs_review);
    store.save_skill(&StoredSkill {
        name: name.clone(),
        body: body.clone(),
        scope: "auto".to_string(),
        ..Default::default()
    })?;
    let skills_dir = project_root.join(".peridot/skills/auto");
    fs::create_dir_all(&skills_dir)?;
    fs::write(skills_dir.join(format!("{name}.md")), body)?;
    Ok(())
}

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
}
