//! Append-only conversation context management.

use peridot_llm::{LlmMessage, MessageRole};
use serde::{Deserialize, Serialize};

/// Source category for a context entry.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContextSource {
    /// User-authored instruction.
    User,
    /// Assistant output.
    Assistant,
    /// Tool observation.
    Tool,
    /// Injected plan reminder.
    PlanReminder,
    /// External untrusted content.
    External,
}

/// One immutable entry in the append-only context log.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ContextEntry {
    /// Source category.
    pub source: ContextSource,
    /// Entry content.
    pub content: String,
    /// Whether this content must be treated as untrusted external text.
    pub untrusted: bool,
}

impl ContextEntry {
    /// Creates a trusted context entry.
    pub fn trusted(source: ContextSource, content: impl Into<String>) -> Self {
        Self {
            source,
            content: content.into(),
            untrusted: false,
        }
    }

    /// Creates an untrusted context entry.
    pub fn untrusted(source: ContextSource, content: impl Into<String>) -> Self {
        Self {
            source,
            content: content.into(),
            untrusted: true,
        }
    }
}

/// Append-only context manager.
#[derive(Clone, Debug, Default)]
pub struct ContextManager {
    entries: Vec<ContextEntry>,
}

impl ContextManager {
    /// Creates an empty context manager.
    pub fn new() -> Self {
        Self::default()
    }

    /// Appends an entry without mutating previous entries.
    pub fn append(&mut self, entry: ContextEntry) {
        self.entries.push(entry);
    }

    /// Returns all context entries in append order.
    pub fn entries(&self) -> &[ContextEntry] {
        &self.entries
    }

    /// Estimates tokens with a conservative character heuristic.
    pub fn estimated_tokens(&self) -> usize {
        self.entries
            .iter()
            .map(|entry| entry.content.len().div_ceil(4))
            .sum()
    }

    /// Builds provider-neutral messages from the current entries.
    pub fn to_messages(&self) -> Vec<LlmMessage> {
        self.entries
            .iter()
            .map(|entry| {
                let role = match entry.source {
                    ContextSource::User => MessageRole::User,
                    ContextSource::Assistant => MessageRole::Assistant,
                    ContextSource::Tool | ContextSource::PlanReminder | ContextSource::External => {
                        MessageRole::User
                    }
                };
                let content = if entry.untrusted {
                    format!("<untrusted>\n{}\n</untrusted>", entry.content)
                } else {
                    entry.content.clone()
                };
                LlmMessage::new(role, content)
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn context_is_append_only() {
        let mut manager = ContextManager::new();
        manager.append(ContextEntry::trusted(ContextSource::User, "hello"));
        manager.append(ContextEntry::trusted(ContextSource::Assistant, "world"));

        assert_eq!(manager.entries()[0].content, "hello");
        assert_eq!(manager.entries()[1].content, "world");
        assert!(manager.estimated_tokens() >= 2);
    }
}
