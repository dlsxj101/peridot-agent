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
    if memory.auto_skills && summary.stopped_reason == StopReason::Done {
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
