//! LLM provider contracts and provider skeletons.

use async_trait::async_trait;
use peridot_common::{PeriError, PeriResult, ToolCall};
use serde::{Deserialize, Serialize};
use serde_json::Value;

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
    /// Ordered messages.
    pub messages: Vec<LlmMessage>,
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

/// Structured model action parsed from assistant text.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ParsedAction {
    /// Optional reasoning text when present.
    pub thinking: Option<String>,
    /// Tool call requested by the model.
    pub tool_call: ToolCall,
}

/// Parses an assistant response using Peridot's staged fallback strategy.
pub fn parse_action(text: &str) -> PeriResult<ParsedAction> {
    if let Ok(value) = serde_json::from_str::<Value>(text) {
        return parse_action_value(value);
    }

    if let Some(block) = first_json_code_block(text)
        && let Ok(value) = serde_json::from_str::<Value>(block)
    {
        return parse_action_value(value);
    }

    if let Some(object) = first_json_object(text)
        && let Ok(value) = serde_json::from_str::<Value>(&object)
    {
        return parse_action_value(value);
    }

    if let Some(action) = extract_action_key(text) {
        return Ok(ParsedAction {
            thinking: None,
            tool_call: ToolCall::new(action, Value::Null),
        });
    }

    Err(PeriError::Parse(
        "assistant response did not contain a recoverable action".to_string(),
    ))
}

fn parse_action_value(value: Value) -> PeriResult<ParsedAction> {
    let action = value
        .get("action")
        .and_then(Value::as_str)
        .ok_or_else(|| PeriError::Parse("missing action field".to_string()))?;
    let thinking = value
        .get("thinking")
        .and_then(Value::as_str)
        .map(str::to_string);
    let parameters = value.get("parameters").cloned().unwrap_or(Value::Null);
    Ok(ParsedAction {
        thinking,
        tool_call: ToolCall::new(action, parameters),
    })
}

fn first_json_code_block(text: &str) -> Option<&str> {
    let start = text.find("```")?;
    let after_fence = &text[start + 3..];
    let content_start = after_fence.find('\n').map_or(0, |idx| idx + 1);
    let after_lang = &after_fence[content_start..];
    let end = after_lang.find("```")?;
    Some(after_lang[..end].trim())
}

fn first_json_object(text: &str) -> Option<String> {
    let start = text.find('{')?;
    let mut depth = 0usize;
    let mut in_string = false;
    let mut escaped = false;

    for (offset, ch) in text[start..].char_indices() {
        if in_string {
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == '"' {
                in_string = false;
            }
            continue;
        }

        match ch {
            '"' => in_string = true,
            '{' => depth += 1,
            '}' => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    return Some(text[start..start + offset + ch.len_utf8()].to_string());
                }
            }
            _ => {}
        }
    }

    None
}

fn extract_action_key(text: &str) -> Option<String> {
    let needle = "\"action\"";
    let start = text.find(needle)? + needle.len();
    let after_key = text[start..].trim_start();
    let after_colon = after_key.strip_prefix(':')?.trim_start();
    let action = after_colon.strip_prefix('"')?;
    let end = action.find('"')?;
    Some(action[..end].to_string())
}

/// Provider pricing information.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct PricingTable {
    /// Cost per million input tokens.
    pub input_per_million: f64,
    /// Cost per million output tokens.
    pub output_per_million: f64,
    /// Cost per million cache-read tokens.
    pub cache_read_per_million: f64,
}

/// Provider abstraction for chat completion and future streaming support.
#[async_trait]
pub trait LlmProvider: Send + Sync {
    /// Complete one model request.
    async fn complete(&self, request: CompletionRequest) -> PeriResult<CompletionResponse>;

    /// Returns true when the provider supports prompt caching.
    fn supports_cache(&self) -> bool;

    /// Returns true when the provider supports response prefill/tool masking.
    fn supports_prefill(&self) -> bool;

    /// Returns true when the provider supports extended thinking controls.
    fn supports_thinking(&self) -> bool;

    /// Returns the provider pricing table.
    fn pricing(&self) -> PricingTable;

    /// Returns the active auth method.
    fn auth_method(&self) -> AuthMethod;
}

/// Claude provider placeholder for Session 1.
#[derive(Clone, Debug)]
pub struct ClaudeProvider {
    model: String,
    api_key_present: bool,
    pricing: PricingTable,
}

impl ClaudeProvider {
    /// Creates a Claude provider skeleton.
    pub fn new(model: impl Into<String>, api_key_present: bool) -> Self {
        Self {
            model: model.into(),
            api_key_present,
            pricing: PricingTable {
                input_per_million: 3.0,
                output_per_million: 15.0,
                cache_read_per_million: 0.30,
            },
        }
    }

    /// Returns the configured model name.
    pub fn model(&self) -> &str {
        &self.model
    }
}

#[async_trait]
impl LlmProvider for ClaudeProvider {
    async fn complete(&self, _request: CompletionRequest) -> PeriResult<CompletionResponse> {
        Err(PeriError::Provider(
            "ClaudeProvider network path is not implemented yet".to_string(),
        ))
    }

    fn supports_cache(&self) -> bool {
        true
    }

    fn supports_prefill(&self) -> bool {
        true
    }

    fn supports_thinking(&self) -> bool {
        true
    }

    fn pricing(&self) -> PricingTable {
        self.pricing
    }

    fn auth_method(&self) -> AuthMethod {
        if self.api_key_present {
            AuthMethod::ApiKey
        } else {
            AuthMethod::NotConfigured
        }
    }
}

/// OpenAI provider placeholder for later Codex OAuth/API support.
#[derive(Clone, Debug)]
pub struct OpenAiProvider {
    auth_method: AuthMethod,
}

impl OpenAiProvider {
    /// Creates an OpenAI provider skeleton.
    pub fn new(auth_method: AuthMethod) -> Self {
        Self { auth_method }
    }
}

#[async_trait]
impl LlmProvider for OpenAiProvider {
    async fn complete(&self, _request: CompletionRequest) -> PeriResult<CompletionResponse> {
        Err(PeriError::Provider(
            "OpenAiProvider network path is not implemented yet".to_string(),
        ))
    }

    fn supports_cache(&self) -> bool {
        false
    }

    fn supports_prefill(&self) -> bool {
        false
    }

    fn supports_thinking(&self) -> bool {
        true
    }

    fn pricing(&self) -> PricingTable {
        PricingTable::default()
    }

    fn auth_method(&self) -> AuthMethod {
        self.auth_method.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_direct_json_action() {
        let action =
            parse_action(r#"{"thinking":"ok","action":"agent_done","parameters":{"done":true}}"#)
                .unwrap();

        assert_eq!(action.thinking.as_deref(), Some("ok"));
        assert_eq!(action.tool_call.name, "agent_done");
    }

    #[test]
    fn parses_json_code_block() {
        let action = parse_action(
            r#"Here:
```json
{"action":"file_read","parameters":{"path":"README.md"}}
```"#,
        )
        .unwrap();

        assert_eq!(action.tool_call.name, "file_read");
    }

    #[test]
    fn extracts_first_json_object() {
        let action =
            parse_action(r#"noise {"action":"plan_create","parameters":{"steps":[]}} tail"#)
                .unwrap();

        assert_eq!(action.tool_call.name, "plan_create");
    }
}
