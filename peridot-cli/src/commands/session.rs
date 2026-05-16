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
        SessionCommand::Replay { id, last } => {
            replay_session_transcript(project_root, id, *last, output)?;
        }
    }
    Ok(())
}

fn replay_session_transcript(
    project_root: &Path,
    id: &str,
    last: Option<usize>,
    output: OutputFormat,
) -> Result<()> {
    let sessions_root = project_root.join(".peridot").join("sessions");
    let bytes = peridot_memory::load_session_blob(&sessions_root, id, "tui_state.json")?
        .with_context(|| format!("no persisted tui_state.json for session {id}"))?;
    let state: peridot_tui::TuiState = serde_json::from_slice(&bytes)
        .with_context(|| format!("failed to parse persisted tui_state.json for {id}"))?;
    let entries: Vec<&peridot_tui::TranscriptEntry> = if let Some(limit) = last {
        let total = state.transcript.len();
        let start = total.saturating_sub(limit);
        state.transcript[start..].iter().collect()
    } else {
        state.transcript.iter().collect()
    };
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
                    "total": state.transcript.len(),
                }))?
            );
        }
        OutputFormat::Text => {
            for entry in &entries {
                let marker = transcript_marker(entry.kind);
                println!("{marker} {}", entry.text);
            }
            if entries.len() < state.transcript.len() {
                println!(
                    "... showing {} of {} entries; drop --last for the full replay.",
                    entries.len(),
                    state.transcript.len()
                );
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
