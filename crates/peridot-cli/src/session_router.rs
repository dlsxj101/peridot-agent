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

use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use peridot_common::PeriResult;
use peridot_core::CancelToken;
use peridot_memory::{SessionLifecycle, SessionRecord};
use peridot_tools::{AgentMessageBus, InboxMessage};

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
    /// Force-compaction flag the TUI sets via `/compact`. The agent
    /// loop swaps it to `false` on consumption, so the slash command
    /// is fire-and-forget and never double-fires.
    pub compact_request: Arc<AtomicBool>,
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
    /// FIFO queue of messages destined for this session, populated by
    /// `RouterMessageBus::send_to_parent` / `send_to_child` from peer
    /// sessions. The agent loop drains this at the start of every turn
    /// and folds each entry into context as a `PlanReminder`. `Mutex`
    /// is sufficient (no async work happens while holding it).
    pub inbox: Arc<Mutex<VecDeque<InboxMessage>>>,
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
            compact_request: Arc::new(AtomicBool::new(false)),
            isolation,
            lifecycle: SessionLifecycle::Idle,
            workspace_root: workspace_root.into(),
            worktree_branch,
            started_at_unix: now_unix(),
            inbox: Arc::new(Mutex::new(VecDeque::new())),
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
            goal_status: None,
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

    /// Returns the parent session id of `id`, if any. Used by
    /// `RouterMessageBus::send_to_parent` to resolve the destination.
    pub fn parent_of(&self, id: &str) -> Option<String> {
        self.sessions.get(id).and_then(|h| h.parent_id.clone())
    }

    /// Returns true when `parent_id` is the registered parent of `child_id`.
    /// Used as a safety check before `send_to_child` so a session can only
    /// message its own direct children, never arbitrary peers.
    pub fn is_child_of(&self, parent_id: &str, child_id: &str) -> bool {
        self.sessions
            .get(child_id)
            .and_then(|h| h.parent_id.as_deref())
            == Some(parent_id)
    }

    /// Returns a clone of the inbox `Arc<Mutex<...>>` for `id`, so a peer
    /// session can push messages without holding a `&mut SessionRouter`.
    pub fn inbox_handle(&self, id: &str) -> Option<Arc<Mutex<VecDeque<InboxMessage>>>> {
        self.sessions.get(id).map(|h| h.inbox.clone())
    }
}

/// `AgentMessageBus` implementation backed by a shared [`SessionRouter`].
/// Carries a single session id (the "current" session — i.e. whose harness
/// instantiated the bus) so the tool can identify itself without consulting
/// the router. Per-spawn `with_current_session` creates a new bus that
/// shares the underlying router but reports a different `current_session_id`.
#[derive(Clone)]
pub struct RouterMessageBus {
    router: Arc<Mutex<SessionRouter>>,
    current_session_id: Option<String>,
}

impl RouterMessageBus {
    /// Creates a bus that delegates to `router`. The `current_session_id`
    /// is initially unset; call `with_current_session` to bind one before
    /// passing the bus into a harness `ToolContext`.
    pub fn new(router: Arc<Mutex<SessionRouter>>) -> Self {
        Self {
            router,
            current_session_id: None,
        }
    }

    /// Returns a clone of this bus that reports `id` as the calling session.
    pub fn with_current_session(&self, id: impl Into<String>) -> Self {
        Self {
            router: self.router.clone(),
            current_session_id: Some(id.into()),
        }
    }

    fn push_to(&self, target: &str, body: InboxMessage) -> PeriResult<()> {
        let router = self
            .router
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let Some(inbox) = router.inbox_handle(target) else {
            return Err(peridot_common::PeriError::Tool(format!(
                "agent_message: no session named `{target}` to deliver to"
            )));
        };
        drop(router);
        inbox
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push_back(body);
        Ok(())
    }
}

#[async_trait]
impl AgentMessageBus for RouterMessageBus {
    fn current_session_id(&self) -> Option<String> {
        self.current_session_id.clone()
    }

    async fn send_to_parent(&self, from_session: &str, message: &str) -> PeriResult<String> {
        let parent_id = {
            let router = self
                .router
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            router.parent_of(from_session)
        };
        let parent_id = parent_id.ok_or_else(|| {
            peridot_common::PeriError::Tool(format!(
                "agent_message: session `{from_session}` has no parent to message"
            ))
        })?;
        self.push_to(
            &parent_id,
            InboxMessage {
                from: from_session.to_string(),
                body: message.to_string(),
                at_unix: now_unix(),
            },
        )?;
        Ok(parent_id)
    }

    async fn send_to_child(
        &self,
        from_session: &str,
        child_session: &str,
        message: &str,
    ) -> PeriResult<()> {
        let is_child = {
            let router = self
                .router
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            router.is_child_of(from_session, child_session)
        };
        if !is_child {
            return Err(peridot_common::PeriError::Tool(format!(
                "agent_message: `{child_session}` is not a registered child of `{from_session}`"
            )));
        }
        self.push_to(
            child_session,
            InboxMessage {
                from: from_session.to_string(),
                body: message.to_string(),
                at_unix: now_unix(),
            },
        )
    }

    async fn drain_inbox(&self, session: &str) -> Vec<InboxMessage> {
        let inbox = {
            let router = self
                .router
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            router.inbox_handle(session)
        };
        let Some(inbox) = inbox else {
            return Vec::new();
        };
        let mut guard = inbox
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        guard.drain(..).collect()
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

    #[tokio::test]
    async fn router_message_bus_routes_parent_and_child() {
        let mut router = SessionRouter::new();
        let mut parent = handle("parent");
        let mut child = handle("child");
        child.parent_id = Some("parent".to_string());
        parent.parent_id = None;
        router.register(parent);
        router.register(child);

        let shared = Arc::new(Mutex::new(router));
        let bus_child = RouterMessageBus::new(shared.clone()).with_current_session("child");
        let bus_parent = RouterMessageBus::new(shared.clone()).with_current_session("parent");

        // child → parent should push to parent's inbox.
        let resolved_parent = bus_child.send_to_parent("child", "hello").await.unwrap();
        assert_eq!(resolved_parent, "parent");
        let parent_inbox = bus_parent.drain_inbox("parent").await;
        assert_eq!(parent_inbox.len(), 1);
        assert_eq!(parent_inbox[0].from, "child");
        assert_eq!(parent_inbox[0].body, "hello");

        // parent → child should push to child's inbox.
        bus_parent
            .send_to_child("parent", "child", "stop")
            .await
            .unwrap();
        let child_inbox = bus_child.drain_inbox("child").await;
        assert_eq!(child_inbox.len(), 1);
        assert_eq!(child_inbox[0].from, "parent");
        assert_eq!(child_inbox[0].body, "stop");

        // second drain returns empty (FIFO + consumes).
        assert!(bus_parent.drain_inbox("parent").await.is_empty());
    }

    #[tokio::test]
    async fn router_message_bus_rejects_sibling_addressing() {
        let mut router = SessionRouter::new();
        let mut parent = handle("p");
        parent.parent_id = None;
        let mut sib1 = handle("s1");
        sib1.parent_id = Some("p".to_string());
        let mut sib2 = handle("s2");
        sib2.parent_id = Some("p".to_string());
        router.register(parent);
        router.register(sib1);
        router.register(sib2);
        let shared = Arc::new(Mutex::new(router));
        let bus = RouterMessageBus::new(shared).with_current_session("s1");
        // s1 cannot address s2 directly — s1's parent_id is "p", not "s1".
        let err = bus.send_to_child("s1", "s2", "hi").await;
        assert!(err.is_err());
    }

    #[tokio::test]
    async fn router_message_bus_send_to_parent_errors_for_root() {
        let mut router = SessionRouter::new();
        router.register(handle("root"));
        let shared = Arc::new(Mutex::new(router));
        let bus = RouterMessageBus::new(shared).with_current_session("root");
        let err = bus.send_to_parent("root", "where are you").await;
        assert!(err.is_err());
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
