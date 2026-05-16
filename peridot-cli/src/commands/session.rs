use super::*;

pub(crate) fn run_session_command(
    command: &SessionCommand,
    project_root: &Path,
    output: OutputFormat,
) -> Result<()> {
    let store = memory_store(project_root);
    match command {
        SessionCommand::List => {
            let sessions = store.list_sessions()?;
            let records = store.list_session_records().unwrap_or_default();
            let record_for = |id: &str| records.iter().find(|r| r.id == id).cloned();
            match output {
                OutputFormat::Json => {
                    let payload: Vec<_> = sessions
                        .iter()
                        .map(|session| {
                            let record = record_for(&session.id);
                            serde_json::json!({
                                "id": session.id,
                                "summary": session.summary,
                                "record": record,
                            })
                        })
                        .collect();
                    println!("{}", serde_json::to_string_pretty(&payload)?);
                }
                OutputFormat::Text => {
                    for session in sessions {
                        let record = record_for(&session.id);
                        let suffix = record
                            .as_ref()
                            .map(|r| {
                                format!(
                                    "\tstatus={:?}\ttokens={}\tcost=${:.4}\tturns={}",
                                    r.status, r.total_tokens, r.total_cost_usd, r.turns_used,
                                )
                            })
                            .unwrap_or_default();
                        println!("{}\t{}{}", session.id, session.summary, suffix);
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
        SessionCommand::Show { id } => {
            let session = store.get_session(id)?;
            let record = store.get_session_record(id).unwrap_or_default();
            match output {
                OutputFormat::Json => {
                    println!(
                        "{}",
                        serde_json::to_string_pretty(&serde_json::json!({
                            "session": session,
                            "record": record,
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
            let deleted = store.delete_session(id)?;
            print_json_or_text_result(
                serde_json::json!({"deleted": deleted, "id": id}),
                format!("deleted session {id}: {deleted}"),
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
    }
}

pub(super) fn memory_store(project_root: &Path) -> MemoryStore {
    MemoryStore::new(project_root.join(".peridot/memory.db"))
}
