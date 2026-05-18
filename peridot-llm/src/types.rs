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
///
/// Carries native tool-calling metadata so providers can emit the structured wire
/// format that OpenAI and Anthropic expect — an assistant turn with `tool_calls` is
/// always followed by `Tool` turn(s) whose `tool_call_id` matches an entry in
/// `tool_calls`. Plain chat messages leave both fields empty.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct LlmMessage {
    /// Message role.
    pub role: MessageRole,
    /// Message text content. May be empty for assistant turns that consist solely of
    /// tool calls, or for `Tool` turns whose payload lives in `content` after the
    /// tool runs.
    pub content: String,
    /// Tool calls emitted by the assistant on this turn. Empty for non-assistant
    /// turns and for assistant turns that returned plain text only.
    #[serde(default)]
    pub tool_calls: Vec<ToolInvocation>,
    /// Identifier of the assistant tool call this message answers. Only set on
    /// `Tool` role messages; pairs with one of the assistant's `tool_calls` ids.
    #[serde(default)]
    pub tool_call_id: Option<String>,
}

impl LlmMessage {
    /// Creates a new plain provider-neutral LLM message.
    pub fn new(role: MessageRole, content: impl Into<String>) -> Self {
        Self {
            role,
            content: content.into(),
            tool_calls: Vec::new(),
            tool_call_id: None,
        }
    }

    /// Builds an assistant message carrying native tool calls. `content` may be
    /// empty when the model returned only tool calls (no text).
    pub fn assistant_with_tool_calls(
        content: impl Into<String>,
        tool_calls: Vec<ToolInvocation>,
    ) -> Self {
        Self {
            role: MessageRole::Assistant,
            content: content.into(),
            tool_calls,
            tool_call_id: None,
        }
    }

    /// Builds a tool-result message paired with the assistant call that produced it.
    pub fn tool_result(tool_call_id: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            role: MessageRole::Tool,
            content: content.into(),
            tool_calls: Vec::new(),
            tool_call_id: Some(tool_call_id.into()),
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
    ///
    /// **Deprecated** in favour of `reasoning_effort`. Kept as a boolean for
    /// backward compatibility — providers that read it treat
    /// `thinking == true` as `ReasoningEffort::Medium` when
    /// `reasoning_effort` is left at its default. New call sites should set
    /// `reasoning_effort` instead and leave this `false`.
    pub thinking: bool,
    /// Requested reasoning depth. Providers translate this to their native
    /// reasoning controls: Anthropic → `thinking: { type: enabled,
    /// budget_tokens }`; OpenAI o-series / gpt-5 → `reasoning: { effort }`;
    /// Codex → forwarded via app-server. `Off` (the default) disables
    /// reasoning entirely so cheap chat-style models keep their cost.
    #[serde(default)]
    pub reasoning_effort: ReasoningEffort,
    /// Optional provider service tier such as `fast` / `priority`.
    /// Providers that do not support service tiers ignore it.
    #[serde(default)]
    pub service_tier: Option<String>,
    /// Native tool definitions surfaced to the model. Empty disables tool calling.
    #[serde(default)]
    pub tools: Vec<ToolDefinition>,
    /// Tool-choice policy mirroring OpenAI/Anthropic semantics.
    #[serde(default)]
    pub tool_choice: ToolChoice,
}

pub use peridot_common::ReasoningEffort;

/// Provider-neutral tool definition surfaced via native tool calling.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ToolDefinition {
    /// Stable tool name the model uses when invoking the function.
    pub name: String,
    /// Human-readable description shown to the model.
    pub description: String,
    /// JSON Schema describing the tool's parameter object.
    pub parameters: Value,
}

/// Tool-choice policy passed to providers.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolChoice {
    /// Model decides whether to call a tool or reply with text.
    #[default]
    Auto,
    /// Model must not call a tool.
    None,
    /// Model must call at least one tool.
    Required,
}

/// One tool invocation requested by the model.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ToolInvocation {
    /// Provider-supplied identifier used to pair the call with its result.
    pub id: String,
    /// Tool name the model wants to call.
    pub name: String,
    /// Raw JSON arguments emitted by the model.
    pub arguments: Value,
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
    /// Raw assistant text. Empty when the model responded with tool calls only.
    pub text: String,
    /// Tool invocations requested by the model. Empty when the model replied with plain text.
    #[serde(default)]
    pub tool_calls: Vec<ToolInvocation>,
    /// Reasoning / chain-of-thought content surfaced separately from the
    /// final reply text. Captured from Anthropic `thinking` content blocks
    /// and OpenAI `reasoning.content` / `reasoning_summary` fields when
    /// present. The TUI does not render this today — it would clutter the
    /// chat view — but downstream consumers (VSCode extension, web GUI,
    /// audit log replayer) can surface it. `None` for providers / models
    /// that don't expose reasoning text.
    #[serde(default)]
    pub reasoning_content: Option<String>,
    /// Provider-reported usage.
    pub usage: Usage,
}

/// Provider-neutral streaming chunk.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct CompletionStreamChunk {
    /// Text delta for this chunk.
    pub delta: String,
    /// Reasoning / thinking delta for this chunk, when the provider emits a
    /// separate reasoning channel. Captured for downstream consumers; the
    /// TUI does not display it.
    #[serde(default)]
    pub reasoning_delta: String,
    /// Tool calls assembled from this chunk (populated on the final chunk).
    #[serde(default)]
    pub tool_calls: Vec<ToolInvocation>,
    /// Whether this is the final chunk.
    pub done: bool,
    /// Usage accounting, populated on the final chunk when available.
    #[serde(default)]
    pub usage: Option<Usage>,
}

use serde::{Deserialize, Serialize};
use serde_json::Value;
