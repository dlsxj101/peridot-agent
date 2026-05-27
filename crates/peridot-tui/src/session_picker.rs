//! Session picker for switching foreground sessions by title/id.

use serde::{Deserialize, Serialize};

use crate::session_directory::SessionDirectoryItem;

/// Live state for the `Ctrl+T` session picker overlay.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct SessionPickerState {
    /// Prefix query typed while the picker is open.
    pub query: String,
    /// Index of the highlighted row inside the filtered session list.
    pub selected: usize,
}

impl SessionPickerState {
    /// Builds an empty picker with no filter.
    pub fn opening() -> Self {
        Self::default()
    }

    /// Appends one character to the query and resets selection.
    pub fn push_query_char(&mut self, ch: char) {
        self.query.push(ch);
        self.selected = 0;
    }

    /// Removes one character from the query and resets selection.
    pub fn backspace_query(&mut self) {
        self.query.pop();
        self.selected = 0;
    }

    /// Moves the highlighted row by `delta`, wrapping at the edges.
    pub fn move_selection(&mut self, delta: i32, match_count: usize) {
        if match_count == 0 {
            self.selected = 0;
            return;
        }
        let len = match_count as i32;
        let current = self.selected.min(match_count - 1) as i32;
        self.selected = ((current + delta).rem_euclid(len)) as usize;
    }

    /// Returns the selected session id from the current query, if any.
    pub fn selected_session_id(&self, sessions: &[SessionDirectoryItem]) -> Option<String> {
        filtered_sessions(sessions, &self.query)
            .get(self.selected)
            .map(|item| item.id.clone())
    }
}

/// Returns sessions matching `query`, ranked by recency. Matching is prefix
/// based against the id, full title, and title words.
pub(crate) fn filtered_sessions<'a>(
    sessions: &'a [SessionDirectoryItem],
    query: &str,
) -> Vec<&'a SessionDirectoryItem> {
    let query = query.trim().to_ascii_lowercase();
    let mut matches: Vec<(usize, &SessionDirectoryItem)> = sessions
        .iter()
        .enumerate()
        .filter(|(_, item)| query.is_empty() || session_matches_query(item, &query))
        .collect();
    matches.sort_by(|(left_index, left), (right_index, right)| {
        right
            .last_event_at_unix
            .cmp(&left.last_event_at_unix)
            .then_with(|| left_index.cmp(right_index))
    });
    matches.into_iter().map(|(_, item)| item).collect()
}

fn session_matches_query(item: &SessionDirectoryItem, query: &str) -> bool {
    let id = item.id.to_ascii_lowercase();
    let title = item.title.to_ascii_lowercase();
    id.starts_with(query)
        || title.starts_with(query)
        || title.split_whitespace().any(|word| word.starts_with(query))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session(id: &str, title: &str, ts: u64) -> SessionDirectoryItem {
        let mut item = SessionDirectoryItem::new(id, title);
        item.last_event_at_unix = ts;
        item
    }

    #[test]
    fn filtered_sessions_rank_by_recency() {
        let sessions = vec![
            session("a", "Alpha work", 10),
            session("b", "Beta work", 30),
            session("c", "Compiler audit", 20),
        ];

        let ids: Vec<&str> = filtered_sessions(&sessions, "")
            .into_iter()
            .map(|item| item.id.as_str())
            .collect();

        assert_eq!(ids, vec!["b", "c", "a"]);
    }

    #[test]
    fn filtered_sessions_matches_title_word_prefixes() {
        let sessions = vec![
            session("alpha-main", "Alpha work", 10),
            session("docs", "Write docs", 20),
            session("review", "Reviewer pass", 30),
        ];

        let ids: Vec<&str> = filtered_sessions(&sessions, "doc")
            .into_iter()
            .map(|item| item.id.as_str())
            .collect();

        assert_eq!(ids, vec!["docs"]);
    }

    #[test]
    fn selection_wraps_and_resets_on_query_changes() {
        let mut picker = SessionPickerState::opening();
        picker.move_selection(1, 3);
        assert_eq!(picker.selected, 1);
        picker.move_selection(-2, 3);
        assert_eq!(picker.selected, 2);

        picker.push_query_char('a');
        assert_eq!(picker.query, "a");
        assert_eq!(picker.selected, 0);

        picker.backspace_query();
        assert!(picker.query.is_empty());
        assert_eq!(picker.selected, 0);
    }
}
