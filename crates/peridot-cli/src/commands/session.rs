use super::*;

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
            let session = store
                .get_session(id)?
                .with_context(|| format!("session not found: {id}"))?;
            let resume_task = format!(
                "Resume session {} from this summary: {}",
                session.id, session.summary
            );
            match output {
                OutputFormat::Json => println!(
                    "{}",
                    serde_json::to_string_pretty(&serde_json::json!({
                        "id": session.id,
                        "summary": session.summary,
                        "resume_task": resume_task
                    }))?
                ),
                OutputFormat::Text => println!("{resume_task}"),
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
            prune_sessions(
                &store,
                project_root,
                status.as_deref(),
                *older_than_days,
                *dry_run,
                output,
            )?;
        }
        SessionCommand::Export { id, out, force } => {
            export_session(project_root, id, out, *force, output)?;
        }
        SessionCommand::Import { from, id, force } => {
            import_session(&store, project_root, from, id.as_deref(), *force, output)?;
        }
        SessionCommand::Note { id, action } => {
            handle_session_note(project_root, id, action, output)?;
        }
        SessionCommand::Locate { id } => {
            let path = project_root.join(".peridot").join("sessions").join(id);
            let exists = path.is_dir();
            match output {
                OutputFormat::Json => {
                    println!(
                        "{}",
                        serde_json::to_string_pretty(&serde_json::json!({
                            "id": id,
                            "path": path.display().to_string(),
                            "exists": exists,
                        }))?
                    );
                }
                OutputFormat::Text => {
                    if exists {
                        println!("{}", path.display());
                    } else {
                        println!("{} (not present)", path.display());
                    }
                }
            }
        }
        SessionCommand::Count => {
            let records = store.list_session_records().unwrap_or_default();
            let total = records.len();
            let mut idle = 0usize;
            let mut running = 0usize;
            let mut suspended = 0usize;
            let mut done = 0usize;
            let mut failed = 0usize;
            for record in &records {
                use peridot_memory::SessionLifecycle;
                match record.status {
                    SessionLifecycle::Idle => idle += 1,
                    SessionLifecycle::Running => running += 1,
                    SessionLifecycle::Suspended => suspended += 1,
                    SessionLifecycle::Done => done += 1,
                    SessionLifecycle::Failed => failed += 1,
                }
            }
            match output {
                OutputFormat::Json => {
                    println!(
                        "{}",
                        serde_json::to_string_pretty(&serde_json::json!({
                            "total": total,
                            "idle": idle,
                            "running": running,
                            "suspended": suspended,
                            "done": done,
                            "failed": failed,
                        }))?
                    );
                }
                OutputFormat::Text => {
                    println!("total:     {total}");
                    println!("idle:      {idle}");
                    println!("running:   {running}");
                    println!("suspended: {suspended}");
                    println!("done:      {done}");
                    println!("failed:    {failed}");
                }
            }
        }
    }
    Ok(())
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

fn handle_session_note(
    project_root: &Path,
    id: &str,
    action: &SessionNoteAction,
    output: OutputFormat,
) -> Result<()> {
    let notes_path = project_root
        .join(".peridot")
        .join("sessions")
        .join(id)
        .join("notes.ndjson");
    match action {
        SessionNoteAction::Add { text } => {
            let body = text.join(" ").trim().to_string();
            if body.is_empty() {
                anyhow::bail!("note text must not be empty");
            }
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
            print_json_or_text_result(
                serde_json::json!({"added": true, "id": id, "ts": timestamp, "text": body}),
                format!("added note to {id}: {body}"),
                output,
            )?;
        }
        SessionNoteAction::List { last } => {
            let raw = match std::fs::read_to_string(&notes_path) {
                Ok(v) => v,
                Err(err) if err.kind() == std::io::ErrorKind::NotFound => String::new(),
                Err(err) => {
                    return Err(err).context(format!("failed to read {}", notes_path.display()));
                }
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
            let trimmed: Vec<&serde_json::Value> = if let Some(limit) = last {
                let start = total.saturating_sub(*limit);
                notes[start..].iter().collect()
            } else {
                notes.iter().collect()
            };
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
            let removed = std::fs::remove_file(&notes_path).is_ok();
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
    match output {
        OutputFormat::Json => {
            println!(
                "{}",
                serde_json::to_string_pretty(&serde_json::json!({
                    "id": derived_id,
                    "source": from.display().to_string(),
                    "destination": destination.display().to_string(),
                    "files": copied,
                }))?
            );
        }
        OutputFormat::Text => {
            println!(
                "imported session {derived_id} from {} into {} ({} entries)",
                from.display(),
                destination.display(),
                copied.len()
            );
            for name in &copied {
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
}

fn export_session(
    project_root: &Path,
    id: &str,
    out_dir: &Path,
    force: bool,
    output: OutputFormat,
) -> Result<()> {
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
    let mut copied: Vec<String> = Vec::new();
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
    match output {
        OutputFormat::Json => {
            println!(
                "{}",
                serde_json::to_string_pretty(&serde_json::json!({
                    "id": id,
                    "source": source.display().to_string(),
                    "destination": out_dir.display().to_string(),
                    "files": copied,
                }))?
            );
        }
        OutputFormat::Text => {
            println!(
                "exported session {id} from {} to {} ({} entries)",
                source.display(),
                out_dir.display(),
                copied.len()
            );
            for name in &copied {
                println!("  - {name}");
            }
        }
    }
    Ok(())
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

fn prune_sessions(
    store: &MemoryStore,
    project_root: &Path,
    status_filter: Option<&str>,
    older_than_days: Option<u64>,
    dry_run: bool,
    output: OutputFormat,
) -> Result<()> {
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
    match output {
        OutputFormat::Json => {
            println!(
                "{}",
                serde_json::to_string_pretty(&serde_json::json!({
                    "dry_run": dry_run,
                    "considered": considered,
                    "removed": removed,
                    "status_filter": status_filter,
                    "older_than_days": older_than_days,
                }))?
            );
        }
        OutputFormat::Text => {
            if dry_run {
                if considered.is_empty() {
                    println!("prune (dry-run): no matching sessions");
                } else {
                    println!(
                        "prune (dry-run): would remove {} session(s):",
                        considered.len()
                    );
                    for id in &considered {
                        println!("  - {id}");
                    }
                }
            } else if removed.is_empty() {
                println!("prune: no matching sessions");
            } else {
                println!("prune: removed {} session(s):", removed.len());
                for id in &removed {
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
    if query.is_empty() {
        anyhow::bail!("search query must not be empty");
    }
    let needle = query.to_ascii_lowercase();
    let sessions_root = project_root.join(".peridot").join("sessions");
    let session_ids: Vec<String> = match only_session {
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
    let mut hits: Vec<serde_json::Value> = Vec::new();
    let cap = limit.unwrap_or(usize::MAX);
    'outer: for id in session_ids {
        let Ok(entries) = load_session_transcript(project_root, &id) else {
            continue;
        };
        for (index, entry) in entries.iter().enumerate() {
            if entry.text.to_ascii_lowercase().contains(&needle) {
                hits.push(serde_json::json!({
                    "session": id,
                    "index": index,
                    "kind": format!("{:?}", entry.kind).to_ascii_lowercase(),
                    "text": entry.text,
                }));
                if hits.len() >= cap {
                    break 'outer;
                }
            }
        }
    }
    match output {
        OutputFormat::Json => {
            println!(
                "{}",
                serde_json::to_string_pretty(&serde_json::json!({
                    "query": query,
                    "total": hits.len(),
                    "hits": hits,
                }))?
            );
        }
        OutputFormat::Text => {
            if hits.is_empty() {
                println!("no matches for '{query}'");
            } else {
                for hit in &hits {
                    println!(
                        "{}[{}] {} {}",
                        hit["session"].as_str().unwrap_or("?"),
                        hit["index"].as_u64().unwrap_or_default(),
                        hit["kind"].as_str().unwrap_or("?"),
                        hit["text"].as_str().unwrap_or(""),
                    );
                }
            }
        }
    }
    Ok(())
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

fn replay_session_transcript(
    project_root: &Path,
    id: &str,
    last: Option<usize>,
    step: bool,
    output: OutputFormat,
) -> Result<()> {
    let entries_owned = load_session_transcript(project_root, id)?;
    let entries: Vec<&peridot_tui::TranscriptEntry> = if let Some(limit) = last {
        let total = entries_owned.len();
        let start = total.saturating_sub(limit);
        entries_owned[start..].iter().collect()
    } else {
        entries_owned.iter().collect()
    };
    let total_entries = entries_owned.len();
    match output {
        OutputFormat::Json => {
            let payload: Vec<_> = entries
                .iter()
                .map(|entry| {
                    serde_json::json!({
                        "kind": format!("{:?}", entry.kind).to_ascii_lowercase(),
                        "text": entry.text,
                    })
                })
                .collect();
            println!(
                "{}",
                serde_json::to_string_pretty(&serde_json::json!({
                    "id": id,
                    "entries": payload,
                    "total": total_entries,
                    "step_mode": step,
                }))?
            );
        }
        OutputFormat::Text => {
            if step {
                replay_step_mode(&entries)?;
            } else {
                for entry in &entries {
                    let marker = transcript_marker(entry.kind);
                    println!("{marker} {}", entry.text);
                }
            }
            if entries.len() < total_entries {
                println!(
                    "... showing {} of {} entries; drop --last for the full replay.",
                    entries.len(),
                    total_entries
                );
            }
        }
    }
    Ok(())
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

fn replay_step_mode(entries: &[&peridot_tui::TranscriptEntry]) -> Result<()> {
    use std::io::{BufRead, Write};
    let stdin = std::io::stdin();
    let mut stdin_lock = stdin.lock();
    let stdout = std::io::stdout();
    let total = entries.len();
    let mut buffer = String::new();
    for (idx, entry) in entries.iter().enumerate() {
        let marker = transcript_marker(entry.kind);
        println!("[{}/{}] {marker} {}", idx + 1, total, entry.text);
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
