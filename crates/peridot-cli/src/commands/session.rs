use super::*;

#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize)]
pub(crate) struct SessionSearchHit {
    pub(crate) session: String,
    pub(crate) index: usize,
    pub(crate) kind: String,
    pub(crate) text: String,
}

#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize)]
pub(crate) struct SessionSearchResult {
    pub(crate) query: String,
    pub(crate) total: usize,
    pub(crate) hits: Vec<SessionSearchHit>,
    pub(crate) truncated: bool,
}

#[derive(Clone, Debug, serde::Serialize)]
pub(crate) struct SessionShowResult {
    pub(crate) id: String,
    pub(crate) session: Option<SessionSummary>,
    pub(crate) record: Option<peridot_memory::SessionRecord>,
    pub(crate) notes_count: usize,
    pub(crate) last_note: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize)]
pub(crate) struct SessionLocateResult {
    pub(crate) id: String,
    pub(crate) path: String,
    pub(crate) exists: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize)]
pub(crate) struct SessionPruneResult {
    pub(crate) dry_run: bool,
    pub(crate) considered: Vec<String>,
    pub(crate) removed: Vec<String>,
    pub(crate) status_filter: Option<String>,
    pub(crate) older_than_days: Option<u64>,
}

#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize)]
pub(crate) struct SessionImportResult {
    pub(crate) id: String,
    pub(crate) source: String,
    pub(crate) destination: String,
    pub(crate) files: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize)]
pub(crate) struct SessionResumeResult {
    pub(crate) id: String,
    pub(crate) summary: String,
    pub(crate) resume_task: String,
}

#[derive(Clone, Debug, serde::Serialize)]
pub(crate) struct SessionReplayResult {
    pub(crate) id: String,
    pub(crate) entries: Vec<SessionReplayTranscriptEntry>,
    pub(crate) timeline: Vec<SessionReplayTimelineEntry>,
    pub(crate) total: usize,
    pub(crate) timeline_total: usize,
    pub(crate) committee_total: usize,
    pub(crate) truncated: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize)]
pub(crate) struct SessionReplayTranscriptEntry {
    pub(crate) kind: String,
    pub(crate) text: String,
    pub(crate) ts: u64,
}

#[derive(Clone, Debug, serde::Serialize)]
pub(crate) struct SessionReplayTimelineEntry {
    pub(crate) source: String,
    pub(crate) kind: String,
    pub(crate) marker: String,
    pub(crate) text: String,
    pub(crate) ts: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) event: Option<serde_json::Value>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct RewindContextResult {
    pub(crate) restored_prompt: String,
    pub(crate) removed_count: usize,
    pub(crate) kept_count: usize,
    pub(crate) rewind_turn_id: Option<u64>,
}

pub(crate) fn rewind_context_entries(
    entries: Vec<peridot_context::ContextEntry>,
) -> Result<(Vec<peridot_context::ContextEntry>, RewindContextResult), String> {
    let Some(user_idx) = entries
        .iter()
        .rposition(|entry| entry.source == peridot_context::ContextSource::User)
    else {
        return Err("rewind: no user message in context snapshot".to_string());
    };
    let restored_prompt = entries[user_idx].content.clone();
    let rewind_turn_id = match entries[user_idx].turn_id {
        0 => None,
        id => Some(id),
    };
    let split_at = rewind_turn_id
        .and_then(|turn_id| {
            entries
                .iter()
                .position(|entry| entry.turn_id != 0 && entry.turn_id >= turn_id)
        })
        .unwrap_or(user_idx);
    let kept: Vec<peridot_context::ContextEntry> = entries[..split_at].to_vec();
    let removed_count = entries.len().saturating_sub(kept.len());
    let kept_count = kept.len();
    Ok((
        kept,
        RewindContextResult {
            restored_prompt,
            removed_count,
            kept_count,
            rewind_turn_id,
        },
    ))
}

pub(crate) fn session_resume_task_text(id: &str, summary: &str, task: &str) -> String {
    let task = task.trim();
    if task.is_empty() {
        format!("Resume session {id} from this summary: {summary}")
    } else {
        format!("Resume session {id} from this summary: {summary}\n\nCurrent task: {task}")
    }
}

pub(crate) fn session_resume_summary(project_root: &Path, id: &str) -> Result<SessionResumeResult> {
    let store = memory_store(project_root);
    let session = store.get_session(id)?;
    let record = store.get_session_record(id).unwrap_or_default();
    let summary = session
        .as_ref()
        .map(|session| session.summary.trim())
        .filter(|summary| !summary.is_empty())
        .or_else(|| {
            record
                .as_ref()
                .map(|record| record.summary.trim())
                .filter(|summary| !summary.is_empty())
        })
        .or_else(|| {
            record
                .as_ref()
                .and_then(|record| record.last_task.as_deref())
                .map(str::trim)
                .filter(|task| !task.is_empty())
        })
        .with_context(|| format!("session not found: {id}"))?
        .to_string();
    Ok(SessionResumeResult {
        id: id.to_string(),
        resume_task: session_resume_task_text(id, &summary, ""),
        summary,
    })
}

pub(crate) fn run_session_command(
    command: &SessionCommand,
    project_root: &Path,
    output: OutputFormat,
) -> Result<()> {
    let store = memory_store(project_root);
    match command {
        SessionCommand::List { status } => {
            let sessions = store.list_sessions()?;
            let records = store.list_session_records().unwrap_or_default();
            let record_for = |id: &str| records.iter().find(|r| r.id == id).cloned();
            let summary_for = |id: &str| sessions.iter().find(|s| s.id == id).cloned();
            let mut ids: Vec<String> = sessions.iter().map(|session| session.id.clone()).collect();
            for record in &records {
                if !ids.iter().any(|id| id == &record.id) {
                    ids.push(record.id.clone());
                }
            }
            let status_filter = match status.as_deref() {
                Some(value) => Some(parse_lifecycle_filter(value)?),
                None => None,
            };
            let keep = |id: &str| match status_filter {
                Some(target) => record_for(id).map(|r| r.status == target).unwrap_or(false),
                None => true,
            };
            match output {
                OutputFormat::Json => {
                    let payload: Vec<_> = sessions
                        .iter()
                        .map(|session| session.id.clone())
                        .chain(records.iter().map(|record| record.id.clone()))
                        .fold(Vec::<String>::new(), |mut acc, id| {
                            if !acc.iter().any(|existing| existing == &id) {
                                acc.push(id);
                            }
                            acc
                        })
                        .into_iter()
                        .filter(|id| keep(id))
                        .map(|id| {
                            let session = summary_for(&id);
                            let record = record_for(&id);
                            serde_json::json!({
                                "id": id,
                                "summary": session
                                    .as_ref()
                                    .map(|session| session.summary.as_str())
                                    .or_else(|| record.as_ref().and_then(record_title))
                                    .unwrap_or(""),
                                "record": record,
                            })
                        })
                        .collect();
                    println!("{}", serde_json::to_string_pretty(&payload)?);
                }
                OutputFormat::Text => {
                    for id in ids {
                        if !keep(&id) {
                            continue;
                        }
                        let session = summary_for(&id);
                        let record = record_for(&id);
                        let summary = session
                            .as_ref()
                            .map(|session| session.summary.as_str())
                            .or_else(|| record.as_ref().and_then(record_title))
                            .unwrap_or("");
                        let suffix = record
                            .as_ref()
                            .map(|r| {
                                format!(
                                    "\tstatus={:?}\ttokens={}\tcost=${:.4}\tturns={}",
                                    r.status, r.total_tokens, r.total_cost_usd, r.turns_used,
                                )
                            })
                            .unwrap_or_default();
                        println!("{id}\t{summary}{suffix}");
                    }
                }
            }
        }
        SessionCommand::Resume { id } => {
            let resume = session_resume_summary(project_root, id)?;
            match output {
                OutputFormat::Json => println!(
                    "{}",
                    serde_json::to_string_pretty(&serde_json::json!({
                        "id": resume.id,
                        "summary": resume.summary,
                        "resume_task": resume.resume_task
                    }))?
                ),
                OutputFormat::Text => println!("{}", resume.resume_task),
            }
        }
        SessionCommand::Save { id, summary } => {
            let session = SessionSummary {
                id: id.clone(),
                summary: summary.join(" "),
            };
            store.save_session(&session)?;
            print_json_or_text_result(
                serde_json::json!({"saved": true, "id": id}),
                format!("saved session {id}"),
                output,
            )?;
        }
        SessionCommand::Show {
            id,
            notes_tail,
            transcript_tail,
            committee_tail,
        } => {
            let session = store.get_session(id)?;
            let record = store.get_session_record(id).unwrap_or_default();
            let (notes_count, last_note) = read_notes_summary(project_root, id);
            let notes_tail_entries = notes_tail
                .filter(|n| *n > 0)
                .map(|n| read_notes_tail(project_root, id, n));
            let transcript_tail_entries = transcript_tail.filter(|n| *n > 0).and_then(|n| {
                load_session_transcript(project_root, id).ok().map(|all| {
                    let start = all.len().saturating_sub(n);
                    all[start..].to_vec()
                })
            });
            let committee_tail_entries = committee_tail
                .filter(|n| *n > 0)
                .map(|n| read_committee_tail(project_root, id, n));
            match output {
                OutputFormat::Json => {
                    let tail_json: Option<Vec<_>> =
                        transcript_tail_entries.as_ref().map(|entries| {
                            entries
                                .iter()
                                .map(|entry| {
                                    serde_json::json!({
                                        "kind": format!("{:?}", entry.kind).to_ascii_lowercase(),
                                        "text": entry.text,
                                    })
                                })
                                .collect()
                        });
                    println!(
                        "{}",
                        serde_json::to_string_pretty(&serde_json::json!({
                            "session": session,
                            "record": record,
                            "notes_count": notes_count,
                            "last_note": last_note,
                            "notes_tail": notes_tail_entries,
                            "transcript_tail": tail_json,
                            "committee_tail": committee_tail_entries,
                        }))?
                    );
                }
                OutputFormat::Text => match (session, record) {
                    (Some(session), Some(record)) => {
                        println!("{}\t{}", session.id, session.summary);
                        println!(
                            "  status={:?} workspace={} tokens={} cost=${:.4} turns={}",
                            record.status,
                            record.workspace_root.display(),
                            record.total_tokens,
                            record.total_cost_usd,
                            record.turns_used,
                        );
                        if let Some(branch) = record.worktree_branch.as_deref() {
                            println!("  worktree branch: {branch}");
                        }
                        if let Some(task) = record.last_task.as_deref() {
                            println!("  last task: {task}");
                        }
                        if notes_count > 0 {
                            print!("  notes: {notes_count}");
                            if let Some(text) = last_note.as_deref() {
                                println!("  ({text})");
                            } else {
                                println!();
                            }
                        }
                        if let Some(entries) = notes_tail_entries.as_ref() {
                            for note in entries {
                                let ts = note["ts"].as_u64().unwrap_or_default();
                                let text = note["text"].as_str().unwrap_or("");
                                println!("    [{ts}] {text}");
                            }
                        }
                        if let Some(entries) = transcript_tail_entries.as_ref() {
                            println!("  transcript (last {}):", entries.len());
                            for entry in entries {
                                let marker = transcript_marker(entry.kind);
                                println!("    {marker} {}", entry.text);
                            }
                        }
                        if let Some(entries) = committee_tail_entries.as_ref() {
                            println!("  committee (last {}):", entries.len());
                            for event in entries {
                                let ts = event["ts"].as_u64().unwrap_or_default();
                                let kind = event["kind"].as_str().unwrap_or("?");
                                let body = match kind {
                                    "planner_plan_ready" => {
                                        event["plan_text"].as_str().unwrap_or("").to_string()
                                    }
                                    "reviewer_verdict" => format!(
                                        "turn {} -> {} {}",
                                        event["turn_index"].as_u64().unwrap_or_default(),
                                        event["verdict"].as_str().unwrap_or(""),
                                        event["comments"].as_str().unwrap_or(""),
                                    ),
                                    "role_usage" => format!(
                                        "{} +${:.4} / +{} tok",
                                        event["role"].as_str().unwrap_or(""),
                                        event["cost_usd"].as_f64().unwrap_or_default(),
                                        event["tokens"].as_u64().unwrap_or_default(),
                                    ),
                                    _ => event.to_string(),
                                };
                                println!("    [{ts}] {kind}: {body}");
                            }
                        }
                    }
                    (Some(session), None) => {
                        println!("{}\t{}", session.id, session.summary);
                        println!("  (no SessionRecord yet — session never persisted a snapshot)");
                    }
                    (None, _) => println!("session not found: {id}"),
                },
            }
        }
        SessionCommand::Delete { id } => {
            let deleted = delete_persisted_session(&store, project_root, id)?;
            print_json_or_text_result(
                serde_json::json!({"deleted": deleted, "id": id}),
                format!("deleted session {id}: {deleted}"),
                output,
            )?;
        }
        SessionCommand::Rename { id, title } => {
            let title = title.join(" ").trim().to_string();
            if title.is_empty() {
                anyhow::bail!("session title must not be empty");
            }
            let renamed = rename_persisted_session(&store, project_root, id, &title)?;
            print_json_or_text_result(
                serde_json::json!({"renamed": renamed, "id": id, "title": title}),
                format!("renamed session {id}: {title}"),
                output,
            )?;
        }
        SessionCommand::Replay { id, last, step } => {
            replay_session_transcript(project_root, id, *last, *step, output)?;
        }
        SessionCommand::Tail {
            id,
            interval_ms,
            from_now,
        } => {
            tail_session_transcript(project_root, id, *interval_ms, *from_now)?;
        }
        SessionCommand::Search {
            query,
            session,
            limit,
        } => {
            search_session_transcripts(project_root, query, session.as_deref(), *limit, output)?;
        }
        SessionCommand::Prune {
            status,
            older_than_days,
            dry_run,
        } => {
            let result = prune_session_records(
                &store,
                project_root,
                status.as_deref(),
                *older_than_days,
                *dry_run,
            )?;
            print_prune_result(&result, output)?;
        }
        SessionCommand::Export {
            id,
            out,
            artifacts,
            force,
        } => {
            export_session(project_root, id, out, artifacts, *force, output)?;
        }
        SessionCommand::Import { from, id, force } => {
            import_session(&store, project_root, from, id.as_deref(), *force, output)?;
        }
        SessionCommand::Note { id, action } => {
            handle_session_note(project_root, id, action, output)?;
        }
        SessionCommand::Locate { id } => {
            let result = session_locate(project_root, id);
            match output {
                OutputFormat::Json => {
                    println!("{}", serde_json::to_string_pretty(&result)?);
                }
                OutputFormat::Text => {
                    if result.exists {
                        println!("{}", result.path);
                    } else {
                        println!("{} (not present)", result.path);
                    }
                }
            }
        }
        SessionCommand::Count => {
            let records = store.list_session_records().unwrap_or_default();
            let summary = session_count_summary(&records);
            match output {
                OutputFormat::Json => {
                    println!("{}", serde_json::to_string_pretty(&summary)?);
                }
                OutputFormat::Text => {
                    println!("total:     {}", summary.total);
                    println!("idle:      {}", summary.idle);
                    println!("running:   {}", summary.running);
                    println!("suspended: {}", summary.suspended);
                    println!("done:      {}", summary.done);
                    println!("failed:    {}", summary.failed);
                }
            }
        }
    }
    Ok(())
}

#[derive(Clone, Debug, Default, Eq, PartialEq, serde::Serialize)]
pub(crate) struct SessionCountSummary {
    pub total: usize,
    pub idle: usize,
    pub running: usize,
    pub suspended: usize,
    pub done: usize,
    pub failed: usize,
}

pub(crate) fn session_count_summary(
    records: &[peridot_memory::SessionRecord],
) -> SessionCountSummary {
    let mut summary = SessionCountSummary {
        total: records.len(),
        ..Default::default()
    };
    for record in records {
        use peridot_memory::SessionLifecycle;
        match record.status {
            SessionLifecycle::Idle => summary.idle += 1,
            SessionLifecycle::Running => summary.running += 1,
            SessionLifecycle::Suspended => summary.suspended += 1,
            SessionLifecycle::Done => summary.done += 1,
            SessionLifecycle::Failed => summary.failed += 1,
        }
    }
    summary
}

fn delete_persisted_session(store: &MemoryStore, project_root: &Path, id: &str) -> Result<bool> {
    let deleted_summary = store.delete_session(id)?;
    let deleted_record = store.delete_session_record(id)?;
    let sessions_root = project_root.join(".peridot").join("sessions");
    let deleted_blobs = peridot_memory::remove_session_dir(&sessions_root, id)?;
    Ok(deleted_summary || deleted_record || deleted_blobs)
}

fn rename_persisted_session(
    store: &MemoryStore,
    project_root: &Path,
    id: &str,
    title: &str,
) -> Result<bool> {
    let existing_summary = store.get_session(id)?;
    let existing_record = store.get_session_record(id)?;
    let sessions_root = project_root.join(".peridot").join("sessions");
    let existing_blob = peridot_memory::load_session_blob(&sessions_root, id, "tui_state.json")?;
    if existing_summary.is_none() && existing_record.is_none() && existing_blob.is_none() {
        return Ok(false);
    }
    store.save_session(&SessionSummary {
        id: id.to_string(),
        summary: title.to_string(),
    })?;
    if let Some(mut record) = existing_record {
        record.summary = title.to_string();
        record.updated_at_unix = unix_timestamp();
        store.save_session_record(&record)?;
    }
    if let Some(bytes) = existing_blob
        && let Ok(mut state) = serde_json::from_slice::<peridot_tui::TuiState>(&bytes)
    {
        for item in &mut state.sessions {
            if item.id == id {
                item.title = title.to_string();
                item.title_generated = true;
            }
        }
        let serialized = serde_json::to_vec(&state)?;
        peridot_memory::save_session_blob(&sessions_root, id, "tui_state.json", &serialized)?;
    }
    Ok(true)
}

fn record_title(record: &peridot_memory::SessionRecord) -> Option<&str> {
    (!record.summary.trim().is_empty())
        .then_some(record.summary.as_str())
        .or_else(|| {
            record
                .last_task
                .as_deref()
                .filter(|task| !task.trim().is_empty())
        })
}

/// Parses a `--status <value>` CLI flag into a `SessionLifecycle` enum.
/// Lower-cases the input so `done`, `Done`, `DONE` all map to the same
/// variant. Returns an `anyhow::Error` for unknown labels so the user sees
/// the expected vocabulary instead of a silent zero-result list.
fn parse_lifecycle_filter(value: &str) -> Result<peridot_memory::SessionLifecycle> {
    use peridot_memory::SessionLifecycle;
    match value.to_ascii_lowercase().as_str() {
        "idle" => Ok(SessionLifecycle::Idle),
        "running" => Ok(SessionLifecycle::Running),
        "suspended" => Ok(SessionLifecycle::Suspended),
        "done" => Ok(SessionLifecycle::Done),
        "failed" => Ok(SessionLifecycle::Failed),
        other => anyhow::bail!(
            "unknown --status '{other}'; expected one of idle|running|suspended|done|failed",
        ),
    }
}

/// Returns the most recent `n` committee events for a session by reading
/// `<sessions>/<id>/committee.ndjson` once. Missing file returns an empty
/// vector. Parse failures on individual lines are silently skipped.
fn read_committee_tail(project_root: &Path, id: &str, n: usize) -> Vec<serde_json::Value> {
    let path = project_root
        .join(".peridot")
        .join("sessions")
        .join(id)
        .join("committee.ndjson");
    let Ok(raw) = std::fs::read_to_string(&path) else {
        return Vec::new();
    };
    let mut events: Vec<serde_json::Value> = Vec::new();
    for line in raw.lines() {
        if line.trim().is_empty() {
            continue;
        }
        if let Ok(value) = serde_json::from_str::<serde_json::Value>(line) {
            events.push(value);
        }
    }
    let start = events.len().saturating_sub(n);
    events.split_off(start)
}

/// Returns the most recent `n` operator notes for a session as raw JSON
/// objects (kept as Value so `Show --notes-tail N` can pass them through
/// to either the text or JSON output path).
fn read_notes_tail(project_root: &Path, id: &str, n: usize) -> Vec<serde_json::Value> {
    let path = project_root
        .join(".peridot")
        .join("sessions")
        .join(id)
        .join("notes.ndjson");
    let Ok(raw) = std::fs::read_to_string(&path) else {
        return Vec::new();
    };
    let mut notes: Vec<serde_json::Value> = Vec::new();
    for line in raw.lines() {
        if line.trim().is_empty() {
            continue;
        }
        if let Ok(value) = serde_json::from_str::<serde_json::Value>(line) {
            notes.push(value);
        }
    }
    let start = notes.len().saturating_sub(n);
    notes.split_off(start)
}

/// Returns the number of operator-written notes and the latest note's text
/// for a session, by reading `<sessions>/<id>/notes.ndjson` once. Missing
/// file is treated as zero notes.
fn read_notes_summary(project_root: &Path, id: &str) -> (usize, Option<String>) {
    let path = project_root
        .join(".peridot")
        .join("sessions")
        .join(id)
        .join("notes.ndjson");
    let Ok(raw) = std::fs::read_to_string(&path) else {
        return (0, None);
    };
    let mut count = 0usize;
    let mut last: Option<String> = None;
    for line in raw.lines() {
        if line.trim().is_empty() {
            continue;
        }
        if let Ok(value) = serde_json::from_str::<serde_json::Value>(line)
            && let Some(text) = value.get("text").and_then(|v| v.as_str())
        {
            count += 1;
            last = Some(text.to_string());
        }
    }
    (count, last)
}

fn session_notes_path(project_root: &Path, id: &str) -> PathBuf {
    project_root
        .join(".peridot")
        .join("sessions")
        .join(id)
        .join("notes.ndjson")
}

pub(crate) fn append_session_note(
    project_root: &Path,
    id: &str,
    body: &str,
) -> Result<serde_json::Value> {
    let body = body.trim();
    if body.is_empty() {
        anyhow::bail!("note text must not be empty");
    }
    let notes_path = session_notes_path(project_root, id);
    if let Some(parent) = notes_path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or_default();
    let line = serde_json::json!({
        "ts": timestamp,
        "text": body,
    });
    use std::io::Write;
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&notes_path)
        .with_context(|| format!("failed to open {}", notes_path.display()))?;
    writeln!(file, "{}", serde_json::to_string(&line)?)?;
    Ok(line)
}

pub(crate) fn read_session_notes(
    project_root: &Path,
    id: &str,
    last: Option<usize>,
) -> Result<(Vec<serde_json::Value>, usize)> {
    let notes_path = session_notes_path(project_root, id);
    let raw = match std::fs::read_to_string(&notes_path) {
        Ok(v) => v,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(err) => return Err(err).context(format!("failed to read {}", notes_path.display())),
    };
    let mut notes = Vec::new();
    for line in raw.lines() {
        if line.trim().is_empty() {
            continue;
        }
        if let Ok(value) = serde_json::from_str::<serde_json::Value>(line) {
            notes.push(value);
        }
    }
    let total = notes.len();
    if let Some(limit) = last {
        let start = total.saturating_sub(limit);
        notes = notes.split_off(start);
    }
    Ok((notes, total))
}

pub(crate) fn clear_session_notes(project_root: &Path, id: &str) -> Result<bool> {
    let notes_path = session_notes_path(project_root, id);
    match std::fs::remove_file(&notes_path) {
        Ok(()) => Ok(true),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(err) => Err(err.into()),
    }
}

fn handle_session_note(
    project_root: &Path,
    id: &str,
    action: &SessionNoteAction,
    output: OutputFormat,
) -> Result<()> {
    match action {
        SessionNoteAction::Add { text } => {
            let body = text.join(" ").trim().to_string();
            let line = append_session_note(project_root, id, &body)?;
            let timestamp = line["ts"].as_u64().unwrap_or_default();
            print_json_or_text_result(
                serde_json::json!({"added": true, "id": id, "ts": timestamp, "text": body}),
                format!("added note to {id}: {body}"),
                output,
            )?;
        }
        SessionNoteAction::List { last } => {
            let (trimmed, total) = read_session_notes(project_root, id, *last)?;
            match output {
                OutputFormat::Json => {
                    println!(
                        "{}",
                        serde_json::to_string_pretty(&serde_json::json!({
                            "id": id,
                            "notes": trimmed,
                            "total": total,
                        }))?
                    );
                }
                OutputFormat::Text => {
                    if trimmed.is_empty() {
                        println!("no notes for {id}");
                    } else {
                        for note in &trimmed {
                            let ts = note["ts"].as_u64().unwrap_or_default();
                            let text = note["text"].as_str().unwrap_or("");
                            println!("[{ts}] {text}");
                        }
                        if trimmed.len() < total {
                            println!(
                                "... showing {} of {} notes; drop --last for the full list.",
                                trimmed.len(),
                                total,
                            );
                        }
                    }
                }
            }
        }
        SessionNoteAction::Clear => {
            let removed = clear_session_notes(project_root, id)?;
            print_json_or_text_result(
                serde_json::json!({"cleared": removed, "id": id}),
                format!("cleared notes for {id}: {removed}"),
                output,
            )?;
        }
    }
    Ok(())
}

fn import_session(
    store: &MemoryStore,
    project_root: &Path,
    from: &Path,
    id_override: Option<&str>,
    force: bool,
    output: OutputFormat,
) -> Result<()> {
    let result = import_session_artifacts(store, project_root, from, id_override, force)?;
    print_import_result(&result, output)?;
    Ok(())
}

pub(crate) fn import_session_artifacts(
    store: &MemoryStore,
    project_root: &Path,
    from: &Path,
    id_override: Option<&str>,
    force: bool,
) -> Result<SessionImportResult> {
    if !from.is_dir() {
        anyhow::bail!("source {} is not a directory", from.display());
    }
    let derived_id = match id_override {
        Some(id) => id.to_string(),
        None => from
            .file_name()
            .and_then(|name| name.to_str())
            .map(str::to_string)
            .with_context(|| {
                format!(
                    "could not derive session id from {}; pass --id <id>",
                    from.display()
                )
            })?,
    };
    let destination = project_root
        .join(".peridot")
        .join("sessions")
        .join(&derived_id);
    if destination.exists() {
        if !force {
            anyhow::bail!(
                "session {derived_id} already exists at {}; pass --force to overwrite",
                destination.display()
            );
        }
        std::fs::remove_dir_all(&destination)
            .with_context(|| format!("failed to clear {} before import", destination.display()))?;
    }
    std::fs::create_dir_all(&destination)
        .with_context(|| format!("failed to create {}", destination.display()))?;
    let mut copied: Vec<String> = Vec::new();
    for entry in std::fs::read_dir(from)? {
        let entry = entry?;
        let file_name = entry.file_name();
        let src = entry.path();
        let dst = destination.join(&file_name);
        if src.is_dir() {
            copy_dir_recursive(&src, &dst)?;
        } else {
            std::fs::copy(&src, &dst).with_context(|| {
                format!("failed to copy {} -> {}", src.display(), dst.display())
            })?;
        }
        if let Some(name) = file_name.to_str() {
            copied.push(name.to_string());
        }
    }
    if let Some(bytes) = peridot_memory::load_session_blob(
        &project_root.join(".peridot").join("sessions"),
        &derived_id,
        "tui_state.json",
    )? && let Ok(state) = serde_json::from_slice::<peridot_tui::TuiState>(&bytes)
    {
        let summary_text = state
            .last_task
            .clone()
            .unwrap_or_else(|| format!("imported session {derived_id}"));
        let _ = store.save_session(&SessionSummary {
            id: derived_id.clone(),
            summary: summary_text,
        });
    }
    Ok(SessionImportResult {
        id: derived_id,
        source: from.display().to_string(),
        destination: destination.display().to_string(),
        files: copied,
    })
}

fn print_import_result(result: &SessionImportResult, output: OutputFormat) -> Result<()> {
    match output {
        OutputFormat::Json => {
            println!("{}", serde_json::to_string_pretty(result)?);
        }
        OutputFormat::Text => {
            println!(
                "imported session {} from {} into {} ({} entries)",
                result.id,
                result.source,
                result.destination,
                result.files.len()
            );
            for name in &result.files {
                println!("  - {name}");
            }
        }
    }
    Ok(())
}

#[cfg(test)]
// Tests live mid-file because the helpers and command types they
// exercise (rename + delete) sit in the first half of the module; the
// remaining helpers (export, prune, search, replay) tested elsewhere
// trail below. Moving the block to the end would put 500+ lines
// between the helpers and their tests for no gain.
#[allow(clippy::items_after_test_module)]
mod tests {
    use super::*;

    fn temp_root(name: &str) -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or_default();
        std::env::temp_dir().join(format!(
            "peridot-session-{name}-{}-{nanos}",
            std::process::id()
        ))
    }

    #[test]
    fn rewind_context_entries_removes_last_turn_and_restores_prompt() {
        let mut first = peridot_context::ContextEntry::trusted(
            peridot_context::ContextSource::User,
            "first prompt",
        );
        first.turn_id = 1;
        let mut first_reply = peridot_context::ContextEntry::trusted(
            peridot_context::ContextSource::Assistant,
            "first reply",
        );
        first_reply.turn_id = 1;
        let mut second = peridot_context::ContextEntry::trusted(
            peridot_context::ContextSource::User,
            "second prompt",
        );
        second.turn_id = 2;
        let mut second_reply = peridot_context::ContextEntry::trusted(
            peridot_context::ContextSource::Assistant,
            "second reply",
        );
        second_reply.turn_id = 2;

        let (kept, rewind) =
            rewind_context_entries(vec![first, first_reply, second, second_reply]).unwrap();

        assert_eq!(rewind.restored_prompt, "second prompt");
        assert_eq!(rewind.rewind_turn_id, Some(2));
        assert_eq!(rewind.removed_count, 2);
        assert_eq!(rewind.kept_count, 2);
        assert_eq!(kept.len(), 2);
        assert_eq!(kept[0].content, "first prompt");
        assert_eq!(kept[1].content, "first reply");
    }

    #[test]
    fn rewind_context_entries_handles_legacy_zero_turn_snapshots() {
        let entries = vec![
            peridot_context::ContextEntry::trusted(
                peridot_context::ContextSource::User,
                "old prompt",
            ),
            peridot_context::ContextEntry::trusted(
                peridot_context::ContextSource::Assistant,
                "old reply",
            ),
            peridot_context::ContextEntry::trusted(
                peridot_context::ContextSource::User,
                "legacy prompt",
            ),
            peridot_context::ContextEntry::trusted(
                peridot_context::ContextSource::Assistant,
                "legacy reply",
            ),
        ];

        let (kept, rewind) = rewind_context_entries(entries).unwrap();

        assert_eq!(rewind.restored_prompt, "legacy prompt");
        assert_eq!(rewind.rewind_turn_id, None);
        assert_eq!(rewind.removed_count, 2);
        assert_eq!(kept.len(), 2);
    }

    #[test]
    fn session_rename_updates_summary_record_and_tui_blob() {
        let root = temp_root("rename");
        let store = memory_store(&root);
        store
            .save_session(&SessionSummary {
                id: "s1".to_string(),
                summary: "old".to_string(),
            })
            .unwrap();
        store
            .save_session_record(&peridot_memory::SessionRecord::new("s1", &root))
            .unwrap();
        let mut state = peridot_tui::TuiState::new(peridot_tui::HeaderState::new(
            peridot_common::ExecutionMode::Execute,
            peridot_common::PermissionMode::Auto,
            "mock",
        ));
        state
            .sessions
            .push(peridot_tui::SessionDirectoryItem::new("s1", "old"));
        let sessions_root = root.join(".peridot").join("sessions");
        peridot_memory::save_session_blob(
            &sessions_root,
            "s1",
            "tui_state.json",
            &serde_json::to_vec(&state).unwrap(),
        )
        .unwrap();

        run_session_command(
            &SessionCommand::Rename {
                id: "s1".to_string(),
                title: vec!["new".to_string(), "title".to_string()],
            },
            &root,
            OutputFormat::Json,
        )
        .unwrap();

        assert_eq!(
            store.get_session("s1").unwrap().unwrap().summary,
            "new title"
        );
        assert_eq!(
            store.get_session_record("s1").unwrap().unwrap().summary,
            "new title"
        );
        let bytes = peridot_memory::load_session_blob(&sessions_root, "s1", "tui_state.json")
            .unwrap()
            .unwrap();
        let state: peridot_tui::TuiState = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(state.sessions[0].title, "new title");
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn session_delete_removes_summary_record_and_blobs() {
        let root = temp_root("delete");
        let store = memory_store(&root);
        store
            .save_session(&SessionSummary {
                id: "s1".to_string(),
                summary: "old".to_string(),
            })
            .unwrap();
        store
            .save_session_record(&peridot_memory::SessionRecord::new("s1", &root))
            .unwrap();
        let sessions_root = root.join(".peridot").join("sessions");
        peridot_memory::save_session_blob(&sessions_root, "s1", "tui_state.json", b"{}").unwrap();

        run_session_command(
            &SessionCommand::Delete {
                id: "s1".to_string(),
            },
            &root,
            OutputFormat::Json,
        )
        .unwrap();

        assert!(store.get_session("s1").unwrap().is_none());
        assert!(store.get_session_record("s1").unwrap().is_none());
        assert!(!sessions_root.join("s1").exists());
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn session_prune_records_supports_dry_run_and_removal() {
        let root = temp_root("prune");
        let store = memory_store(&root);
        let sessions_root = root.join(".peridot").join("sessions");
        for (id, status) in [
            ("done-one", peridot_memory::SessionLifecycle::Done),
            ("failed-one", peridot_memory::SessionLifecycle::Failed),
        ] {
            store
                .save_session(&SessionSummary {
                    id: id.to_string(),
                    summary: id.to_string(),
                })
                .unwrap();
            let mut record = peridot_memory::SessionRecord::new(id, &root);
            record.status = status;
            store.save_session_record(&record).unwrap();
            peridot_memory::save_session_blob(&sessions_root, id, "tui_state.json", b"{}").unwrap();
        }

        let preview =
            prune_session_records(&store, &root, Some("done"), None, true).expect("dry-run prune");
        assert_eq!(preview.considered, vec!["done-one"]);
        assert!(preview.removed.is_empty());
        assert!(store.get_session_record("done-one").unwrap().is_some());
        assert!(sessions_root.join("done-one").exists());

        let removed =
            prune_session_records(&store, &root, Some("done"), None, false).expect("real prune");
        assert_eq!(removed.removed, vec!["done-one"]);
        assert!(store.get_session("done-one").unwrap().is_none());
        assert!(store.get_session_record("done-one").unwrap().is_none());
        assert!(!sessions_root.join("done-one").exists());
        assert!(store.get_session_record("failed-one").unwrap().is_some());
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn session_search_returns_transcript_hits_with_limit() {
        let root = temp_root("search");
        let sessions_root = root.join(".peridot").join("sessions");
        let mut first = peridot_tui::TuiState::new(peridot_tui::HeaderState::new(
            peridot_common::ExecutionMode::Execute,
            peridot_common::PermissionMode::Auto,
            "mock",
        ));
        first.push_transcript_entry(peridot_tui::TranscriptKind::User, "parser failure");
        first.push_transcript_entry(peridot_tui::TranscriptKind::Assistant, "fixed parser");
        peridot_memory::save_session_blob(
            &sessions_root,
            "session-a",
            "tui_state.json",
            &serde_json::to_vec(&first).unwrap(),
        )
        .unwrap();
        let mut second = peridot_tui::TuiState::new(peridot_tui::HeaderState::new(
            peridot_common::ExecutionMode::Execute,
            peridot_common::PermissionMode::Auto,
            "mock",
        ));
        second.push_transcript_entry(peridot_tui::TranscriptKind::User, "unrelated");
        peridot_memory::save_session_blob(
            &sessions_root,
            "session-b",
            "tui_state.json",
            &serde_json::to_vec(&second).unwrap(),
        )
        .unwrap();

        let result = search_session_transcript_hits(&root, "parser", None, Some(1)).unwrap();

        assert_eq!(result.query, "parser");
        assert_eq!(result.total, 1);
        assert!(result.truncated);
        assert_eq!(
            result.hits,
            vec![SessionSearchHit {
                session: "session-a".to_string(),
                index: 0,
                kind: "user".to_string(),
                text: "parser failure".to_string(),
            }]
        );
        fs::remove_dir_all(&root).ok();
    }

    fn transcript_entry(
        kind: peridot_tui::TranscriptKind,
        text: &str,
        ts: u64,
    ) -> peridot_tui::TranscriptEntry {
        peridot_tui::TranscriptEntry {
            kind,
            text: text.to_string(),
            ts,
            parent_turn: None,
        }
    }

    fn committee_event(order: usize, raw: serde_json::Value) -> CommitteeReplayEvent {
        CommitteeReplayEvent {
            ts: raw
                .get("ts")
                .and_then(|value| value.as_u64())
                .unwrap_or_default(),
            kind: raw
                .get("kind")
                .and_then(|value| value.as_str())
                .unwrap()
                .to_string(),
            order,
            raw,
        }
    }

    #[test]
    fn replay_timeline_orders_timestamped_transcript_and_committee_events() {
        let transcript = vec![
            transcript_entry(peridot_tui::TranscriptKind::User, "task", 10),
            transcript_entry(peridot_tui::TranscriptKind::Assistant, "working", 12),
        ];
        let committee = vec![committee_event(
            0,
            serde_json::json!({
                "ts": 11,
                "kind": "role_usage",
                "role": "planner",
                "cost_usd": 0.01,
                "tokens": 25
            }),
        )];

        let timeline = build_replay_timeline(&transcript, &committee);

        assert_eq!(
            timeline
                .iter()
                .map(|entry| entry.text())
                .collect::<Vec<_>>(),
            vec![
                "task".to_string(),
                "committee planner usage: +$0.0100 / +25 tok".to_string(),
                "working".to_string(),
            ]
        );
    }

    #[test]
    fn replay_timeline_replaces_legacy_duplicate_committee_rows() {
        let transcript = vec![
            transcript_entry(peridot_tui::TranscriptKind::User, "task", 0),
            transcript_entry(
                peridot_tui::TranscriptKind::System,
                "committee planner ready:\n1. edit",
                0,
            ),
            transcript_entry(
                peridot_tui::TranscriptKind::Notice,
                "committee reviewer (turn 1): request_changes — indent off",
                0,
            ),
        ];
        let committee = vec![
            committee_event(
                0,
                serde_json::json!({
                    "ts": 20,
                    "kind": "role_usage",
                    "role": "planner",
                    "cost_usd": 0.02,
                    "tokens": 30
                }),
            ),
            committee_event(
                1,
                serde_json::json!({
                    "ts": 21,
                    "kind": "planner_plan_ready",
                    "plan_text": "1. edit"
                }),
            ),
            committee_event(
                2,
                serde_json::json!({
                    "ts": 22,
                    "kind": "reviewer_verdict",
                    "turn_index": 1,
                    "verdict": "request_changes",
                    "comments": "indent off"
                }),
            ),
        ];

        let timeline = build_replay_timeline(&transcript, &committee);
        let texts: Vec<String> = timeline.iter().map(|entry| entry.text()).collect();

        assert_eq!(texts.len(), 4);
        assert_eq!(texts[0], "task");
        assert_eq!(texts[1], "committee planner usage: +$0.0200 / +30 tok");
        assert_eq!(texts[2], "committee planner ready:\n1. edit");
        assert_eq!(
            texts[3],
            "committee reviewer (turn 1): request_changes — indent off"
        );
    }

    #[test]
    fn replay_timeline_last_can_limit_unified_entries() {
        let transcript = vec![
            transcript_entry(peridot_tui::TranscriptKind::User, "one", 1),
            transcript_entry(peridot_tui::TranscriptKind::Assistant, "three", 3),
        ];
        let committee = vec![committee_event(
            0,
            serde_json::json!({
                "ts": 2,
                "kind": "role_usage",
                "role": "reviewer",
                "cost_usd": 0.03,
                "tokens": 40
            }),
        )];

        let timeline = build_replay_timeline(&transcript, &committee);
        let last_two = &timeline[timeline.len().saturating_sub(2)..];

        assert_eq!(
            last_two
                .iter()
                .map(|entry| entry.text())
                .collect::<Vec<_>>(),
            vec![
                "committee reviewer usage: +$0.0300 / +40 tok".to_string(),
                "three".to_string()
            ]
        );
    }

    #[test]
    fn session_export_selected_artifacts_writes_portable_files() {
        let root = temp_root("export-selected");
        let session_dir = root.join(".peridot").join("sessions").join("s1");
        fs::create_dir_all(&session_dir).unwrap();
        let context = vec![peridot_context::ContextEntry::trusted(
            peridot_context::ContextSource::PlanReminder,
            "[attachment]\npath: src/lib.rs\nbytes: 12\n\n```text\nhello world\n```",
        )];
        fs::write(
            session_dir.join("context.bin"),
            serde_json::to_vec(&context).unwrap(),
        )
        .unwrap();
        fs::write(
            session_dir.join("notes.ndjson"),
            "{\"ts\":1,\"text\":\"remember export\"}\n",
        )
        .unwrap();
        fs::write(
            session_dir.join("transcript.ndjson"),
            format!(
                "{}\n",
                serde_json::to_string(&transcript_entry(
                    peridot_tui::TranscriptKind::User,
                    "task",
                    1
                ))
                .unwrap()
            ),
        )
        .unwrap();

        let out = root.join("export");
        export_session(
            &root,
            "s1",
            &out,
            &[
                SessionExportArtifact::Attachments,
                SessionExportArtifact::Notes,
                SessionExportArtifact::Timeline,
            ],
            false,
            OutputFormat::Text,
        )
        .unwrap();

        assert!(!out.join("context.bin").exists());
        let attachments: Vec<serde_json::Value> =
            serde_json::from_slice(&fs::read(out.join("attachments.json")).unwrap()).unwrap();
        assert_eq!(attachments[0]["path"], "src/lib.rs");
        assert_eq!(attachments[0]["content"], "hello world");
        assert_eq!(count_non_empty_lines(&out.join("notes.ndjson")).unwrap(), 1);
        let timeline: Vec<serde_json::Value> =
            serde_json::from_slice(&fs::read(out.join("timeline.json")).unwrap()).unwrap();
        assert_eq!(timeline.len(), 1);
        assert_eq!(timeline[0]["source"], "transcript");
        let manifest: serde_json::Value =
            serde_json::from_slice(&fs::read(out.join("export-manifest.json")).unwrap()).unwrap();
        assert_eq!(
            manifest["artifact_classes"],
            serde_json::json!(["attachments", "notes", "timeline"])
        );
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn session_export_without_artifacts_preserves_full_copy_default() {
        let root = temp_root("export-full-default");
        let session_dir = root.join(".peridot").join("sessions").join("s1");
        fs::create_dir_all(&session_dir).unwrap();
        fs::write(session_dir.join("context.bin"), "[]").unwrap();

        let out = root.join("export");
        export_session(&root, "s1", &out, &[], false, OutputFormat::Text).unwrap();

        assert!(out.join("context.bin").exists());
        assert!(!out.join("export-manifest.json").exists());
        fs::remove_dir_all(&root).ok();
    }
}

fn export_session(
    project_root: &Path,
    id: &str,
    out_dir: &Path,
    artifacts: &[SessionExportArtifact],
    force: bool,
    output: OutputFormat,
) -> Result<()> {
    let report = export_session_artifacts(project_root, id, out_dir, artifacts, force)?;
    match output {
        OutputFormat::Json => {
            println!("{}", serde_json::to_string_pretty(&report)?);
        }
        OutputFormat::Text => {
            println!(
                "exported session {id} from {} to {} ({} copied entries, {} generated artifacts)",
                report.source,
                report.destination,
                report.files.len(),
                report.artifacts.len()
            );
            for name in &report.files {
                println!("  - {name}");
            }
            for artifact in &report.artifacts {
                println!(
                    "  - {} ({}, {} entries)",
                    artifact.path, artifact.class, artifact.count
                );
            }
        }
    }
    Ok(())
}

#[derive(Clone, Debug, serde::Serialize)]
pub(crate) struct SessionExportReport {
    pub(crate) id: String,
    pub(crate) source: String,
    pub(crate) destination: String,
    pub(crate) artifact_classes: Vec<String>,
    pub(crate) files: Vec<String>,
    pub(crate) artifacts: Vec<ExportedArtifact>,
}

#[derive(Clone, Debug, serde::Serialize)]
pub(crate) struct ExportedArtifact {
    pub(crate) class: &'static str,
    pub(crate) path: String,
    pub(crate) count: usize,
}

pub(crate) fn export_session_artifacts(
    project_root: &Path,
    id: &str,
    out_dir: &Path,
    artifacts: &[SessionExportArtifact],
    force: bool,
) -> Result<SessionExportReport> {
    let source = project_root.join(".peridot").join("sessions").join(id);
    if !source.is_dir() {
        anyhow::bail!(
            "session {id} has no on-disk directory at {}",
            source.display()
        );
    }
    if out_dir.exists() {
        if !force {
            anyhow::bail!(
                "{} already exists; pass --force to overwrite",
                out_dir.display()
            );
        }
        std::fs::remove_dir_all(out_dir)
            .with_context(|| format!("failed to clear {} before export", out_dir.display()))?;
    }
    std::fs::create_dir_all(out_dir)
        .with_context(|| format!("failed to create export directory {}", out_dir.display()))?;
    let selected = effective_export_artifacts(artifacts);
    let mut copied: Vec<String> = Vec::new();
    if selected.contains(&SessionExportArtifact::Full) {
        for entry in std::fs::read_dir(&source)? {
            let entry = entry?;
            let file_name = entry.file_name();
            let from = entry.path();
            let to = out_dir.join(&file_name);
            if from.is_dir() {
                copy_dir_recursive(&from, &to)?;
            } else {
                std::fs::copy(&from, &to).with_context(|| {
                    format!("failed to copy {} -> {}", from.display(), to.display())
                })?;
            }
            if let Some(name) = file_name.to_str() {
                copied.push(name.to_string());
            }
        }
        copied.sort();
    }
    let mut generated: Vec<ExportedArtifact> = Vec::new();
    for artifact in &selected {
        match artifact {
            SessionExportArtifact::Full => {}
            SessionExportArtifact::Attachments => {
                generated.push(write_attachments_export(project_root, id, out_dir)?);
            }
            SessionExportArtifact::Notes => {
                generated.push(write_notes_export(&source, out_dir)?);
            }
            SessionExportArtifact::Timeline => {
                generated.push(write_timeline_export(project_root, id, out_dir)?);
            }
        }
    }
    if !generated.is_empty() {
        write_export_manifest(id, &source, out_dir, &selected, &copied, &generated)?;
    }
    Ok(SessionExportReport {
        id: id.to_string(),
        source: source.display().to_string(),
        destination: out_dir.display().to_string(),
        artifact_classes: selected
            .iter()
            .map(|artifact| artifact.as_str().to_string())
            .collect(),
        files: copied,
        artifacts: generated,
    })
}

fn effective_export_artifacts(artifacts: &[SessionExportArtifact]) -> Vec<SessionExportArtifact> {
    let artifacts = if artifacts.is_empty() {
        &[SessionExportArtifact::Full][..]
    } else {
        artifacts
    };
    let mut selected = Vec::new();
    for artifact in artifacts {
        if !selected.contains(artifact) {
            selected.push(*artifact);
        }
    }
    selected
}

fn write_attachments_export(
    project_root: &Path,
    id: &str,
    out_dir: &Path,
) -> Result<ExportedArtifact> {
    let entries = read_context_snapshot_for_export(project_root, id)?;
    let attachments = attachments_from_context(&entries);
    let path = out_dir.join("attachments.json");
    write_pretty_json(&path, &attachments)?;
    Ok(ExportedArtifact {
        class: SessionExportArtifact::Attachments.as_str(),
        path: "attachments.json".to_string(),
        count: attachments.len(),
    })
}

fn write_notes_export(source: &Path, out_dir: &Path) -> Result<ExportedArtifact> {
    let source_path = source.join("notes.ndjson");
    let destination_path = out_dir.join("notes.ndjson");
    let count = if source_path.exists() {
        std::fs::copy(&source_path, &destination_path).with_context(|| {
            format!(
                "failed to copy {} -> {}",
                source_path.display(),
                destination_path.display()
            )
        })?;
        count_non_empty_lines(&destination_path)?
    } else {
        std::fs::write(&destination_path, "")
            .with_context(|| format!("failed to write {}", destination_path.display()))?;
        0
    };
    Ok(ExportedArtifact {
        class: SessionExportArtifact::Notes.as_str(),
        path: "notes.ndjson".to_string(),
        count,
    })
}

fn write_timeline_export(
    project_root: &Path,
    id: &str,
    out_dir: &Path,
) -> Result<ExportedArtifact> {
    let entries = load_session_transcript(project_root, id)?;
    let committee = load_committee_replay_events(project_root, id);
    let timeline = build_replay_timeline(&entries, &committee);
    let values: Vec<_> = timeline.iter().map(ReplayTimelineEntry::to_json).collect();
    let path = out_dir.join("timeline.json");
    write_pretty_json(&path, &values)?;
    Ok(ExportedArtifact {
        class: SessionExportArtifact::Timeline.as_str(),
        path: "timeline.json".to_string(),
        count: values.len(),
    })
}

fn write_export_manifest(
    id: &str,
    source: &Path,
    out_dir: &Path,
    selected: &[SessionExportArtifact],
    copied: &[String],
    generated: &[ExportedArtifact],
) -> Result<()> {
    let manifest = serde_json::json!({
        "id": id,
        "source": source.display().to_string(),
        "destination": out_dir.display().to_string(),
        "artifact_classes": selected.iter().map(|artifact| artifact.as_str()).collect::<Vec<_>>(),
        "files": copied,
        "artifacts": generated,
    });
    write_pretty_json(&out_dir.join("export-manifest.json"), &manifest)
}

fn read_context_snapshot_for_export(
    project_root: &Path,
    id: &str,
) -> Result<Vec<peridot_context::ContextEntry>> {
    let path = project_root
        .join(".peridot")
        .join("sessions")
        .join(id)
        .join("context.bin");
    if !path.exists() {
        return Ok(Vec::new());
    }
    let bytes =
        std::fs::read(&path).with_context(|| format!("failed to read {}", path.display()))?;
    serde_json::from_slice(&bytes).with_context(|| format!("failed to parse {}", path.display()))
}

fn count_non_empty_lines(path: &Path) -> Result<usize> {
    let raw = std::fs::read_to_string(path)
        .with_context(|| format!("failed to read {}", path.display()))?;
    Ok(raw.lines().filter(|line| !line.trim().is_empty()).count())
}

fn write_pretty_json(path: &Path, value: &impl serde::Serialize) -> Result<()> {
    let raw = serde_json::to_vec_pretty(value)?;
    std::fs::write(path, raw).with_context(|| format!("failed to write {}", path.display()))
}

fn copy_dir_recursive(from: &Path, to: &Path) -> Result<()> {
    std::fs::create_dir_all(to).with_context(|| format!("failed to create {}", to.display()))?;
    for entry in std::fs::read_dir(from)? {
        let entry = entry?;
        let src = entry.path();
        let dst = to.join(entry.file_name());
        if src.is_dir() {
            copy_dir_recursive(&src, &dst)?;
        } else {
            std::fs::copy(&src, &dst).with_context(|| {
                format!("failed to copy {} -> {}", src.display(), dst.display())
            })?;
        }
    }
    Ok(())
}

pub(crate) fn prune_session_records(
    store: &MemoryStore,
    project_root: &Path,
    status_filter: Option<&str>,
    older_than_days: Option<u64>,
    dry_run: bool,
) -> Result<SessionPruneResult> {
    use peridot_memory::SessionLifecycle;
    let want_status: Option<SessionLifecycle> = match status_filter {
        Some(value) => match value.to_ascii_lowercase().as_str() {
            "idle" => Some(SessionLifecycle::Idle),
            "running" => Some(SessionLifecycle::Running),
            "suspended" => Some(SessionLifecycle::Suspended),
            "done" => Some(SessionLifecycle::Done),
            "failed" => Some(SessionLifecycle::Failed),
            other => anyhow::bail!(
                "unknown --status '{other}'; expected one of idle|running|suspended|done|failed",
            ),
        },
        None => None,
    };
    let status_filter = status_filter.map(|status| status.to_ascii_lowercase());
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or_default();
    let threshold = older_than_days.map(|days| now.saturating_sub(days.saturating_mul(86_400)));
    let records = store.list_session_records().unwrap_or_default();
    let mut removed: Vec<String> = Vec::new();
    let mut considered: Vec<String> = Vec::new();
    for record in records {
        if let Some(target) = want_status
            && record.status != target
        {
            continue;
        }
        if let Some(limit) = threshold
            && record.updated_at_unix > limit
        {
            continue;
        }
        considered.push(record.id.clone());
        if dry_run {
            continue;
        }
        let sessions_root = project_root.join(".peridot").join("sessions");
        let _ = peridot_memory::remove_session_dir(&sessions_root, &record.id);
        let _ = store.delete_session(&record.id);
        if store.delete_session_record(&record.id).is_ok() {
            removed.push(record.id);
        }
    }
    Ok(SessionPruneResult {
        dry_run,
        considered,
        removed,
        status_filter,
        older_than_days,
    })
}

fn print_prune_result(result: &SessionPruneResult, output: OutputFormat) -> Result<()> {
    match output {
        OutputFormat::Json => {
            println!("{}", serde_json::to_string_pretty(result)?);
        }
        OutputFormat::Text => {
            if result.dry_run {
                if result.considered.is_empty() {
                    println!("prune (dry-run): no matching sessions");
                } else {
                    println!(
                        "prune (dry-run): would remove {} session(s):",
                        result.considered.len()
                    );
                    for id in &result.considered {
                        println!("  - {id}");
                    }
                }
            } else if result.removed.is_empty() {
                println!("prune: no matching sessions");
            } else {
                println!("prune: removed {} session(s):", result.removed.len());
                for id in &result.removed {
                    println!("  - {id}");
                }
            }
        }
    }
    Ok(())
}

fn search_session_transcripts(
    project_root: &Path,
    query: &str,
    only_session: Option<&str>,
    limit: Option<usize>,
    output: OutputFormat,
) -> Result<()> {
    let result = search_session_transcript_hits(project_root, query, only_session, limit)?;
    match output {
        OutputFormat::Json => {
            println!("{}", serde_json::to_string_pretty(&result)?);
        }
        OutputFormat::Text => {
            if result.hits.is_empty() {
                println!("no matches for '{query}'");
            } else {
                for hit in &result.hits {
                    println!("{}[{}] {} {}", hit.session, hit.index, hit.kind, hit.text);
                }
                if result.truncated {
                    println!("... truncated at {} match(es)", result.hits.len());
                }
            }
        }
    }
    Ok(())
}

pub(crate) fn search_session_transcript_hits(
    project_root: &Path,
    query: &str,
    only_session: Option<&str>,
    limit: Option<usize>,
) -> Result<SessionSearchResult> {
    if query.is_empty() {
        anyhow::bail!("search query must not be empty");
    }
    let needle = query.to_ascii_lowercase();
    let sessions_root = project_root.join(".peridot").join("sessions");
    let mut session_ids: Vec<String> = match only_session {
        Some(id) => vec![id.to_string()],
        None => match std::fs::read_dir(&sessions_root) {
            Ok(entries) => entries
                .filter_map(|entry| entry.ok())
                .filter(|entry| entry.path().is_dir())
                .filter_map(|entry| entry.file_name().into_string().ok())
                .collect(),
            Err(_) => Vec::new(),
        },
    };
    session_ids.sort();
    let cap = limit.unwrap_or(usize::MAX);
    let mut hits = Vec::new();
    let mut truncated = false;
    'outer: for id in session_ids {
        let Ok(entries) = load_session_transcript(project_root, &id) else {
            continue;
        };
        for (index, entry) in entries.iter().enumerate() {
            if entry.text.to_ascii_lowercase().contains(&needle) {
                if hits.len() >= cap {
                    truncated = true;
                    break 'outer;
                }
                hits.push(SessionSearchHit {
                    session: id.clone(),
                    index,
                    kind: format!("{:?}", entry.kind).to_ascii_lowercase(),
                    text: entry.text.clone(),
                });
            }
        }
    }
    Ok(SessionSearchResult {
        query: query.to_string(),
        total: hits.len(),
        hits,
        truncated,
    })
}

pub(crate) fn session_show_summary(project_root: &Path, id: &str) -> Result<SessionShowResult> {
    let store = memory_store(project_root);
    let session = store.get_session(id)?;
    let record = store.get_session_record(id)?;
    if session.is_none() && record.is_none() {
        anyhow::bail!("session not found: {id}");
    }
    let (notes_count, last_note) = read_notes_summary(project_root, id);
    Ok(SessionShowResult {
        id: id.to_string(),
        session,
        record,
        notes_count,
        last_note,
    })
}

pub(crate) fn session_locate(project_root: &Path, id: &str) -> SessionLocateResult {
    let path = project_root.join(".peridot").join("sessions").join(id);
    SessionLocateResult {
        id: id.to_string(),
        path: path.display().to_string(),
        exists: path.is_dir(),
    }
}

fn tail_session_transcript(
    project_root: &Path,
    id: &str,
    interval_ms: u64,
    from_now: bool,
) -> Result<()> {
    use std::io::{BufRead, BufReader, Seek, SeekFrom};
    use std::thread::sleep;
    use std::time::Duration;
    let path = project_root
        .join(".peridot")
        .join("sessions")
        .join(id)
        .join("transcript.ndjson");
    let interval = Duration::from_millis(interval_ms.max(50));
    let mut offset: u64 = 0;
    if from_now && let Ok(meta) = std::fs::metadata(&path) {
        offset = meta.len();
        println!("tail: starting from end of {}", path.display());
    } else if path.exists() {
        let file = std::fs::File::open(&path)?;
        let reader = BufReader::new(file);
        for line in reader.lines() {
            let line = line?;
            if line.trim().is_empty() {
                continue;
            }
            print_transcript_ndjson_line(&line);
        }
        if let Ok(meta) = std::fs::metadata(&path) {
            offset = meta.len();
        }
    } else {
        println!("tail: {} does not exist yet; waiting...", path.display());
    }
    loop {
        sleep(interval);
        let Ok(meta) = std::fs::metadata(&path) else {
            continue;
        };
        let len = meta.len();
        if len <= offset {
            if len < offset {
                println!("tail: file truncated; restarting offset");
                offset = 0;
            }
            continue;
        }
        let Ok(mut file) = std::fs::File::open(&path) else {
            continue;
        };
        if file.seek(SeekFrom::Start(offset)).is_err() {
            continue;
        }
        let mut reader = BufReader::new(file);
        let mut buf = String::new();
        loop {
            buf.clear();
            match reader.read_line(&mut buf) {
                Ok(0) => break,
                Ok(_) => {
                    let line = buf.trim_end_matches(['\r', '\n']);
                    if line.is_empty() {
                        continue;
                    }
                    print_transcript_ndjson_line(line);
                }
                Err(_) => break,
            }
        }
        offset = len;
    }
}

fn print_transcript_ndjson_line(line: &str) {
    match serde_json::from_str::<peridot_tui::TranscriptEntry>(line) {
        Ok(entry) => {
            let marker = transcript_marker(entry.kind);
            println!("{marker} {}", entry.text);
        }
        Err(_) => {
            println!("? <invalid ndjson line>");
        }
    }
}

#[derive(Clone, Debug)]
struct CommitteeReplayEvent {
    ts: u64,
    kind: String,
    order: usize,
    raw: serde_json::Value,
}

#[derive(Clone, Debug)]
enum ReplayTimelineEntry {
    Transcript {
        entry: peridot_tui::TranscriptEntry,
        order: usize,
    },
    Committee {
        event: CommitteeReplayEvent,
    },
}

impl ReplayTimelineEntry {
    fn ts(&self) -> u64 {
        match self {
            ReplayTimelineEntry::Transcript { entry, .. } => entry.ts,
            ReplayTimelineEntry::Committee { event } => event.ts,
        }
    }

    fn order(&self) -> usize {
        match self {
            ReplayTimelineEntry::Transcript { order, .. } => *order,
            ReplayTimelineEntry::Committee { event } => 1_000_000usize.saturating_add(event.order),
        }
    }

    fn marker(&self) -> &'static str {
        match self {
            ReplayTimelineEntry::Transcript { entry, .. } => transcript_marker(entry.kind),
            ReplayTimelineEntry::Committee { event } => committee_marker(event),
        }
    }

    fn text(&self) -> String {
        match self {
            ReplayTimelineEntry::Transcript { entry, .. } => entry.text.clone(),
            ReplayTimelineEntry::Committee { event } => committee_text(event),
        }
    }

    fn to_json(&self) -> serde_json::Value {
        match self {
            ReplayTimelineEntry::Transcript { entry, .. } => serde_json::json!({
                "source": "transcript",
                "kind": format!("{:?}", entry.kind).to_ascii_lowercase(),
                "text": entry.text.clone(),
                "ts": entry.ts,
            }),
            ReplayTimelineEntry::Committee { event } => serde_json::json!({
                "source": "committee",
                "kind": event.kind.clone(),
                "text": committee_text(event),
                "ts": event.ts,
                "event": event.raw.clone(),
            }),
        }
    }
}

pub(crate) fn session_replay_summary(
    project_root: &Path,
    id: &str,
    last: Option<usize>,
) -> Result<SessionReplayResult> {
    let entries_owned = load_session_transcript(project_root, id)?;
    let committee_events = load_committee_replay_events(project_root, id);
    let timeline_owned = build_replay_timeline(&entries_owned, &committee_events);
    let total_timeline_entries = timeline_owned.len();
    let start = last
        .map(|limit| total_timeline_entries.saturating_sub(limit))
        .unwrap_or(0);
    let entries = entries_owned
        .iter()
        .map(|entry| SessionReplayTranscriptEntry {
            kind: format!("{:?}", entry.kind).to_ascii_lowercase(),
            text: entry.text.clone(),
            ts: entry.ts,
        })
        .collect();
    let timeline = timeline_owned[start..]
        .iter()
        .map(|entry| match entry {
            ReplayTimelineEntry::Transcript { entry, .. } => SessionReplayTimelineEntry {
                source: "transcript".to_string(),
                kind: format!("{:?}", entry.kind).to_ascii_lowercase(),
                marker: transcript_marker(entry.kind).to_string(),
                text: entry.text.clone(),
                ts: entry.ts,
                event: None,
            },
            ReplayTimelineEntry::Committee { event } => SessionReplayTimelineEntry {
                source: "committee".to_string(),
                kind: event.kind.clone(),
                marker: committee_marker(event).to_string(),
                text: committee_text(event),
                ts: event.ts,
                event: Some(event.raw.clone()),
            },
        })
        .collect();
    Ok(SessionReplayResult {
        id: id.to_string(),
        total: entries_owned.len(),
        timeline_total: total_timeline_entries,
        committee_total: committee_events.len(),
        truncated: start > 0,
        entries,
        timeline,
    })
}

fn replay_session_transcript(
    project_root: &Path,
    id: &str,
    last: Option<usize>,
    step: bool,
    output: OutputFormat,
) -> Result<()> {
    let entries_owned = load_session_transcript(project_root, id)?;
    let committee_events = load_committee_replay_events(project_root, id);
    let timeline_owned = build_replay_timeline(&entries_owned, &committee_events);
    let timeline: Vec<&ReplayTimelineEntry> = if let Some(limit) = last {
        let total = timeline_owned.len();
        let start = total.saturating_sub(limit);
        timeline_owned[start..].iter().collect()
    } else {
        timeline_owned.iter().collect()
    };
    let total_entries = entries_owned.len();
    let total_timeline_entries = timeline_owned.len();
    match output {
        OutputFormat::Json => {
            let payload: Vec<_> = entries_owned
                .iter()
                .map(|entry| {
                    serde_json::json!({
                        "kind": format!("{:?}", entry.kind).to_ascii_lowercase(),
                        "text": entry.text,
                    })
                })
                .collect();
            let timeline_payload: Vec<_> = timeline.iter().map(|entry| entry.to_json()).collect();
            println!(
                "{}",
                serde_json::to_string_pretty(&serde_json::json!({
                    "id": id,
                    "entries": payload,
                    "timeline": timeline_payload,
                    "total": total_entries,
                    "timeline_total": total_timeline_entries,
                    "committee_total": committee_events.len(),
                    "step_mode": step,
                }))?
            );
        }
        OutputFormat::Text => {
            if step {
                replay_step_mode(&timeline)?;
            } else {
                for entry in &timeline {
                    println!("{} {}", entry.marker(), entry.text());
                }
            }
            if timeline.len() < total_timeline_entries {
                println!(
                    "... showing {} of {} timeline entries; drop --last for the full replay.",
                    timeline.len(),
                    total_timeline_entries
                );
            }
        }
    }
    Ok(())
}

fn build_replay_timeline(
    transcript: &[peridot_tui::TranscriptEntry],
    committee: &[CommitteeReplayEvent],
) -> Vec<ReplayTimelineEntry> {
    if transcript.iter().any(|entry| entry.ts > 0) {
        return build_timestamped_timeline(transcript, committee);
    }
    build_legacy_timeline(transcript, committee)
}

fn build_timestamped_timeline(
    transcript: &[peridot_tui::TranscriptEntry],
    committee: &[CommitteeReplayEvent],
) -> Vec<ReplayTimelineEntry> {
    let mut used_committee = vec![false; committee.len()];
    let mut timeline = Vec::new();
    for (order, entry) in transcript.iter().cloned().enumerate() {
        if let Some(index) = matching_committee_event_index(&entry, committee, &used_committee) {
            used_committee[index] = true;
            continue;
        }
        timeline.push(ReplayTimelineEntry::Transcript { entry, order });
    }
    for event in committee.iter().cloned() {
        timeline.push(ReplayTimelineEntry::Committee { event });
    }
    timeline.sort_by_key(|entry| (entry.ts(), entry.order()));
    timeline
}

fn build_legacy_timeline(
    transcript: &[peridot_tui::TranscriptEntry],
    committee: &[CommitteeReplayEvent],
) -> Vec<ReplayTimelineEntry> {
    let mut used_committee = vec![false; committee.len()];
    let mut timeline = Vec::new();
    for (order, entry) in transcript.iter().cloned().enumerate() {
        if let Some(index) = matching_committee_event_index(&entry, committee, &used_committee) {
            push_related_role_usage(index, committee, &mut used_committee, &mut timeline);
            used_committee[index] = true;
            timeline.push(ReplayTimelineEntry::Committee {
                event: committee[index].clone(),
            });
        } else {
            timeline.push(ReplayTimelineEntry::Transcript { entry, order });
        }
    }
    for (index, event) in committee.iter().cloned().enumerate() {
        if !used_committee[index] {
            timeline.push(ReplayTimelineEntry::Committee { event });
        }
    }
    timeline
}

fn push_related_role_usage(
    event_index: usize,
    committee: &[CommitteeReplayEvent],
    used_committee: &mut [bool],
    timeline: &mut Vec<ReplayTimelineEntry>,
) {
    let Some(role) = related_usage_role(&committee[event_index]) else {
        return;
    };
    if let Some(index) = committee.iter().enumerate().position(|(idx, event)| {
        !used_committee[idx]
            && idx < event_index
            && event.kind == "role_usage"
            && event.raw.get("role").and_then(|value| value.as_str()) == Some(role)
    }) {
        used_committee[index] = true;
        timeline.push(ReplayTimelineEntry::Committee {
            event: committee[index].clone(),
        });
    }
}

fn related_usage_role(event: &CommitteeReplayEvent) -> Option<&'static str> {
    match event.kind.as_str() {
        "planner_plan_ready" => Some("planner"),
        "reviewer_verdict" => Some("reviewer"),
        _ => None,
    }
}

fn matching_committee_event_index(
    entry: &peridot_tui::TranscriptEntry,
    committee: &[CommitteeReplayEvent],
    used_committee: &[bool],
) -> Option<usize> {
    if entry.text.starts_with("committee planner ready:") {
        return committee.iter().enumerate().position(|(index, event)| {
            !used_committee[index] && event.kind == "planner_plan_ready"
        });
    }
    if !entry.text.starts_with("committee reviewer (turn ") {
        return None;
    }
    committee.iter().enumerate().position(|(index, event)| {
        !used_committee[index]
            && event.kind == "reviewer_verdict"
            && entry.text.starts_with(&reviewer_text_prefix(event))
    })
}

fn reviewer_text_prefix(event: &CommitteeReplayEvent) -> String {
    let turn = event
        .raw
        .get("turn_index")
        .and_then(|value| value.as_u64())
        .unwrap_or_default();
    let verdict = event
        .raw
        .get("verdict")
        .and_then(|value| value.as_str())
        .unwrap_or("unknown");
    format!("committee reviewer (turn {turn}): {verdict}")
}

fn committee_marker(event: &CommitteeReplayEvent) -> &'static str {
    match event.kind.as_str() {
        "reviewer_verdict" => match event.raw.get("verdict").and_then(|value| value.as_str()) {
            Some("approve") => "·",
            Some("block") => "⚠",
            _ => "⚠",
        },
        _ => "·",
    }
}

fn committee_text(event: &CommitteeReplayEvent) -> String {
    match event.kind.as_str() {
        "planner_plan_ready" => format!(
            "committee planner ready:\n{}",
            event
                .raw
                .get("plan_text")
                .and_then(|value| value.as_str())
                .unwrap_or("")
        ),
        "role_usage" => {
            let role = event
                .raw
                .get("role")
                .and_then(|value| value.as_str())
                .unwrap_or("unknown");
            let cost = event
                .raw
                .get("cost_usd")
                .and_then(|value| value.as_f64())
                .unwrap_or_default();
            let tokens = event
                .raw
                .get("tokens")
                .and_then(|value| value.as_u64())
                .unwrap_or_default();
            format!("committee {role} usage: +${cost:.4} / +{tokens} tok")
        }
        "reviewer_verdict" => {
            let prefix = reviewer_text_prefix(event);
            let comments = event
                .raw
                .get("comments")
                .and_then(|value| value.as_str())
                .unwrap_or("");
            if comments.is_empty() {
                prefix
            } else {
                format!("{prefix} — {comments}")
            }
        }
        other => format!("committee {other}"),
    }
}

fn load_committee_replay_events(project_root: &Path, id: &str) -> Vec<CommitteeReplayEvent> {
    let path = project_root
        .join(".peridot")
        .join("sessions")
        .join(id)
        .join("committee.ndjson");
    let Ok(raw) = std::fs::read_to_string(&path) else {
        return Vec::new();
    };
    raw.lines()
        .filter(|line| !line.trim().is_empty())
        .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
        .enumerate()
        .filter_map(|(order, raw)| {
            let kind = raw.get("kind")?.as_str()?.to_string();
            let ts = raw.get("ts").and_then(|value| value.as_u64()).unwrap_or(0);
            Some(CommitteeReplayEvent {
                ts,
                kind,
                order,
                raw,
            })
        })
        .collect()
}

/// Loads a session's transcript by preferring the canonical `tui_state.json`
/// snapshot and falling back to the incremental `transcript.ndjson` journal
/// (M9). Returns an error only when both sources are missing.
fn load_session_transcript(
    project_root: &Path,
    id: &str,
) -> Result<Vec<peridot_tui::TranscriptEntry>> {
    let sessions_root = project_root.join(".peridot").join("sessions");
    if let Some(bytes) = peridot_memory::load_session_blob(&sessions_root, id, "tui_state.json")? {
        let state: peridot_tui::TuiState = serde_json::from_slice(&bytes)
            .with_context(|| format!("failed to parse persisted tui_state.json for {id}"))?;
        return Ok(state.transcript);
    }
    let ndjson_path = sessions_root.join(id).join("transcript.ndjson");
    if !ndjson_path.exists() {
        anyhow::bail!(
            "no persisted transcript for session {id} (looked for tui_state.json and transcript.ndjson)"
        );
    }
    let raw = std::fs::read_to_string(&ndjson_path).with_context(|| {
        format!(
            "failed to read transcript.ndjson at {}",
            ndjson_path.display()
        )
    })?;
    let mut entries = Vec::new();
    for (line_idx, line) in raw.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let entry: peridot_tui::TranscriptEntry =
            serde_json::from_str(line).with_context(|| {
                format!(
                    "failed to parse line {} of {}",
                    line_idx + 1,
                    ndjson_path.display()
                )
            })?;
        entries.push(entry);
    }
    Ok(entries)
}

fn replay_step_mode(entries: &[&ReplayTimelineEntry]) -> Result<()> {
    use std::io::{BufRead, Write};
    let stdin = std::io::stdin();
    let mut stdin_lock = stdin.lock();
    let stdout = std::io::stdout();
    let total = entries.len();
    let mut buffer = String::new();
    for (idx, entry) in entries.iter().enumerate() {
        println!(
            "[{}/{}] {} {}",
            idx + 1,
            total,
            entry.marker(),
            entry.text()
        );
        if idx + 1 < total {
            {
                let mut handle = stdout.lock();
                write!(
                    handle,
                    "press Enter for next ({}/{}), q+Enter to quit > ",
                    idx + 2,
                    total
                )?;
                handle.flush()?;
            }
            buffer.clear();
            stdin_lock.read_line(&mut buffer)?;
            if matches!(buffer.trim(), "q" | "quit") {
                println!("replay aborted at entry {}/{}.", idx + 1, total);
                break;
            }
        }
    }
    Ok(())
}

fn transcript_marker(kind: peridot_tui::TranscriptKind) -> &'static str {
    use peridot_tui::TranscriptKind as K;
    match kind {
        K::User => "▸",
        K::Assistant => "◆",
        K::ToolStart => "❯",
        K::ToolOk => "✔",
        K::ToolFail => "✘",
        K::System => "·",
        K::Notice => "⚠",
        K::Error => "⚠",
        K::Debug => "?",
        K::TurnSeparator => "—",
        K::Thinking => "…",
        K::Meta => "·",
        K::Diff => "±",
    }
}

pub(super) fn memory_store(project_root: &Path) -> MemoryStore {
    MemoryStore::new(project_root.join(".peridot/memory.db"))
}
