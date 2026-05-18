//! Multi-session TUI scaffolding: per-session directory entry, tab bar render,
//! and the keyboard plumbing that switches focus.
//!
//! The data here is intentionally additive: existing single-session callers
//! see an empty `sessions` vector and continue to render exactly as before.
//! Live integration with the `SessionRouter` (peridot-cli) lands in a follow-up
//! PR; this module ships the types, render helpers, and tests so the TUI
//! surface is ready for that wiring.

use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use unicode_width::UnicodeWidthStr;

use crate::state::{AgentRunStatus, TuiState};

/// One session as displayed in the tab bar / picker.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct SessionDirectoryItem {
    /// Stable session id.
    pub id: String,
    /// Short display title (≤50 chars suggested).
    pub title: String,
    /// Latest known agent run status.
    pub status: AgentRunStatus,
    /// Total provider tokens used.
    pub tokens: u64,
    /// Estimated cost in USD.
    pub cost_usd: f64,
    /// Unix seconds of the last observed event.
    pub last_event_at_unix: u64,
    /// Whether the session has an approval / ask_user prompt awaiting the user.
    pub pending_attention: bool,
    /// Parent session id when this entry was spawned as a subagent
    /// (`/fork`, `/teammate`, `/worktree`). `None` means the session is a
    /// top-level user session.
    #[serde(default)]
    pub parent_id: Option<String>,
    /// Kind label used to distinguish fork / teammate / worktree subagents in
    /// the side panel tree. `None` for top-level sessions.
    #[serde(default)]
    pub kind: Option<String>,
}

impl SessionDirectoryItem {
    /// Builds a fresh directory item in idle state.
    pub fn new(id: impl Into<String>, title: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            title: title.into(),
            status: AgentRunStatus::Idle,
            tokens: 0,
            cost_usd: 0.0,
            last_event_at_unix: 0,
            pending_attention: false,
            parent_id: None,
            kind: None,
        }
    }

    /// Marks this entry as a subagent of `parent_id` with the given kind label
    /// (e.g. "fork", "teammate", "worktree").
    pub fn with_parent(mut self, parent_id: impl Into<String>, kind: impl Into<String>) -> Self {
        self.parent_id = Some(parent_id.into());
        self.kind = Some(kind.into());
        self
    }
}

/// Returns the index of the foreground session inside `sessions` (if any).
pub fn foreground_index(state: &TuiState) -> Option<usize> {
    state
        .sessions
        .iter()
        .position(|item| item.id == state.current_session_id)
}

/// Rotates the foreground pointer to the next session. Returns the new id, if
/// any. Wraps to the first session at the end of the list.
pub fn cycle_foreground(state: &mut TuiState) -> Option<String> {
    if state.sessions.is_empty() {
        return None;
    }
    let next_index = match foreground_index(state) {
        Some(current) => (current + 1) % state.sessions.len(),
        None => 0,
    };
    state.current_session_id = state.sessions[next_index].id.clone();
    Some(state.current_session_id.clone())
}

/// Builds the styled tab-bar line shown directly under the header.
/// Returns an empty Line when zero sessions are registered.
pub fn render_tab_bar(state: &TuiState) -> Line<'static> {
    if state.sessions.is_empty() {
        return Line::from("");
    }
    let mut spans = Vec::with_capacity(state.sessions.len() * 2);
    for (index, item) in state.sessions.iter().enumerate() {
        if index > 0 {
            spans.push(Span::raw("  "));
        }
        let is_active = item.id == state.current_session_id;
        let label = if item.title.width() > 24 {
            let mut w = 0;
            let truncated: String = item
                .title
                .chars()
                .take_while(|ch| {
                    w += unicode_width::UnicodeWidthChar::width(*ch).unwrap_or(0);
                    w <= 23
                })
                .collect();
            format!("{truncated}…")
        } else {
            item.title.clone()
        };
        let badge = if item.pending_attention { " ◉" } else { "" };
        let text = format!("{label}{badge}");
        let mut style = Style::default().fg(Color::Gray);
        if is_active {
            style = style
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD | Modifier::UNDERLINED);
        } else if item.pending_attention {
            style = style.fg(Color::Yellow).add_modifier(Modifier::BOLD);
        }
        spans.push(Span::styled(text, style));
    }
    Line::from(spans)
}

/// One-line plain-text summary used by the text snapshot path.
pub fn render_tab_bar_text(state: &TuiState) -> String {
    if state.sessions.is_empty() {
        return String::new();
    }
    state
        .sessions
        .iter()
        .map(|item| {
            let marker = if item.id == state.current_session_id {
                "*"
            } else {
                "·"
            };
            let attention = if item.pending_attention { "!" } else { "" };
            format!("{marker} {}{attention}", item.title)
        })
        .collect::<Vec<_>>()
        .join("  ")
}

/// Computes how many cells the tab bar would consume. Used by the layout to
/// decide whether to reserve a row for it.
pub fn tab_bar_height(state: &TuiState) -> u16 {
    if state.sessions.is_empty() { 0 } else { 1 }
}

/// Convenience wrapper used by the eventual SessionRouter integration: trims
/// the directory to a maximum size, dropping oldest items.
pub fn trim_directory(items: &mut Vec<SessionDirectoryItem>, max: usize) {
    if items.len() > max {
        let overflow = items.len() - max;
        items.drain(0..overflow);
    }
}

/// Returns a Rect that the tab bar should be drawn into, given the area
/// directly below the header (returns a zero-height Rect when not needed).
#[allow(dead_code)]
pub fn tab_bar_rect(area: Rect, state: &TuiState) -> Rect {
    Rect {
        x: area.x,
        y: area.y,
        width: area.width,
        height: tab_bar_height(state),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::HeaderState;
    use peridot_common::{ExecutionMode, PermissionMode};

    fn fixture() -> TuiState {
        TuiState::new(HeaderState::new(
            ExecutionMode::Execute,
            PermissionMode::Auto,
            "mock",
        ))
    }

    #[test]
    fn empty_directory_yields_zero_height_and_empty_text() {
        let state = fixture();
        assert_eq!(tab_bar_height(&state), 0);
        assert!(render_tab_bar_text(&state).is_empty());
    }

    #[test]
    fn cycle_foreground_wraps_around_sessions() {
        let mut state = fixture();
        state.sessions = vec![
            SessionDirectoryItem::new("s1", "first"),
            SessionDirectoryItem::new("s2", "second"),
            SessionDirectoryItem::new("s3", "third"),
        ];
        state.current_session_id = "s1".to_string();
        assert_eq!(cycle_foreground(&mut state).as_deref(), Some("s2"));
        assert_eq!(cycle_foreground(&mut state).as_deref(), Some("s3"));
        assert_eq!(cycle_foreground(&mut state).as_deref(), Some("s1"));
    }

    #[test]
    fn tab_bar_text_marks_active_and_pending_attention() {
        let mut state = fixture();
        state.sessions = vec![SessionDirectoryItem::new("s1", "first"), {
            let mut item = SessionDirectoryItem::new("s2", "needs you");
            item.pending_attention = true;
            item
        }];
        state.current_session_id = "s2".to_string();
        let text = render_tab_bar_text(&state);
        assert!(text.contains("· first"));
        assert!(text.contains("* needs you!"));
    }

    #[test]
    fn trim_directory_drops_oldest_entries() {
        let mut items = vec![
            SessionDirectoryItem::new("a", "a"),
            SessionDirectoryItem::new("b", "b"),
            SessionDirectoryItem::new("c", "c"),
        ];
        trim_directory(&mut items, 2);
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].id, "b");
        assert_eq!(items[1].id, "c");
    }
}
