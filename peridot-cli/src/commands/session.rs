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
