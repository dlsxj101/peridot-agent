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
            match output {
                OutputFormat::Json => println!("{}", serde_json::to_string_pretty(&sessions)?),
                OutputFormat::Text => {
                    for session in sessions {
                        println!("{}\t{}", session.id, session.summary);
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
            match output {
                OutputFormat::Json => println!("{}", serde_json::to_string_pretty(&session)?),
                OutputFormat::Text => match session {
                    Some(session) => println!("{}\t{}", session.id, session.summary),
                    None => println!("session not found: {id}"),
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
    }
    Ok(())
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
