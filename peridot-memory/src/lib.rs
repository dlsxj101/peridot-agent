//! Session and learned-memory persistence boundary.

use std::fs;
use std::path::PathBuf;

use peridot_common::{PeriError, PeriResult};
use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};

/// Stored session summary.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SessionSummary {
    /// Session identifier.
    pub id: String,
    /// Human-readable summary.
    pub summary: String,
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
                "#,
            )
            .map_err(sql_error)?;
        Ok(())
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
