//! Append-only conversation context management.

use std::fs;
use std::path::{Path, PathBuf};

use peridot_common::{PeriError, PeriResult};
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

/// Context manager limits and offload configuration.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ContextLimits {
    /// Hard token limit for message construction.
    pub hard_limit_tokens: usize,
    /// Character threshold above which observations are offloaded.
    pub offload_threshold_chars: usize,
    /// Directory where offloaded observations are written.
    pub offload_dir: Option<PathBuf>,
}

impl Default for ContextLimits {
    fn default() -> Self {
        Self {
            hard_limit_tokens: 160_000,
            offload_threshold_chars: 3_000,
            offload_dir: None,
        }
    }
}

/// Append-only context manager.
#[derive(Clone, Debug, Default)]
pub struct ContextManager {
    entries: Vec<ContextEntry>,
    limits: ContextLimits,
    offload_counter: usize,
}

impl ContextManager {
    /// Creates an empty context manager.
    pub fn new() -> Self {
        Self::default()
    }

    /// Creates a context manager with explicit limits.
    pub fn with_limits(limits: ContextLimits) -> Self {
        Self {
            entries: Vec::new(),
            limits,
            offload_counter: 0,
        }
    }

    /// Appends an entry without mutating previous entries.
    pub fn append(&mut self, entry: ContextEntry) {
        self.entries.push(entry);
    }

    /// Appends a tool observation, offloading large content when configured.
    pub fn append_observation(&mut self, content: impl Into<String>) -> PeriResult<()> {
        let content = content.into();
        if content.len() <= self.limits.offload_threshold_chars {
            self.append(ContextEntry::untrusted(ContextSource::Tool, content));
            return Ok(());
        }

        let Some(offload_dir) = self.limits.offload_dir.clone() else {
            self.append(ContextEntry::untrusted(ContextSource::Tool, content));
            return Ok(());
        };

        fs::create_dir_all(&offload_dir).map_err(|err| {
            PeriError::Tool(format!(
                "failed to create offload dir {}: {err}",
                offload_dir.display()
            ))
        })?;
        self.offload_counter += 1;
        let path = offload_dir.join(format!("observation-{}.txt", self.offload_counter));
        fs::write(&path, content).map_err(|err| {
            PeriError::Tool(format!("failed to write offload {}: {err}", path.display()))
        })?;
        self.append(ContextEntry::untrusted(
            ContextSource::Tool,
            format!(
                "Large observation offloaded to {}. Read it if needed.",
                path.display()
            ),
        ));
        Ok(())
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
        let mut messages = self
            .entries
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
            .collect::<Vec<_>>();
        merge_consecutive_roles(&mut messages);
        trim_to_hard_limit(&mut messages, self.limits.hard_limit_tokens);
        messages
    }
}

fn merge_consecutive_roles(messages: &mut Vec<LlmMessage>) {
    let mut merged: Vec<LlmMessage> = Vec::new();
    for message in messages.drain(..) {
        if let Some(last) = merged.last_mut()
            && last.role == message.role
        {
            last.content.push_str("\n\n");
            last.content.push_str(&message.content);
            continue;
        }
        merged.push(message);
    }
    *messages = merged;
}

fn trim_to_hard_limit(messages: &mut Vec<LlmMessage>, hard_limit_tokens: usize) {
    while estimated_message_tokens(messages) > hard_limit_tokens && messages.len() > 1 {
        messages.remove(0);
    }
}

fn estimated_message_tokens(messages: &[LlmMessage]) -> usize {
    messages
        .iter()
        .map(|message| message.content.len().div_ceil(4))
        .sum()
}

/// Builds default offload limits for a project root.
pub fn project_context_limits(project_root: &Path) -> ContextLimits {
    ContextLimits {
        offload_dir: Some(project_root.join(".peridot/mem")),
        ..ContextLimits::default()
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

    #[test]
    fn messages_merge_consecutive_roles() {
        let mut manager = ContextManager::new();
        manager.append(ContextEntry::trusted(ContextSource::User, "one"));
        manager.append(ContextEntry::trusted(ContextSource::Tool, "two"));
        manager.append(ContextEntry::trusted(ContextSource::Assistant, "three"));

        let messages = manager.to_messages();

        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0].content, "one\n\ntwo");
    }

    #[test]
    fn large_observations_are_offloaded() {
        let root =
            std::env::temp_dir().join(format!("peridot-context-offload-{}", std::process::id()));
        let mut manager = ContextManager::with_limits(ContextLimits {
            hard_limit_tokens: 160_000,
            offload_threshold_chars: 4,
            offload_dir: Some(root.clone()),
        });

        manager.append_observation("this is large").unwrap();

        assert_eq!(manager.entries().len(), 1);
        assert!(manager.entries()[0].content.contains("offloaded"));
        assert!(root.join("observation-1.txt").exists());
        fs::remove_dir_all(root).unwrap();
    }
}
