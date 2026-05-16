//! Session and learned-memory persistence boundary.

use std::fs;
use std::path::{Path, PathBuf};

use peridot_common::{PeriError, PeriResult};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};

/// Lifecycle stage of a stored session.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionLifecycle {
    /// Session is reserved but no run is active.
    #[default]
    Idle,
    /// Session has a running agent loop.
    Running,
    /// Session was paused / backgrounded.
    Suspended,
    /// Session terminated successfully.
    Done,
    /// Session terminated in failure.
    Failed,
}

impl SessionLifecycle {
    fn as_db(self) -> &'static str {
        match self {
            SessionLifecycle::Idle => "idle",
            SessionLifecycle::Running => "running",
            SessionLifecycle::Suspended => "suspended",
            SessionLifecycle::Done => "done",
            SessionLifecycle::Failed => "failed",
        }
    }

    fn from_db(value: &str) -> Self {
        match value {
            "running" => SessionLifecycle::Running,
            "suspended" => SessionLifecycle::Suspended,
            "done" => SessionLifecycle::Done,
            "failed" => SessionLifecycle::Failed,
            _ => SessionLifecycle::Idle,
        }
    }
}

/// Stored session summary (legacy table, used by `peridot session save`).
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SessionSummary {
    /// Session identifier.
    pub id: String,
    /// Human-readable summary.
    pub summary: String,
}

/// Extended session record used by the multi-session TUI to resume work.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SessionRecord {
    /// Stable session identifier.
    pub id: String,
    /// Human-readable summary (latest known).
    pub summary: String,
    /// Current lifecycle stage.
    pub status: SessionLifecycle,
    /// Creation time (unix seconds).
    pub created_at_unix: u64,
    /// Last update time (unix seconds).
    pub updated_at_unix: u64,
    /// Project root where the session was created.
    pub workspace_root: PathBuf,
    /// Optional git worktree branch isolating the session.
    pub worktree_branch: Option<String>,
    /// Most recent task text submitted by the user.
    pub last_task: Option<String>,
    /// Total provider tokens consumed.
    pub total_tokens: u64,
    /// Total estimated cost in USD.
    pub total_cost_usd: f64,
    /// Turns the agent has consumed.
    pub turns_used: u32,
}

impl SessionRecord {
    /// Builds a fresh idle record with timestamps zeroed (caller fills them).
    pub fn new(id: impl Into<String>, workspace_root: impl Into<PathBuf>) -> Self {
        Self {
            id: id.into(),
            summary: String::new(),
            status: SessionLifecycle::Idle,
            created_at_unix: 0,
            updated_at_unix: 0,
            workspace_root: workspace_root.into(),
            worktree_branch: None,
            last_task: None,
            total_tokens: 0,
            total_cost_usd: 0.0,
            turns_used: 0,
        }
    }
}

/// Stored learned skill.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct StoredSkill {
    /// Skill name.
    pub name: String,
    /// Skill body.
    pub body: String,
}

/// Stored error resolution.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ErrorResolution {
    /// Stable error signature.
    pub signature: String,
    /// Resolution notes.
    pub resolution: String,
}

/// SQLite-backed memory store.
#[derive(Clone, Debug)]
pub struct MemoryStore {
    path: PathBuf,
}

impl MemoryStore {
    /// Creates a memory store pointing at a SQLite database path.
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    /// Returns the configured database path.
    pub fn path(&self) -> &PathBuf {
        &self.path
    }

    /// Initializes the SQLite schema if it does not exist.
    pub fn initialize(&self) -> PeriResult<()> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent).map_err(|err| {
                PeriError::Tool(format!("failed to create {}: {err}", parent.display()))
            })?;
        }
        let connection = self.connection()?;
        connection
            .execute_batch(
                r#"
                CREATE TABLE IF NOT EXISTS sessions (
                    id TEXT PRIMARY KEY,
                    summary TEXT NOT NULL,
                    updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
                );
                CREATE TABLE IF NOT EXISTS skills (
                    name TEXT PRIMARY KEY,
                    body TEXT NOT NULL,
                    updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
                );
                CREATE TABLE IF NOT EXISTS errors (
                    signature TEXT PRIMARY KEY,
                    resolution TEXT NOT NULL,
                    updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
                );
                CREATE TABLE IF NOT EXISTS session_records (
                    id TEXT PRIMARY KEY,
                    summary TEXT NOT NULL DEFAULT '',
                    status TEXT NOT NULL DEFAULT 'idle',
                    created_at_unix INTEGER NOT NULL DEFAULT 0,
                    updated_at_unix INTEGER NOT NULL DEFAULT 0,
                    workspace_root TEXT NOT NULL DEFAULT '',
                    worktree_branch TEXT,
                    last_task TEXT,
                    total_tokens INTEGER NOT NULL DEFAULT 0,
                    total_cost_usd REAL NOT NULL DEFAULT 0.0,
                    turns_used INTEGER NOT NULL DEFAULT 0
                );
                "#,
            )
            .map_err(sql_error)?;
        Ok(())
    }

    /// Saves or replaces a full session record.
    pub fn save_session_record(&self, record: &SessionRecord) -> PeriResult<()> {
        self.initialize()?;
        let connection = self.connection()?;
        connection
            .execute(
                r#"
                INSERT INTO session_records (
                    id, summary, status,
                    created_at_unix, updated_at_unix,
                    workspace_root, worktree_branch, last_task,
                    total_tokens, total_cost_usd, turns_used
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)
                ON CONFLICT(id) DO UPDATE SET
                    summary = excluded.summary,
                    status = excluded.status,
                    updated_at_unix = excluded.updated_at_unix,
                    workspace_root = excluded.workspace_root,
                    worktree_branch = excluded.worktree_branch,
                    last_task = excluded.last_task,
                    total_tokens = excluded.total_tokens,
                    total_cost_usd = excluded.total_cost_usd,
                    turns_used = excluded.turns_used
                "#,
                params![
                    record.id,
                    record.summary,
                    record.status.as_db(),
                    record.created_at_unix as i64,
                    record.updated_at_unix as i64,
                    record.workspace_root.to_string_lossy().to_string(),
                    record.worktree_branch,
                    record.last_task,
                    record.total_tokens as i64,
                    record.total_cost_usd,
                    record.turns_used as i64,
                ],
            )
            .map_err(sql_error)?;
        Ok(())
    }

    /// Updates only the lifecycle stage for an existing session record.
    pub fn update_session_lifecycle(
        &self,
        id: &str,
        status: SessionLifecycle,
        updated_at_unix: u64,
    ) -> PeriResult<()> {
        self.initialize()?;
        let connection = self.connection()?;
        connection
            .execute(
                "UPDATE session_records SET status = ?1, updated_at_unix = ?2 WHERE id = ?3",
                params![status.as_db(), updated_at_unix as i64, id],
            )
            .map_err(sql_error)?;
        Ok(())
    }

    /// Loads one session record by id.
    pub fn get_session_record(&self, id: &str) -> PeriResult<Option<SessionRecord>> {
        self.initialize()?;
        let connection = self.connection()?;
        let mut statement = connection
            .prepare(
                r#"
                SELECT id, summary, status, created_at_unix, updated_at_unix,
                       workspace_root, worktree_branch, last_task,
                       total_tokens, total_cost_usd, turns_used
                FROM session_records WHERE id = ?1
                "#,
            )
            .map_err(sql_error)?;
        let row = statement
            .query_row([id], session_record_from_row)
            .optional()
            .map_err(sql_error)?;
        Ok(row)
    }

    /// Lists session records in latest-first order.
    pub fn list_session_records(&self) -> PeriResult<Vec<SessionRecord>> {
        self.initialize()?;
        let connection = self.connection()?;
        let mut statement = connection
            .prepare(
                r#"
                SELECT id, summary, status, created_at_unix, updated_at_unix,
                       workspace_root, worktree_branch, last_task,
                       total_tokens, total_cost_usd, turns_used
                FROM session_records
                ORDER BY updated_at_unix DESC, id ASC
                "#,
            )
            .map_err(sql_error)?;
        let rows = statement
            .query_map([], session_record_from_row)
            .map_err(sql_error)?;
        collect_rows(rows)
    }

    /// Removes a session record (does not touch blob files).
    pub fn delete_session_record(&self, id: &str) -> PeriResult<bool> {
        self.initialize()?;
        let connection = self.connection()?;
        let changed = connection
            .execute("DELETE FROM session_records WHERE id = ?1", [id])
            .map_err(sql_error)?;
        Ok(changed > 0)
    }

    /// Saves or replaces a session summary.
    pub fn save_session(&self, session: &SessionSummary) -> PeriResult<()> {
        self.initialize()?;
        let connection = self.connection()?;
        connection
            .execute(
                r#"
                INSERT INTO sessions (id, summary, updated_at)
                VALUES (?1, ?2, CURRENT_TIMESTAMP)
                ON CONFLICT(id) DO UPDATE SET
                    summary = excluded.summary,
                    updated_at = CURRENT_TIMESTAMP
                "#,
                params![session.id, session.summary],
            )
            .map_err(sql_error)?;
        Ok(())
    }

    /// Lists saved sessions in latest-first order.
    pub fn list_sessions(&self) -> PeriResult<Vec<SessionSummary>> {
        self.initialize()?;
        let connection = self.connection()?;
        let mut statement = connection
            .prepare("SELECT id, summary FROM sessions ORDER BY updated_at DESC, id ASC")
            .map_err(sql_error)?;
        let rows = statement
            .query_map([], |row| {
                Ok(SessionSummary {
                    id: row.get(0)?,
                    summary: row.get(1)?,
                })
            })
            .map_err(sql_error)?;
        let mut sessions = Vec::new();
        for row in rows {
            sessions.push(row.map_err(sql_error)?);
        }
        Ok(sessions)
    }

    /// Fetches one saved session summary by id.
    pub fn get_session(&self, id: &str) -> PeriResult<Option<SessionSummary>> {
        self.initialize()?;
        let connection = self.connection()?;
        let mut statement = connection
            .prepare("SELECT id, summary FROM sessions WHERE id = ?1")
            .map_err(sql_error)?;
        let result = statement.query_row([id], |row| {
            Ok(SessionSummary {
                id: row.get(0)?,
                summary: row.get(1)?,
            })
        });
        match result {
            Ok(session) => Ok(Some(session)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(err) => Err(sql_error(err)),
        }
    }

    /// Deletes a saved session summary.
    pub fn delete_session(&self, id: &str) -> PeriResult<bool> {
        self.initialize()?;
        let connection = self.connection()?;
        let changed = connection
            .execute("DELETE FROM sessions WHERE id = ?1", [id])
            .map_err(sql_error)?;
        Ok(changed > 0)
    }

    /// Saves or replaces a learned skill.
    pub fn save_skill(&self, skill: &StoredSkill) -> PeriResult<()> {
        self.initialize()?;
        let connection = self.connection()?;
        connection
            .execute(
                r#"
                INSERT INTO skills (name, body, updated_at)
                VALUES (?1, ?2, CURRENT_TIMESTAMP)
                ON CONFLICT(name) DO UPDATE SET
                    body = excluded.body,
                    updated_at = CURRENT_TIMESTAMP
                "#,
                params![skill.name, skill.body],
            )
            .map_err(sql_error)?;
        Ok(())
    }

    /// Lists learned skills in latest-first order.
    pub fn list_skills(&self) -> PeriResult<Vec<StoredSkill>> {
        self.initialize()?;
        let connection = self.connection()?;
        let mut statement = connection
            .prepare("SELECT name, body FROM skills ORDER BY updated_at DESC, name ASC")
            .map_err(sql_error)?;
        let rows = statement
            .query_map([], |row| {
                Ok(StoredSkill {
                    name: row.get(0)?,
                    body: row.get(1)?,
                })
            })
            .map_err(sql_error)?;
        collect_rows(rows)
    }

    /// Searches learned skills by name or body text.
    pub fn search_skills(&self, query: &str) -> PeriResult<Vec<StoredSkill>> {
        self.initialize()?;
        let pattern = format!("%{query}%");
        let connection = self.connection()?;
        let mut statement = connection
            .prepare(
                "SELECT name, body FROM skills
                 WHERE name LIKE ?1 OR body LIKE ?1
                 ORDER BY updated_at DESC, name ASC",
            )
            .map_err(sql_error)?;
        let rows = statement
            .query_map([pattern], |row| {
                Ok(StoredSkill {
                    name: row.get(0)?,
                    body: row.get(1)?,
                })
            })
            .map_err(sql_error)?;
        collect_rows(rows)
    }

    /// Saves or replaces an error resolution.
    pub fn save_error_resolution(&self, resolution: &ErrorResolution) -> PeriResult<()> {
        self.initialize()?;
        let connection = self.connection()?;
        connection
            .execute(
                r#"
                INSERT INTO errors (signature, resolution, updated_at)
                VALUES (?1, ?2, CURRENT_TIMESTAMP)
                ON CONFLICT(signature) DO UPDATE SET
                    resolution = excluded.resolution,
                    updated_at = CURRENT_TIMESTAMP
                "#,
                params![resolution.signature, resolution.resolution],
            )
            .map_err(sql_error)?;
        Ok(())
    }

    /// Fetches one stored error resolution.
    pub fn get_error_resolution(&self, signature: &str) -> PeriResult<Option<ErrorResolution>> {
        self.initialize()?;
        let connection = self.connection()?;
        let mut statement = connection
            .prepare("SELECT signature, resolution FROM errors WHERE signature = ?1")
            .map_err(sql_error)?;
        let result = statement.query_row([signature], |row| {
            Ok(ErrorResolution {
                signature: row.get(0)?,
                resolution: row.get(1)?,
            })
        });
        match result {
            Ok(resolution) => Ok(Some(resolution)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(err) => Err(sql_error(err)),
        }
    }

    fn connection(&self) -> PeriResult<Connection> {
        Connection::open(&self.path).map_err(sql_error)
    }
}

fn session_record_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<SessionRecord> {
    let status_db: String = row.get(2)?;
    let workspace_root: String = row.get(5)?;
    Ok(SessionRecord {
        id: row.get(0)?,
        summary: row.get(1)?,
        status: SessionLifecycle::from_db(&status_db),
        created_at_unix: row.get::<_, i64>(3)? as u64,
        updated_at_unix: row.get::<_, i64>(4)? as u64,
        workspace_root: PathBuf::from(workspace_root),
        worktree_branch: row.get(6)?,
        last_task: row.get(7)?,
        total_tokens: row.get::<_, i64>(8)? as u64,
        total_cost_usd: row.get(9)?,
        turns_used: row.get::<_, i64>(10)? as u32,
    })
}

/// Writes a session blob (TUI state, context, transcript log) atomically to
/// `<sessions_root>/<id>/<filename>` using a tempfile + rename.
pub fn save_session_blob(
    sessions_root: &Path,
    id: &str,
    filename: &str,
    bytes: &[u8],
) -> PeriResult<()> {
    let dir = sessions_root.join(id);
    fs::create_dir_all(&dir)
        .map_err(|err| PeriError::Tool(format!("failed to create {}: {err}", dir.display())))?;
    let target = dir.join(filename);
    let temp = dir.join(format!(".{filename}.tmp"));
    fs::write(&temp, bytes)
        .map_err(|err| PeriError::Tool(format!("failed to write {}: {err}", temp.display())))?;
    fs::rename(&temp, &target).map_err(|err| {
        PeriError::Tool(format!(
            "failed to rename {} to {}: {err}",
            temp.display(),
            target.display()
        ))
    })?;
    Ok(())
}

/// Reads a previously saved session blob; returns None when the file is missing.
pub fn load_session_blob(
    sessions_root: &Path,
    id: &str,
    filename: &str,
) -> PeriResult<Option<Vec<u8>>> {
    let target = sessions_root.join(id).join(filename);
    match fs::read(&target) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(PeriError::Tool(format!(
            "failed to read {}: {err}",
            target.display()
        ))),
    }
}

/// Deletes the per-session directory if it exists.
pub fn remove_session_dir(sessions_root: &Path, id: &str) -> PeriResult<bool> {
    let target = sessions_root.join(id);
    if !target.exists() {
        return Ok(false);
    }
    fs::remove_dir_all(&target)
        .map_err(|err| PeriError::Tool(format!("failed to remove {}: {err}", target.display())))?;
    Ok(true)
}

fn sql_error(err: rusqlite::Error) -> PeriError {
    PeriError::Tool(format!("sqlite error: {err}"))
}

fn collect_rows<T>(
    rows: rusqlite::MappedRows<'_, impl FnMut(&rusqlite::Row<'_>) -> rusqlite::Result<T>>,
) -> PeriResult<Vec<T>> {
    let mut values = Vec::new();
    for row in rows {
        values.push(row.map_err(sql_error)?);
    }
    Ok(values)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn saves_and_lists_sessions() {
        let root = std::env::temp_dir().join(format!("peridot-memory-{}", std::process::id()));
        let store = MemoryStore::new(root.join("memory.db"));
        store
            .save_session(&SessionSummary {
                id: "session-1".to_string(),
                summary: "built things".to_string(),
            })
            .unwrap();

        let sessions = store.list_sessions().unwrap();

        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].id, "session-1");
        assert_eq!(
            store.get_session("session-1").unwrap().unwrap().summary,
            "built things"
        );
        assert!(store.delete_session("session-1").unwrap());
        assert!(store.list_sessions().unwrap().is_empty());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn session_record_round_trip_and_lifecycle() {
        let root = std::env::temp_dir().join(format!(
            "peridot-memory-records-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or_default()
        ));
        fs::create_dir_all(&root).unwrap();
        let store = MemoryStore::new(root.join("memory.db"));
        let record = SessionRecord {
            id: "session-a".to_string(),
            summary: "drafted plan".to_string(),
            status: SessionLifecycle::Running,
            created_at_unix: 100,
            updated_at_unix: 100,
            workspace_root: PathBuf::from("/tmp/work"),
            worktree_branch: Some("feat/x".to_string()),
            last_task: Some("rewrite README".to_string()),
            total_tokens: 1500,
            total_cost_usd: 0.034,
            turns_used: 3,
        };
        store.save_session_record(&record).unwrap();

        let restored = store.get_session_record("session-a").unwrap().unwrap();
        assert_eq!(restored, record);
        assert_eq!(store.list_session_records().unwrap().len(), 1);

        store
            .update_session_lifecycle("session-a", SessionLifecycle::Done, 200)
            .unwrap();
        let updated = store.get_session_record("session-a").unwrap().unwrap();
        assert_eq!(updated.status, SessionLifecycle::Done);
        assert_eq!(updated.updated_at_unix, 200);
        assert!(store.delete_session_record("session-a").unwrap());

        save_session_blob(&root, "session-a", "tui_state.json", b"{\"hello\":1}").unwrap();
        let loaded = load_session_blob(&root, "session-a", "tui_state.json")
            .unwrap()
            .unwrap();
        assert_eq!(loaded, b"{\"hello\":1}");
        assert!(
            load_session_blob(&root, "session-a", "missing.bin")
                .unwrap()
                .is_none()
        );
        assert!(remove_session_dir(&root, "session-a").unwrap());

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn saves_and_searches_skills_and_errors() {
        let root =
            std::env::temp_dir().join(format!("peridot-memory-skills-{}", std::process::id()));
        let store = MemoryStore::new(root.join("memory.db"));
        store
            .save_skill(&StoredSkill {
                name: "rust-fmt".to_string(),
                body: "Run cargo fmt before clippy.".to_string(),
            })
            .unwrap();
        store
            .save_error_resolution(&ErrorResolution {
                signature: "clippy-too-many-args".to_string(),
                resolution: "Group related parameters into a struct.".to_string(),
            })
            .unwrap();

        assert_eq!(store.list_skills().unwrap().len(), 1);
        assert_eq!(store.search_skills("fmt").unwrap()[0].name, "rust-fmt");
        assert_eq!(
            store
                .get_error_resolution("clippy-too-many-args")
                .unwrap()
                .unwrap()
                .resolution,
            "Group related parameters into a struct."
        );
        fs::remove_dir_all(root).unwrap();
    }
}
