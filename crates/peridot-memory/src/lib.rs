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
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, Default)]
pub struct StoredSkill {
    /// Skill name.
    pub name: String,
    /// Skill body.
    pub body: String,
    /// Last time the skill body was actually loaded into agent context
    /// (e.g. via `skill_view`). Unix seconds; `0` means never used since
    /// it was saved. Drives the Curator's stale/archive decision.
    #[serde(default)]
    pub last_used_at_unix: u64,
    /// When the Curator (or operator) archived this skill. Unix seconds;
    /// `0` means active. Archived skills stay in the DB for restore but
    /// are excluded from default listings and the auto-activation pool.
    #[serde(default)]
    pub archived_at_unix: u64,
    /// Origin tag controlling whether the Curator may rewrite or archive
    /// this skill. `"auto"` = agent-authored (Curator target).
    /// `"bundled"` / `"community"` / `""` = leave alone. Empty string is
    /// treated as bundled to keep legacy rows safe.
    #[serde(default)]
    pub scope: String,
    /// One-line human description of what the skill is for. Surfaced by
    /// `skill_list` (L0 disclosure) so the model can pick a relevant
    /// skill without paying the body-tokens cost of `skill_view`. Empty
    /// string for legacy rows; new auto-skill writers should populate
    /// this from the YAML frontmatter of the LLM-rewritten SKILL body.
    #[serde(default)]
    pub description: String,
    /// When the operator pinned this skill (`peridot skill pin <name>`
    /// or equivalent). Unix seconds; `0` means not pinned. The Curator
    /// excludes pinned rows from `keep / patch / consolidate / archive`
    /// actions, matching Hermes' pin semantics — patches *can* still
    /// apply if the operator re-edits, but no automated archival.
    #[serde(default)]
    pub pinned_at_unix: u64,
}

/// Snapshot of a skill row plus Curator-relevant metadata.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SkillRecord {
    /// Full stored skill, including body.
    pub skill: StoredSkill,
    /// Last update time (unix seconds) — when the body changed.
    pub updated_at_unix: u64,
}

/// Outcome of the Curator's automatic age-based pipeline (no LLM input).
/// Rules are predictable and offline-safe; they age `scope='auto'` skills
/// against `last_used_at_unix` when set, otherwise `updated_at_unix`.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AutoRuleVerdict {
    /// Recently saved or used; leave alone.
    Active,
    /// Idle past `STALE_THRESHOLD_SECS`. Surface a warning, keep listed.
    Stale,
    /// Idle past `ARCHIVE_THRESHOLD_SECS`. Archive automatically.
    Archive,
}

/// 30 days. Hermes-aligned stale window.
pub const STALE_THRESHOLD_SECS: u64 = 30 * 24 * 3600;
/// 90 days. Hermes-aligned archive window.
pub const ARCHIVE_THRESHOLD_SECS: u64 = 90 * 24 * 3600;

/// Pure 30/90-day verdict for one skill record. Archived rows are
/// always reported as `Active` so the Curator never re-archives.
pub fn auto_rule_verdict(record: &SkillRecord, now_unix: u64) -> AutoRuleVerdict {
    if record.skill.archived_at_unix > 0 {
        return AutoRuleVerdict::Active;
    }
    let reference = if record.skill.last_used_at_unix > 0 {
        record.skill.last_used_at_unix
    } else {
        record.updated_at_unix
    };
    let elapsed = now_unix.saturating_sub(reference);
    if elapsed >= ARCHIVE_THRESHOLD_SECS {
        AutoRuleVerdict::Archive
    } else if elapsed >= STALE_THRESHOLD_SECS {
        AutoRuleVerdict::Stale
    } else {
        AutoRuleVerdict::Active
    }
}

/// Stored error resolution.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ErrorResolution {
    /// Stable error signature.
    pub signature: String,
    /// Resolution notes.
    pub resolution: String,
}

/// One n-gram (length 2 or 3) of tool names observed across sessions.
/// Used by the cross-session reflection pass to spot patterns the
/// operator runs repeatedly and promote them into auto-skills.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ToolNgram {
    /// Stable hash key (BLAKE2 / sha-style) used for upserts.
    pub hash: String,
    /// Ordered tool names that make up this n-gram.
    pub tools: Vec<String>,
    /// How many times this n-gram has been observed across all sessions.
    pub occurrence_count: u32,
    /// Unix timestamp of the most recent observation.
    pub last_seen_unix: u64,
    /// Unix timestamp at which this n-gram was promoted into an
    /// auto-skill. `0` means it has not been promoted yet.
    pub promoted_at_unix: u64,
    /// Session id where this n-gram was last observed (used for
    /// reflection prompts).
    pub last_session_id: String,
    /// Truncated task summary from the most recent session that
    /// produced this n-gram. Useful context for the reflection prompt
    /// without storing every historical occurrence.
    pub last_task_summary: String,
}

impl ToolNgram {
    /// Returns the n-gram length (number of tools).
    pub fn length(&self) -> usize {
        self.tools.len()
    }

    /// Returns true when this n-gram has already been promoted into a
    /// skill (and so should not be promoted again).
    pub fn is_promoted(&self) -> bool {
        self.promoted_at_unix > 0
    }
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
                    updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
                    updated_at_unix INTEGER NOT NULL DEFAULT 0,
                    last_used_at_unix INTEGER NOT NULL DEFAULT 0,
                    archived_at_unix INTEGER NOT NULL DEFAULT 0,
                    scope TEXT NOT NULL DEFAULT ''
                );
                CREATE TABLE IF NOT EXISTS kv_meta (
                    key TEXT PRIMARY KEY,
                    value TEXT NOT NULL
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
                CREATE TABLE IF NOT EXISTS tool_sequences (
                    session_id        TEXT PRIMARY KEY,
                    sequence_json     TEXT NOT NULL,
                    task_summary      TEXT NOT NULL DEFAULT '',
                    created_at_unix   INTEGER NOT NULL DEFAULT 0
                );
                CREATE TABLE IF NOT EXISTS tool_ngrams (
                    ngram_hash        TEXT PRIMARY KEY,
                    ngram_tools       TEXT NOT NULL,
                    ngram_length      INTEGER NOT NULL,
                    occurrence_count  INTEGER NOT NULL DEFAULT 0,
                    last_seen_unix    INTEGER NOT NULL DEFAULT 0,
                    promoted_at_unix  INTEGER NOT NULL DEFAULT 0,
                    last_session_id   TEXT NOT NULL DEFAULT '',
                    last_task_summary TEXT NOT NULL DEFAULT ''
                );
                CREATE TABLE IF NOT EXISTS harness_adjustments (
                    field             TEXT PRIMARY KEY,
                    previous_value    TEXT NOT NULL DEFAULT '',
                    new_value         TEXT NOT NULL DEFAULT '',
                    applied_at_unix   INTEGER NOT NULL DEFAULT 0,
                    signal            TEXT NOT NULL DEFAULT ''
                );
                "#,
            )
            .map_err(sql_error)?;
        ensure_column(
            &connection,
            "skills",
            "updated_at_unix",
            "INTEGER NOT NULL DEFAULT 0",
        )?;
        ensure_column(
            &connection,
            "skills",
            "last_used_at_unix",
            "INTEGER NOT NULL DEFAULT 0",
        )?;
        ensure_column(
            &connection,
            "skills",
            "archived_at_unix",
            "INTEGER NOT NULL DEFAULT 0",
        )?;
        ensure_column(&connection, "skills", "scope", "TEXT NOT NULL DEFAULT ''")?;
        // New in v0.8.11: `description` for L0 list-mode disclosure, and
        // `pinned_at_unix` so operators can shield a skill from the
        // automated Curator. Both columns default to "empty/zero" so
        // existing rows survive the migration unchanged.
        ensure_column(
            &connection,
            "skills",
            "description",
            "TEXT NOT NULL DEFAULT ''",
        )?;
        ensure_column(
            &connection,
            "skills",
            "pinned_at_unix",
            "INTEGER NOT NULL DEFAULT 0",
        )?;
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

    /// Saves or replaces a learned skill. Carries `scope`, `last_used_at_unix`,
    /// and `archived_at_unix` through so the Curator can round-trip records
    /// it has just rewritten. Save with `scope = "auto"` for agent-authored
    /// skills that the Curator is allowed to rewrite/archive.
    pub fn save_skill(&self, skill: &StoredSkill) -> PeriResult<()> {
        self.initialize()?;
        let connection = self.connection()?;
        let now = unix_now() as i64;
        connection
            .execute(
                r#"
                INSERT INTO skills (
                    name, body, updated_at, updated_at_unix,
                    last_used_at_unix, archived_at_unix, scope,
                    description, pinned_at_unix
                )
                VALUES (?1, ?2, CURRENT_TIMESTAMP, ?3, ?4, ?5, ?6, ?7, ?8)
                ON CONFLICT(name) DO UPDATE SET
                    body = excluded.body,
                    updated_at = CURRENT_TIMESTAMP,
                    updated_at_unix = excluded.updated_at_unix,
                    last_used_at_unix = excluded.last_used_at_unix,
                    archived_at_unix = excluded.archived_at_unix,
                    scope = excluded.scope,
                    description = excluded.description,
                    pinned_at_unix = excluded.pinned_at_unix
                "#,
                params![
                    skill.name,
                    skill.body,
                    now,
                    skill.last_used_at_unix as i64,
                    skill.archived_at_unix as i64,
                    skill.scope,
                    skill.description,
                    skill.pinned_at_unix as i64,
                ],
            )
            .map_err(sql_error)?;
        Ok(())
    }

    /// Lists active (non-archived) skills in latest-first order.
    pub fn list_skills(&self) -> PeriResult<Vec<StoredSkill>> {
        self.initialize()?;
        let connection = self.connection()?;
        let mut statement = connection
            .prepare(
                "SELECT name, body, last_used_at_unix, archived_at_unix, scope, \
                        description, pinned_at_unix \
                 FROM skills \
                 WHERE archived_at_unix = 0 \
                 ORDER BY updated_at DESC, name ASC",
            )
            .map_err(sql_error)?;
        let rows = statement
            .query_map([], stored_skill_from_row)
            .map_err(sql_error)?;
        collect_rows(rows)
    }

    /// Lists every skill row including archived ones plus their last update
    /// timestamp. The Curator uses this for stale/archive decisions.
    pub fn list_skill_records(&self) -> PeriResult<Vec<SkillRecord>> {
        self.initialize()?;
        let connection = self.connection()?;
        let mut statement = connection
            .prepare(
                "SELECT name, body, last_used_at_unix, archived_at_unix, scope, \
                        description, pinned_at_unix, updated_at_unix \
                 FROM skills \
                 ORDER BY updated_at_unix DESC, name ASC",
            )
            .map_err(sql_error)?;
        let rows = statement
            .query_map([], |row| {
                // `updated_at_unix` lives in slot 7 now that the
                // decoder consumes columns 0..=6.
                Ok(SkillRecord {
                    skill: stored_skill_from_row(row)?,
                    updated_at_unix: row.get::<_, i64>(7)? as u64,
                })
            })
            .map_err(sql_error)?;
        collect_rows(rows)
    }

    /// Searches active skills by name or body text.
    pub fn search_skills(&self, query: &str) -> PeriResult<Vec<StoredSkill>> {
        self.initialize()?;
        let pattern = format!("%{query}%");
        let connection = self.connection()?;
        let mut statement = connection
            .prepare(
                "SELECT name, body, last_used_at_unix, archived_at_unix, scope, \
                        description, pinned_at_unix \
                 FROM skills \
                 WHERE archived_at_unix = 0 AND (name LIKE ?1 OR body LIKE ?1) \
                 ORDER BY updated_at DESC, name ASC",
            )
            .map_err(sql_error)?;
        let rows = statement
            .query_map([pattern], stored_skill_from_row)
            .map_err(sql_error)?;
        collect_rows(rows)
    }

    /// Records that a skill body was just loaded into agent context. Drives
    /// Curator's last-used tracking — only `skill_view` should call this.
    pub fn mark_skill_viewed(&self, name: &str, at_unix: u64) -> PeriResult<bool> {
        self.initialize()?;
        let connection = self.connection()?;
        let changed = connection
            .execute(
                "UPDATE skills SET last_used_at_unix = ?1 WHERE name = ?2",
                params![at_unix as i64, name],
            )
            .map_err(sql_error)?;
        Ok(changed > 0)
    }

    /// Marks a skill archived (Curator's 90-day rule or operator command).
    /// Pass `at_unix = 0` to restore. Returns whether a row was changed.
    pub fn set_skill_archived(&self, name: &str, at_unix: u64) -> PeriResult<bool> {
        self.initialize()?;
        let connection = self.connection()?;
        let changed = connection
            .execute(
                "UPDATE skills SET archived_at_unix = ?1 WHERE name = ?2",
                params![at_unix as i64, name],
            )
            .map_err(sql_error)?;
        Ok(changed > 0)
    }

    /// Pin or unpin a skill so the Curator can't archive it. Pass
    /// `at_unix = 0` to unpin, any positive value to pin. Returns
    /// whether a row was changed (false if the skill doesn't exist).
    /// Pinning a skill that's already archived doesn't restore it —
    /// callers should call `set_skill_archived(.., 0)` separately.
    pub fn set_skill_pinned(&self, name: &str, at_unix: u64) -> PeriResult<bool> {
        self.initialize()?;
        let connection = self.connection()?;
        let changed = connection
            .execute(
                "UPDATE skills SET pinned_at_unix = ?1 WHERE name = ?2",
                params![at_unix as i64, name],
            )
            .map_err(sql_error)?;
        Ok(changed > 0)
    }

    /// Applies the 30/90-day Stale/Archive rules to every `scope='auto'`
    /// row. Returns one `(name, verdict)` pair per auto skill so the caller
    /// can render a report; bundled / community / empty-scope rows are
    /// skipped entirely. When `dry_run = false`, rows reaching
    /// `Archive` are persisted via `set_skill_archived(name, now_unix)`.
    pub fn apply_auto_rules(
        &self,
        now_unix: u64,
        dry_run: bool,
    ) -> PeriResult<Vec<(String, AutoRuleVerdict)>> {
        self.initialize()?;
        let records = self.list_skill_records()?;
        let mut decisions = Vec::with_capacity(records.len());
        for record in records {
            if record.skill.scope != "auto" {
                continue;
            }
            // Pinned skills are shielded from automated archival —
            // the operator explicitly told us "keep this one around."
            // They're still listed in the decision vec so callers can
            // surface "skipped: pinned" feedback, mirroring Hermes'
            // `curator --status` UX.
            if record.skill.pinned_at_unix > 0 {
                // Pinned → always-active by definition. We use the
                // existing `Active` verdict rather than introducing a
                // dedicated `Pinned` enum just so callers can report
                // "skipped, still pinned" without a schema bump.
                decisions.push((record.skill.name, AutoRuleVerdict::Active));
                continue;
            }
            let verdict = auto_rule_verdict(&record, now_unix);
            if matches!(verdict, AutoRuleVerdict::Archive) && !dry_run {
                self.set_skill_archived(&record.skill.name, now_unix)?;
            }
            decisions.push((record.skill.name, verdict));
        }
        Ok(decisions)
    }

    /// Reads a Curator metadata value. Returns `None` when unset.
    pub fn get_meta(&self, key: &str) -> PeriResult<Option<String>> {
        self.initialize()?;
        let connection = self.connection()?;
        let mut statement = connection
            .prepare("SELECT value FROM kv_meta WHERE key = ?1")
            .map_err(sql_error)?;
        let value = statement
            .query_row([key], |row| row.get::<_, String>(0))
            .optional()
            .map_err(sql_error)?;
        Ok(value)
    }

    /// Writes a Curator metadata value (e.g. `last_curator_run_unix`).
    pub fn set_meta(&self, key: &str, value: &str) -> PeriResult<()> {
        self.initialize()?;
        let connection = self.connection()?;
        connection
            .execute(
                r#"
                INSERT INTO kv_meta (key, value) VALUES (?1, ?2)
                ON CONFLICT(key) DO UPDATE SET value = excluded.value
                "#,
                params![key, value],
            )
            .map_err(sql_error)?;
        Ok(())
    }

    /// Most recent session-update unix timestamp across both session tables;
    /// `0` when no sessions exist. The Curator's 7-day idle trigger compares
    /// this with `last_curator_run_unix`.
    pub fn last_activity_unix(&self) -> PeriResult<u64> {
        self.initialize()?;
        let connection = self.connection()?;
        let from_records: i64 = connection
            .query_row(
                "SELECT COALESCE(MAX(updated_at_unix), 0) FROM session_records",
                [],
                |row| row.get(0),
            )
            .map_err(sql_error)?;
        let from_legacy: i64 = connection
            .query_row(
                "SELECT COALESCE(MAX(strftime('%s', updated_at)), 0) FROM sessions",
                [],
                |row| {
                    let raw: String = row.get(0).unwrap_or_default();
                    Ok(raw.parse::<i64>().unwrap_or(0))
                },
            )
            .map_err(sql_error)?;
        Ok(from_records.max(from_legacy).max(0) as u64)
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

    /// Persists a session's full tool-call sequence and increments the
    /// rolling n-gram counters that the cross-session reflection pass
    /// reads. Idempotent on `session_id`: a re-run replaces the prior
    /// sequence row and bumps the n-gram counts again — callers should
    /// avoid invoking this twice for the same session.
    ///
    /// `tools` is the ordered list of tool names from
    /// `summary.turns.iter().map(|t| t.tool_name.clone())`. Trivial
    /// sequences (length < min n-gram width) are still saved so the
    /// audit trail is complete, but they contribute no n-grams.
    ///
    /// `task_summary` is folded into every n-gram's `last_task_summary`
    /// so the reflection prompt sees what the user was actually trying
    /// to do, not just the tool names.
    pub fn save_tool_sequence(
        &self,
        session_id: &str,
        tools: &[String],
        task_summary: &str,
        max_ngram_length: u32,
        now_unix: u64,
    ) -> PeriResult<()> {
        self.initialize()?;
        let connection = self.connection()?;
        // Tool names are ASCII identifiers (`file_write`, `git_push`,
        // …) so a pipe-delimited line is a fine on-disk format. Avoids
        // pulling serde_json into the memory crate just for an audit
        // trail blob.
        let sequence_blob = tools.join("|");
        connection
            .execute(
                "INSERT INTO tool_sequences (session_id, sequence_json, task_summary, created_at_unix) \
                 VALUES (?1, ?2, ?3, ?4) \
                 ON CONFLICT(session_id) DO UPDATE SET \
                    sequence_json = excluded.sequence_json, \
                    task_summary = excluded.task_summary, \
                    created_at_unix = excluded.created_at_unix",
                rusqlite::params![
                    session_id,
                    sequence_blob,
                    task_summary,
                    now_unix as i64,
                ],
            )
            .map_err(sql_error)?;
        // Extract n-grams. Cap the per-session n-gram updates so a
        // pathologically long session can't blow up the table — we
        // sample evenly across the sequence above the cap.
        const MAX_NGRAM_UPDATES_PER_SESSION: usize = 50;
        let widths = 2..=max_ngram_length.max(2) as usize;
        let mut candidates: Vec<Vec<String>> = Vec::new();
        for width in widths {
            if width > tools.len() {
                continue;
            }
            for window in tools.windows(width) {
                // Skip trivial n-grams where every tool is the same
                // (e.g. file_read x 3 isn't a "pattern", it's just
                // exploration).
                let distinct = window
                    .iter()
                    .collect::<std::collections::HashSet<_>>()
                    .len();
                if distinct < 2 {
                    continue;
                }
                candidates.push(window.to_vec());
            }
        }
        // Sample down to the cap. Even stride preserves a representative
        // slice instead of biasing toward the head of the sequence.
        if candidates.len() > MAX_NGRAM_UPDATES_PER_SESSION {
            let stride = candidates.len() / MAX_NGRAM_UPDATES_PER_SESSION;
            candidates = candidates
                .into_iter()
                .step_by(stride.max(1))
                .take(MAX_NGRAM_UPDATES_PER_SESSION)
                .collect();
        }
        for window in candidates {
            let hash = ngram_hash(&window);
            let ngram_tools = window.join("|");
            let ngram_length = window.len() as i64;
            connection
                .execute(
                    "INSERT INTO tool_ngrams ( \
                        ngram_hash, ngram_tools, ngram_length, occurrence_count, \
                        last_seen_unix, last_session_id, last_task_summary \
                     ) VALUES (?1, ?2, ?3, 1, ?4, ?5, ?6) \
                     ON CONFLICT(ngram_hash) DO UPDATE SET \
                        occurrence_count = occurrence_count + 1, \
                        last_seen_unix   = excluded.last_seen_unix, \
                        last_session_id  = excluded.last_session_id, \
                        last_task_summary = excluded.last_task_summary",
                    rusqlite::params![
                        hash,
                        ngram_tools,
                        ngram_length,
                        now_unix as i64,
                        session_id,
                        task_summary,
                    ],
                )
                .map_err(sql_error)?;
        }
        Ok(())
    }

    /// Returns n-grams that have crossed the promotion threshold but
    /// have not yet been turned into a skill. Sorted by occurrence_count
    /// descending so the reflection pass tackles the most-used patterns
    /// first. Capped to keep the batch cost bounded.
    pub fn list_promotion_candidates(
        &self,
        min_count: u32,
        max_results: usize,
    ) -> PeriResult<Vec<ToolNgram>> {
        self.initialize()?;
        let connection = self.connection()?;
        let mut stmt = connection
            .prepare(
                "SELECT ngram_hash, ngram_tools, ngram_length, occurrence_count, \
                    last_seen_unix, promoted_at_unix, last_session_id, last_task_summary \
                 FROM tool_ngrams \
                 WHERE promoted_at_unix = 0 AND occurrence_count >= ?1 \
                 ORDER BY occurrence_count DESC, last_seen_unix DESC \
                 LIMIT ?2",
            )
            .map_err(sql_error)?;
        let mut rows = stmt
            .query(rusqlite::params![min_count as i64, max_results as i64])
            .map_err(sql_error)?;
        let mut out = Vec::new();
        while let Some(row) = rows.next().map_err(sql_error)? {
            let tools_blob: String = row.get(1).map_err(sql_error)?;
            let tools = tools_blob.split('|').map(str::to_string).collect();
            out.push(ToolNgram {
                hash: row.get(0).map_err(sql_error)?,
                tools,
                occurrence_count: row.get::<_, i64>(3).map_err(sql_error)? as u32,
                last_seen_unix: row.get::<_, i64>(4).map_err(sql_error)? as u64,
                promoted_at_unix: row.get::<_, i64>(5).map_err(sql_error)? as u64,
                last_session_id: row.get(6).map_err(sql_error)?,
                last_task_summary: row.get(7).map_err(sql_error)?,
            });
        }
        Ok(out)
    }

    /// Stamps `promoted_at_unix` on an n-gram so the reflection pass
    /// stops considering it for promotion. Returns true when a row was
    /// updated.
    pub fn mark_ngram_promoted(&self, hash: &str, at_unix: u64) -> PeriResult<bool> {
        self.initialize()?;
        let connection = self.connection()?;
        let updated = connection
            .execute(
                "UPDATE tool_ngrams SET promoted_at_unix = ?1 \
                 WHERE ngram_hash = ?2 AND promoted_at_unix = 0",
                rusqlite::params![at_unix as i64, hash],
            )
            .map_err(sql_error)?;
        Ok(updated > 0)
    }

    /// Returns the most recent N tool-call sequences (whole sessions),
    /// ordered newest first. Used by the harness-learning pass to
    /// decide whether the operator runs `git_commit` or `git_branch`
    /// often enough to justify flipping a default. Sessions older than
    /// `since_unix` are excluded.
    pub fn recent_tool_sequences(
        &self,
        limit: usize,
        since_unix: u64,
    ) -> PeriResult<Vec<Vec<String>>> {
        self.initialize()?;
        let connection = self.connection()?;
        let mut stmt = connection
            .prepare(
                "SELECT sequence_json FROM tool_sequences \
                 WHERE created_at_unix >= ?1 \
                 ORDER BY created_at_unix DESC \
                 LIMIT ?2",
            )
            .map_err(sql_error)?;
        let mut rows = stmt
            .query(rusqlite::params![since_unix as i64, limit as i64])
            .map_err(sql_error)?;
        let mut out: Vec<Vec<String>> = Vec::new();
        while let Some(row) = rows.next().map_err(sql_error)? {
            let blob: String = row.get(0).map_err(sql_error)?;
            let tools = blob
                .split('|')
                .filter(|tool| !tool.is_empty())
                .map(str::to_string)
                .collect();
            out.push(tools);
        }
        Ok(out)
    }

    /// Returns true when the harness-learning pass has already
    /// auto-adjusted this config field. Each field is adjusted at most
    /// once across the project's lifetime — afterwards the operator
    /// has the final word.
    pub fn was_field_auto_adjusted(&self, field: &str) -> PeriResult<bool> {
        self.initialize()?;
        let connection = self.connection()?;
        let exists: Option<i64> = connection
            .query_row(
                "SELECT 1 FROM harness_adjustments WHERE field = ?1 LIMIT 1",
                rusqlite::params![field],
                |row| row.get(0),
            )
            .optional()
            .map_err(sql_error)?;
        Ok(exists.is_some())
    }

    /// Records one harness-learning auto-adjustment so the pass
    /// remembers not to re-tune the same field next week.
    pub fn record_harness_adjustment(
        &self,
        field: &str,
        previous_value: &str,
        new_value: &str,
        signal: &str,
        at_unix: u64,
    ) -> PeriResult<()> {
        self.initialize()?;
        let connection = self.connection()?;
        connection
            .execute(
                "INSERT INTO harness_adjustments \
                    (field, previous_value, new_value, applied_at_unix, signal) \
                 VALUES (?1, ?2, ?3, ?4, ?5) \
                 ON CONFLICT(field) DO UPDATE SET \
                    previous_value = excluded.previous_value, \
                    new_value      = excluded.new_value, \
                    applied_at_unix = excluded.applied_at_unix, \
                    signal         = excluded.signal",
                rusqlite::params![field, previous_value, new_value, at_unix as i64, signal],
            )
            .map_err(sql_error)?;
        Ok(())
    }

    fn connection(&self) -> PeriResult<Connection> {
        let conn = Connection::open(&self.path).map_err(sql_error)?;
        // Concurrency hardening. A fresh connection is opened per operation
        // and the daemon serves multiple sessions (plus the occasional CLI
        // invocation) against the same memory.db. Under SQLite's default
        // rollback journal with no busy handler, a second writer fails
        // immediately with "database is locked" and readers block the writer.
        //   - busy_timeout: contended access waits up to 5s instead of erroring.
        //   - WAL: readers and a single writer proceed concurrently.
        //   - synchronous=NORMAL: safe with WAL and avoids an fsync per commit.
        conn.busy_timeout(std::time::Duration::from_secs(5))
            .map_err(sql_error)?;
        // journal_mode returns the resulting mode, so it must be read via a
        // query rather than pragma_update (which rejects result-returning
        // statements).
        let _: String = conn
            .query_row("PRAGMA journal_mode = WAL", [], |row| row.get(0))
            .map_err(sql_error)?;
        conn.pragma_update(None, "synchronous", "NORMAL")
            .map_err(sql_error)?;
        Ok(conn)
    }
}

/// Hash used as the primary key for `tool_ngrams`. Joins the tool
/// names with `|` and runs them through a SHA-256 to get a stable
/// 64-char hex id. We don't ship a hashing crate just for this — the
/// existing `rusqlite` brings `sqlite3` which we already trust, and
/// `serde_json` brings nothing relevant. Roll a small inline hash
/// using `DefaultHasher` (note: not cryptographically secure, just
/// stable across one rustc version; that's fine because the table
/// rebuilds gracefully on collision).
fn ngram_hash(tools: &[String]) -> String {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let mut hasher = DefaultHasher::new();
    for tool in tools {
        tool.hash(&mut hasher);
        // Separator so ["a","bc"] and ["ab","c"] hash differently.
        b'|'.hash(&mut hasher);
    }
    format!("{:016x}", hasher.finish())
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
/// Defense-in-depth guard: refuses to build a session path from an id that
/// could escape the sessions root. Callers (the daemon RPC surface) are
/// expected to validate ids at ingress, but these helpers are public and write
/// to / delete from the filesystem, so they re-check rather than trust.
fn reject_unsafe_session_id(id: &str) -> PeriResult<()> {
    if peridot_common::is_valid_session_id(id) {
        Ok(())
    } else {
        Err(PeriError::Tool(format!(
            "refusing unsafe session id: {id:?}"
        )))
    }
}

pub fn save_session_blob(
    sessions_root: &Path,
    id: &str,
    filename: &str,
    bytes: &[u8],
) -> PeriResult<()> {
    reject_unsafe_session_id(id)?;
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
    reject_unsafe_session_id(id)?;
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
    reject_unsafe_session_id(id)?;
    let target = sessions_root.join(id);
    if !target.exists() {
        return Ok(false);
    }
    fs::remove_dir_all(&target)
        .map_err(|err| PeriError::Tool(format!("failed to remove {}: {err}", target.display())))?;
    Ok(true)
}

fn stored_skill_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<StoredSkill> {
    // Column order is the same across every query that uses this
    // decoder. New fields go after the legacy ones so v0.8.10-and-older
    // dumps that still produce a 5-column projection (without
    // `description` / `pinned_at_unix`) keep deserialising — see the
    // `_legacy` decoder for that path.
    Ok(StoredSkill {
        name: row.get(0)?,
        body: row.get(1)?,
        last_used_at_unix: row.get::<_, i64>(2)? as u64,
        archived_at_unix: row.get::<_, i64>(3)? as u64,
        scope: row.get(4)?,
        description: row.get::<_, Option<String>>(5)?.unwrap_or_default(),
        pinned_at_unix: row.get::<_, i64>(6)? as u64,
    })
}

/// Adds a column to an existing table when it is missing. SQLite has no
/// `ADD COLUMN IF NOT EXISTS`, so this checks `PRAGMA table_info` first.
fn ensure_column(
    connection: &Connection,
    table: &str,
    column: &str,
    type_clause: &str,
) -> PeriResult<()> {
    let mut statement = connection
        .prepare(&format!("PRAGMA table_info({table})"))
        .map_err(sql_error)?;
    let existing: Vec<String> = statement
        .query_map([], |row| row.get::<_, String>(1))
        .map_err(sql_error)?
        .filter_map(Result::ok)
        .collect();
    if existing.iter().any(|name| name == column) {
        return Ok(());
    }
    connection
        .execute(
            &format!("ALTER TABLE {table} ADD COLUMN {column} {type_clause}"),
            [],
        )
        .map_err(sql_error)?;
    Ok(())
}

fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or_default()
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
    fn connection_enables_wal_and_busy_timeout() {
        let root = std::env::temp_dir().join(format!(
            "peridot-memory-wal-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let store = MemoryStore::new(root.join("memory.db"));
        store.initialize().unwrap();
        let conn = store.connection().unwrap();
        let mode: String = conn
            .query_row("PRAGMA journal_mode", [], |row| row.get(0))
            .unwrap();
        assert_eq!(mode.to_lowercase(), "wal");
        std::fs::remove_dir_all(&root).ok();
    }

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
    fn skill_metadata_round_trips_and_curator_helpers_work() {
        let root = std::env::temp_dir().join(format!(
            "peridot-memory-curator-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or_default()
        ));
        let store = MemoryStore::new(root.join("memory.db"));
        store
            .save_skill(&StoredSkill {
                name: "auto-fix-parser".to_string(),
                body: "Parser recipe".to_string(),
                scope: "auto".to_string(),
                ..Default::default()
            })
            .unwrap();
        store
            .save_skill(&StoredSkill {
                name: "bundled-fmt".to_string(),
                body: "Run cargo fmt".to_string(),
                ..Default::default()
            })
            .unwrap();

        assert!(store.mark_skill_viewed("auto-fix-parser", 1_000).unwrap());
        let records = store.list_skill_records().unwrap();
        let auto = records
            .iter()
            .find(|r| r.skill.name == "auto-fix-parser")
            .unwrap();
        assert_eq!(auto.skill.last_used_at_unix, 1_000);
        assert_eq!(auto.skill.scope, "auto");

        assert!(store.set_skill_archived("auto-fix-parser", 2_000).unwrap());
        let listed = store.list_skills().unwrap();
        assert!(listed.iter().all(|s| s.name != "auto-fix-parser"));
        assert_eq!(listed.len(), 1);
        assert_eq!(
            store
                .list_skill_records()
                .unwrap()
                .iter()
                .find(|r| r.skill.name == "auto-fix-parser")
                .map(|r| r.skill.archived_at_unix),
            Some(2_000)
        );

        store
            .set_meta("last_curator_run_unix", "1234567890")
            .unwrap();
        assert_eq!(
            store.get_meta("last_curator_run_unix").unwrap().as_deref(),
            Some("1234567890")
        );

        store
            .save_session_record(&SessionRecord {
                id: "session-recent".to_string(),
                summary: String::new(),
                status: SessionLifecycle::Done,
                created_at_unix: 9_999,
                updated_at_unix: 9_999,
                workspace_root: PathBuf::from("/tmp"),
                worktree_branch: None,
                last_task: None,
                total_tokens: 0,
                total_cost_usd: 0.0,
                turns_used: 0,
            })
            .unwrap();
        assert_eq!(store.last_activity_unix().unwrap(), 9_999);

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn skill_description_and_pin_roundtrip() {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "peridot-memory-skill-pin-{}-{nanos}",
            std::process::id()
        ));
        let store = MemoryStore::new(root.join(".peridot/memory.db"));
        store
            .save_skill(&StoredSkill {
                name: "ship-daily".into(),
                body: "# Ship Daily\nrun verify_build, then commit, then push.".into(),
                scope: "auto".into(),
                description: "Daily release checklist".into(),
                pinned_at_unix: 1_700_000_000,
                ..Default::default()
            })
            .unwrap();

        // List + read back: both new columns survive the round trip.
        let active = store.list_skills().unwrap();
        let stored = active.iter().find(|s| s.name == "ship-daily").unwrap();
        assert_eq!(stored.description, "Daily release checklist");
        assert_eq!(stored.pinned_at_unix, 1_700_000_000);

        // Pinned rows are protected from `apply_auto_rules` — verdict
        // is Active even past the 90-day archive threshold. We
        // anchor `now` to the live system clock plus a year so the
        // record's `updated_at_unix` (set by save_skill to the real
        // current time) is meaningfully older than our `now`.
        let real_now = unix_now();
        let now_far_future = real_now + 200 * 24 * 3600;
        let decisions = store.apply_auto_rules(now_far_future, false).unwrap();
        let verdict = decisions
            .iter()
            .find(|(n, _)| n == "ship-daily")
            .map(|(_, v)| *v)
            .expect("ship-daily decision present");
        assert_eq!(verdict, AutoRuleVerdict::Active, "pinned shouldn't archive");

        // Unpinning re-enables the rule path. Now 200 days have
        // elapsed since `updated_at_unix`, so we expect Archive
        // (>90 days threshold).
        store.set_skill_pinned("ship-daily", 0).unwrap();
        let after = store.apply_auto_rules(now_far_future, true).unwrap();
        let verdict_after = after
            .iter()
            .find(|(n, _)| n == "ship-daily")
            .map(|(_, v)| *v)
            .unwrap();
        assert!(
            matches!(
                verdict_after,
                AutoRuleVerdict::Stale | AutoRuleVerdict::Archive
            ),
            "unpinned, 200-days-stale skill must now be Stale or Archive, got {verdict_after:?}"
        );

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn auto_rule_verdict_distinguishes_active_stale_archive() {
        let record = |last_used: u64, updated: u64| SkillRecord {
            skill: StoredSkill {
                name: "x".into(),
                body: "y".into(),
                last_used_at_unix: last_used,
                archived_at_unix: 0,
                scope: "auto".into(),
                description: String::new(),
                pinned_at_unix: 0,
            },
            updated_at_unix: updated,
        };
        let now: u64 = 100_000_000;
        assert_eq!(
            auto_rule_verdict(&record(now - 1, now - 1), now),
            AutoRuleVerdict::Active
        );
        assert_eq!(
            auto_rule_verdict(&record(0, now - 31 * 24 * 3600), now),
            AutoRuleVerdict::Stale,
            "31-day-old never-used skill reads stale from updated_at_unix"
        );
        assert_eq!(
            auto_rule_verdict(&record(0, now - 95 * 24 * 3600), now),
            AutoRuleVerdict::Archive,
        );
        // Already-archived rows are never re-archived.
        let mut archived = record(0, now - 95 * 24 * 3600);
        archived.skill.archived_at_unix = now - 1;
        assert_eq!(auto_rule_verdict(&archived, now), AutoRuleVerdict::Active);
    }

    #[test]
    fn apply_auto_rules_archives_only_old_auto_skills() {
        let root = std::env::temp_dir().join(format!(
            "peridot-memory-rules-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or_default()
        ));
        let store = MemoryStore::new(root.join("memory.db"));
        let now: u64 = 100_000_000_000; // ~year 5138; safely in the future of any real save_skill timestamp
        // Auto skill, never re-used after save — should archive.
        store
            .save_skill(&StoredSkill {
                name: "old-auto".into(),
                body: "old".into(),
                last_used_at_unix: 1,
                scope: "auto".into(),
                ..Default::default()
            })
            .unwrap();
        // Auto skill, last_used pinned right at `now` — should stay active.
        store
            .save_skill(&StoredSkill {
                name: "fresh-auto".into(),
                body: "fresh".into(),
                last_used_at_unix: now,
                scope: "auto".into(),
                ..Default::default()
            })
            .unwrap();
        // Bundled (empty scope) — never touched by Curator even if ancient.
        store
            .save_skill(&StoredSkill {
                name: "bundled-old".into(),
                body: "shipped".into(),
                last_used_at_unix: 1,
                ..Default::default()
            })
            .unwrap();
        let decisions = store.apply_auto_rules(now, false).unwrap();
        let by_name: std::collections::HashMap<_, _> = decisions.into_iter().collect();
        assert_eq!(by_name.get("old-auto"), Some(&AutoRuleVerdict::Archive));
        assert_eq!(by_name.get("fresh-auto"), Some(&AutoRuleVerdict::Active));
        // bundled-old is filtered out entirely (not just Active).
        assert!(!by_name.contains_key("bundled-old"));
        // old-auto must actually be archived in storage.
        let listed = store.list_skills().unwrap();
        assert!(listed.iter().all(|s| s.name != "old-auto"));
        assert!(listed.iter().any(|s| s.name == "bundled-old"));
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
                ..Default::default()
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

    fn fresh_store(label: &str) -> (PathBuf, MemoryStore) {
        let root = std::env::temp_dir().join(format!(
            "peridot-memory-ngram-{label}-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let store = MemoryStore::new(root.join("memory.db"));
        store.initialize().unwrap();
        (root, store)
    }

    fn tools(names: &[&str]) -> Vec<String> {
        names.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn save_tool_sequence_counts_distinct_bigrams_and_trigrams() {
        let (root, store) = fresh_store("counts");
        store
            .save_tool_sequence(
                "sess-1",
                &tools(&["file_read", "verify_build", "git_commit"]),
                "ship the change",
                3,
                1_700_000_000,
            )
            .unwrap();

        // bigrams: (file_read, verify_build), (verify_build, git_commit)
        // trigrams: (file_read, verify_build, git_commit)
        // All distinct so all survive the distinct-tool filter.
        let candidates = store.list_promotion_candidates(1, 10).unwrap();
        assert_eq!(candidates.len(), 3);
        assert!(candidates.iter().all(|c| c.occurrence_count == 1));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn save_tool_sequence_skips_self_repeats() {
        let (root, store) = fresh_store("skips");
        // file_read x 4 = three (file_read, file_read) bigrams — all
        // single-distinct-tool, must be filtered.
        store
            .save_tool_sequence(
                "sess-skip",
                &tools(&["file_read", "file_read", "file_read", "file_read"]),
                "explore",
                3,
                1_700_000_000,
            )
            .unwrap();
        let candidates = store.list_promotion_candidates(1, 10).unwrap();
        assert!(
            candidates.is_empty(),
            "self-repeat ngrams should be filtered, got {candidates:?}"
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn repeated_pattern_across_sessions_accumulates() {
        let (root, store) = fresh_store("accumulate");
        let seq = tools(&["verify_build", "git_commit", "git_push"]);
        for i in 0..5 {
            store
                .save_tool_sequence(
                    &format!("sess-{i}"),
                    &seq,
                    "ship daily",
                    3,
                    1_700_000_000 + i as u64,
                )
                .unwrap();
        }
        // The trigram (verify_build, git_commit, git_push) and both of
        // its constituent bigrams should now show occurrence_count = 5.
        let candidates = store.list_promotion_candidates(5, 10).unwrap();
        assert!(!candidates.is_empty());
        assert!(candidates.iter().all(|c| c.occurrence_count == 5));
        assert!(
            candidates.iter().any(|c| c.tools.len() == 3),
            "the full trigram should be a candidate"
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn mark_ngram_promoted_excludes_from_future_candidates() {
        let (root, store) = fresh_store("promoted");
        let seq = tools(&["verify_test", "git_commit"]);
        for i in 0..3 {
            store
                .save_tool_sequence(
                    &format!("sess-{i}"),
                    &seq,
                    "test then commit",
                    3,
                    1_700_000_000 + i as u64,
                )
                .unwrap();
        }
        let before = store.list_promotion_candidates(3, 10).unwrap();
        assert_eq!(before.len(), 1);
        let hash = before[0].hash.clone();

        let updated = store.mark_ngram_promoted(&hash, 1_700_001_000).unwrap();
        assert!(updated);

        let after = store.list_promotion_candidates(3, 10).unwrap();
        assert!(after.is_empty(), "promoted ngram should be excluded");
        // Re-marking is a no-op (returns false).
        let again = store.mark_ngram_promoted(&hash, 1_700_002_000).unwrap();
        assert!(!again);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn min_count_filter_excludes_one_off_patterns() {
        let (root, store) = fresh_store("min_count");
        store
            .save_tool_sequence(
                "sess-once",
                &tools(&["plan_create", "shell_exec"]),
                "draft",
                3,
                1_700_000_000,
            )
            .unwrap();
        // Threshold 5 should exclude the one-off bigram.
        assert!(store.list_promotion_candidates(5, 10).unwrap().is_empty());
        // Threshold 1 includes it.
        assert_eq!(store.list_promotion_candidates(1, 10).unwrap().len(), 1);
        fs::remove_dir_all(root).unwrap();
    }
}
