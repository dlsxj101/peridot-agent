/// Authentication method used by an LLM provider.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuthMethod {
    /// API-key based authentication.
    ApiKey,
    /// OAuth based authentication.
    OAuth,
    /// No authentication is configured yet.
    NotConfigured,
}

/// Role for a message sent to a model.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MessageRole {
    /// System-level instruction.
    System,
    /// User-authored content.
    User,
    /// Assistant-authored content.
    Assistant,
    /// Tool observation content.
    Tool,
}

/// A chat message in Peridot's provider-neutral format.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct LlmMessage {
    /// Message role.
    pub role: MessageRole,
    /// Message text content.
    pub content: String,
}

impl LlmMessage {
    /// Creates a new provider-neutral LLM message.
    pub fn new(role: MessageRole, content: impl Into<String>) -> Self {
        Self {
            role,
            content: content.into(),
        }
    }
}

/// Completion request sent through an LLM provider.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CompletionRequest {
    /// Model identifier.
    pub model: String,
    /// Optional top-level system prompt.
    pub system: Option<String>,
    /// Ordered messages.
    pub messages: Vec<LlmMessage>,
    /// Maximum output tokens.
    pub max_tokens: Option<u32>,
    /// Whether extended thinking is enabled for the session.
    pub thinking: bool,
}

/// Token and billing usage returned by a provider.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Usage {
    /// Input tokens billed normally.
    pub input_tokens: u64,
    /// Output tokens billed normally.
    pub output_tokens: u64,
    /// Cached input tokens.
    pub cache_read_tokens: u64,
    /// Cache creation input tokens.
    pub cache_creation_tokens: u64,
    /// Reasoning output tokens when reported separately by the provider.
    #[serde(default)]
    pub reasoning_output_tokens: u64,
    /// Estimated cost in USD.
    pub estimated_cost_usd: f64,
}

/// Completion response returned by an LLM provider.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CompletionResponse {
    /// Raw assistant text.
    pub text: String,
    /// Provider-reported usage.
    pub usage: Usage,
}

/// Provider-neutral streaming chunk.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct CompletionStreamChunk {
    /// Text delta for this chunk.
    pub delta: String,
    /// Whether this is the final chunk.
    pub done: bool,
    /// Usage accounting, populated on the final chunk when available.
    #[serde(default)]
    pub usage: Option<Usage>,
}

/// Structured model action parsed from assistant text.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ParsedAction {
    /// Optional reasoning text when present.
    pub thinking: Option<String>,
    /// Tool call requested by the model.
    pub tool_call: ToolCall,
}
use peridot_common::ToolCall;
use serde::{Deserialize, Serialize};
