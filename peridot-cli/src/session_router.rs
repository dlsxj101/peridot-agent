//! Multi-session orchestration scaffolding.
//!
//! The router owns one [`SessionHandle`] per concurrent agent run, multiplexes
//! `TuiRuntimeEvent`s from each session's mpsc channel into the TUI, and tracks
//! a per-process "foreground" session that the TUI is currently focused on.
//!
//! `main.rs` registers an initial foreground session at startup and routes
//! every `/session new|switch|close`, `/fork`, `/teammate`, `/worktree` slash
//! intent (carried as [`peridot_tui::SessionCommandEvent`]) through the router.
//! Worktree isolation and persistence round-trip land in milestones M2/M3 of
//! the multi-session runbook.
//!
//! `SessionTotals`, `ActiveSession`, `to_record`, and a couple of router
//! accessors live on the data model now but are exercised by milestones M2–M5
//! (worktree isolation, persistence round-trip, subagent fan-in, attention
//! notifier). They sit behind `#[allow(dead_code)]` until those milestones
//! land so we don't churn imports on every PR.

#![allow(dead_code)]

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use peridot_core::CancelToken;
use peridot_memory::{SessionLifecycle, SessionRecord};

/// Where an agent loop should perform its tool calls.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub enum WorkspaceIsolation {
    /// Same workspace as the parent (default for the foreground session).
    #[default]
    Shared,
    /// Use a dedicated git worktree on the named branch.
    Worktree {
        /// Branch name.
        branch: String,
    },
    /// Reserved for a future subprocess sandbox.
    Subprocess,
}

/// All resources owned by one in-flight agent session.
#[derive(Debug)]
pub struct SessionHandle {
    /// Stable session id (matches the on-disk SessionRecord id).
    pub id: String,
    /// Parent session id if this session was spawned from another (Phase 15).
    pub parent_id: Option<String>,
    /// Cancellation handle, fired by Esc / `/session close`.
    pub cancel: CancelToken,
    /// Workspace isolation policy used when the session started.
    pub isolation: WorkspaceIsolation,
    /// Latest lifecycle stage observed by the router.
    pub lifecycle: SessionLifecycle,
    /// Workspace root the agent loop was launched against.
    pub workspace_root: PathBuf,
    /// Optional worktree branch for `WorkspaceIsolation::Worktree`.
    pub worktree_branch: Option<String>,
    /// Wall-clock spawn time (unix seconds).
    pub started_at_unix: u64,
}

impl SessionHandle {
    /// Builds a fresh handle in `Idle` state with `started_at_unix` set to now.
    pub fn new(
        id: impl Into<String>,
        workspace_root: impl Into<PathBuf>,
        isolation: WorkspaceIsolation,
    ) -> Self {
        let id = id.into();
        let worktree_branch = if let WorkspaceIsolation::Worktree { branch } = &isolation {
            Some(branch.clone())
        } else {
            None
        };
        Self {
            id,
            parent_id: None,
            cancel: CancelToken::new(),
            isolation,
            lifecycle: SessionLifecycle::Idle,
            workspace_root: workspace_root.into(),
            worktree_branch,
            started_at_unix: now_unix(),
        }
    }

    /// Returns a [`SessionRecord`] reflecting the handle's current state.
    pub fn to_record(&self, summary: &str, totals: SessionTotals) -> SessionRecord {
        let now = now_unix();
        SessionRecord {
            id: self.id.clone(),
            summary: summary.to_string(),
            status: self.lifecycle,
            created_at_unix: self.started_at_unix,
            updated_at_unix: now,
            workspace_root: self.workspace_root.clone(),
            worktree_branch: self.worktree_branch.clone(),
            last_task: totals.last_task,
            total_tokens: totals.total_tokens,
            total_cost_usd: totals.total_cost_usd,
            turns_used: totals.turns_used,
        }
    }
}

/// Aggregated per-session counters that live on the TUI side and are folded
/// into a `SessionRecord` when persisting.
#[derive(Clone, Debug, Default)]
pub struct SessionTotals {
    /// Last submitted task text.
    pub last_task: Option<String>,
    /// Total provider tokens used.
    pub total_tokens: u64,
    /// Total estimated cost (USD).
    pub total_cost_usd: f64,
    /// Turns consumed so far.
    pub turns_used: u32,
}

/// Router that owns all live session handles and the foreground pointer.
///
/// The current iteration is single-threaded and synchronous; concurrent agent
/// task spawning + event multiplexing is wired in PR10 once the TUI tab UI
/// is in place.
#[derive(Debug, Default)]
pub struct SessionRouter {
    sessions: HashMap<String, SessionHandle>,
    order: Vec<String>,
    foreground: Option<String>,
}

impl SessionRouter {
    /// Creates an empty router.
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers a session handle. If no foreground is set, it becomes the foreground.
    pub fn register(&mut self, handle: SessionHandle) {
        let id = handle.id.clone();
        self.order.retain(|existing| existing != &id);
        self.order.push(id.clone());
        self.sessions.insert(id.clone(), handle);
        if self.foreground.is_none() {
            self.foreground = Some(id);
        }
    }

    /// Returns true when the router holds zero sessions.
    pub fn is_empty(&self) -> bool {
        self.sessions.is_empty()
    }

    /// Returns the number of registered sessions.
    pub fn len(&self) -> usize {
        self.sessions.len()
    }

    /// Returns the id of the foreground session, when one is set.
    pub fn foreground(&self) -> Option<&str> {
        self.foreground.as_deref()
    }

    /// Returns an immutable reference to a session handle.
    pub fn get(&self, id: &str) -> Option<&SessionHandle> {
        self.sessions.get(id)
    }

    /// Returns a mutable reference to a session handle.
    pub fn get_mut(&mut self, id: &str) -> Option<&mut SessionHandle> {
        self.sessions.get_mut(id)
    }

    /// Iterates over sessions in registration order.
    pub fn iter(&self) -> impl Iterator<Item = &SessionHandle> + '_ {
        self.order.iter().filter_map(|id| self.sessions.get(id))
    }

    /// Switches the foreground pointer to `id` if it exists.
    pub fn switch_to(&mut self, id: &str) -> bool {
        if self.sessions.contains_key(id) {
            self.foreground = Some(id.to_string());
            true
        } else {
            false
        }
    }

    /// Cycles the foreground session in registration order. Returns the new
    /// foreground id, or None when the router is empty.
    pub fn cycle_foreground(&mut self) -> Option<&str> {
        if self.order.is_empty() {
            return None;
        }
        let next_index = match self.foreground.as_ref() {
            Some(current) => self
                .order
                .iter()
                .position(|id| id == current)
                .map(|pos| (pos + 1) % self.order.len())
                .unwrap_or(0),
            None => 0,
        };
        let id = self.order[next_index].clone();
        self.foreground = Some(id);
        self.foreground.as_deref()
    }

    /// Cancels and removes a session. Returns true when something was removed.
    pub fn close(&mut self, id: &str) -> bool {
        let removed = self.sessions.remove(id).is_some();
        if removed {
            self.order.retain(|existing| existing != id);
            if self.foreground.as_deref() == Some(id) {
                self.foreground = self.order.first().cloned();
            }
        }
        removed
    }
}

/// Thread-safe handle that callbacks (on_submit / on_interrupt) share to fire
/// cancellation on the active foreground session.
#[derive(Clone, Default)]
pub struct ActiveSession {
    inner: Arc<Mutex<Option<CancelToken>>>,
}

impl ActiveSession {
    /// Returns an empty active-session handle.
    pub fn new() -> Self {
        Self::default()
    }

    /// Replaces the stored token (call this when a new agent run starts).
    pub fn set(&self, token: CancelToken) {
        *self.inner.lock().unwrap() = Some(token);
    }

    /// Takes the current token, leaving the handle empty. Useful when the
    /// caller wants to consume the token before firing cancel.
    pub fn take(&self) -> Option<CancelToken> {
        self.inner.lock().unwrap().take()
    }

    /// Fires `cancel()` on the active token if one is set. Returns true when
    /// a token was held.
    pub fn cancel(&self) -> bool {
        if let Some(token) = self.take() {
            token.cancel();
            true
        } else {
            false
        }
    }
}

fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn handle(id: &str) -> SessionHandle {
        SessionHandle::new(id, PathBuf::from("/tmp/work"), WorkspaceIsolation::Shared)
    }

    #[test]
    fn register_assigns_foreground_when_empty() {
        let mut router = SessionRouter::new();
        assert!(router.is_empty());
        router.register(handle("s1"));
        assert_eq!(router.foreground(), Some("s1"));
        assert_eq!(router.len(), 1);
    }

    #[test]
    fn cycle_foreground_rotates_through_sessions() {
        let mut router = SessionRouter::new();
        router.register(handle("s1"));
        router.register(handle("s2"));
        router.register(handle("s3"));
        assert_eq!(router.foreground(), Some("s1"));
        assert_eq!(router.cycle_foreground(), Some("s2"));
        assert_eq!(router.cycle_foreground(), Some("s3"));
        assert_eq!(router.cycle_foreground(), Some("s1"));
    }

    #[test]
    fn close_picks_next_foreground() {
        let mut router = SessionRouter::new();
        router.register(handle("s1"));
        router.register(handle("s2"));
        assert!(router.switch_to("s2"));
        assert_eq!(router.foreground(), Some("s2"));
        assert!(router.close("s2"));
        assert_eq!(router.foreground(), Some("s1"));
        assert!(!router.close("missing"));
    }

    #[test]
    fn worktree_isolation_records_branch_on_handle() {
        let handle = SessionHandle::new(
            "wt1",
            PathBuf::from("/tmp/wt1"),
            WorkspaceIsolation::Worktree {
                branch: "peridot/teammate-wt1".to_string(),
            },
        );
        assert_eq!(
            handle.worktree_branch.as_deref(),
            Some("peridot/teammate-wt1")
        );
        assert_eq!(handle.workspace_root, PathBuf::from("/tmp/wt1"));
        assert!(matches!(
            handle.isolation,
            WorkspaceIsolation::Worktree { .. }
        ));
    }

    #[test]
    fn handle_to_record_carries_workspace_and_totals() {
        let mut handle = handle("s1");
        handle.lifecycle = SessionLifecycle::Running;
        handle.parent_id = Some("parent".to_string());
        let totals = SessionTotals {
            last_task: Some("rewrite README".to_string()),
            total_tokens: 1200,
            total_cost_usd: 0.07,
            turns_used: 4,
        };
        let record = handle.to_record("drafted plan", totals);
        assert_eq!(record.id, "s1");
        assert_eq!(record.status, SessionLifecycle::Running);
        assert_eq!(record.workspace_root, PathBuf::from("/tmp/work"));
        assert_eq!(record.last_task.as_deref(), Some("rewrite README"));
        assert_eq!(record.total_tokens, 1200);
        assert_eq!(record.turns_used, 4);
    }

    #[test]
    fn active_session_fires_cancel_once() {
        let active = ActiveSession::new();
        assert!(!active.cancel(), "no token set yet");
        let token = CancelToken::new();
        active.set(token.clone());
        assert!(active.cancel());
        assert!(token.is_cancelled());
        assert!(!active.cancel(), "second cancel should noop");
    }
}
