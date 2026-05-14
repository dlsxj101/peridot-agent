//! LLM provider contracts and provider skeletons.

use async_trait::async_trait;
use peridot_common::{PeriError, PeriResult, ToolCall};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

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
    api_key: Option<String>,
    base_url: String,
    client: reqwest::Client,
    pricing: PricingTable,
}

impl ClaudeProvider {
    /// Creates a Claude provider skeleton.
    pub fn new(model: impl Into<String>, api_key_present: bool) -> Self {
        let api_key = api_key_present.then(|| "configured".to_string());
        Self::with_options(model, api_key, "https://api.anthropic.com")
    }

    /// Creates a Claude provider with explicit API options.
    pub fn with_options(
        model: impl Into<String>,
        api_key: Option<String>,
        base_url: impl Into<String>,
    ) -> Self {
        Self {
            model: model.into(),
            api_key,
            base_url: base_url.into(),
            client: reqwest::Client::new(),
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

    /// Returns the configured Anthropic base URL.
    pub fn base_url(&self) -> &str {
        &self.base_url
    }
}

#[async_trait]
impl LlmProvider for ClaudeProvider {
    async fn complete(&self, request: CompletionRequest) -> PeriResult<CompletionResponse> {
        let api_key = self
            .api_key
            .as_deref()
            .ok_or_else(|| PeriError::Provider("missing Anthropic API key".to_string()))?;
        let payload = anthropic_payload(&request);
        let endpoint = format!("{}/v1/messages", self.base_url.trim_end_matches('/'));
        let response = self
            .client
            .post(endpoint)
            .header("x-api-key", api_key)
            .header("anthropic-version", "2023-06-01")
            .header("content-type", "application/json")
            .json(&payload)
            .send()
            .await
            .map_err(|err| PeriError::Provider(format!("Anthropic request failed: {err}")))?;

        let status = response.status();
        let body = response
            .text()
            .await
            .map_err(|err| PeriError::Provider(format!("Anthropic response read failed: {err}")))?;
        if !status.is_success() {
            return Err(PeriError::Provider(format!(
                "Anthropic request returned {status}: {body}"
            )));
        }
        parse_anthropic_response(&body, self.pricing)
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
        if self.api_key.is_some() {
            AuthMethod::ApiKey
        } else {
            AuthMethod::NotConfigured
        }
    }
}

fn anthropic_payload(request: &CompletionRequest) -> Value {
    let mut system_parts = Vec::new();
    if let Some(system) = &request.system {
        system_parts.push(system.clone());
    }

    let messages = request
        .messages
        .iter()
        .filter_map(|message| match message.role {
            MessageRole::System => {
                system_parts.push(message.content.clone());
                None
            }
            MessageRole::Assistant => Some(json!({
                "role": "assistant",
                "content": message.content
            })),
            MessageRole::User | MessageRole::Tool => Some(json!({
                "role": "user",
                "content": message.content
            })),
        })
        .collect::<Vec<_>>();

    let mut payload = json!({
        "model": request.model,
        "max_tokens": request.max_tokens.unwrap_or(4096),
        "messages": messages
    });

    if !system_parts.is_empty() {
        payload["system"] = Value::String(system_parts.join("\n\n"));
    }

    if request.thinking {
        payload["thinking"] = json!({
            "type": "enabled",
            "budget_tokens": 1024
        });
    }

    payload
}

fn parse_anthropic_response(body: &str, pricing: PricingTable) -> PeriResult<CompletionResponse> {
    let value = serde_json::from_str::<Value>(body)
        .map_err(|err| PeriError::Provider(format!("invalid Anthropic JSON: {err}")))?;
    let text = value
        .get("content")
        .and_then(Value::as_array)
        .map(|content| {
            content
                .iter()
                .filter_map(|part| {
                    (part.get("type").and_then(Value::as_str) == Some("text"))
                        .then(|| part.get("text").and_then(Value::as_str))
                        .flatten()
                })
                .collect::<Vec<_>>()
                .join("")
        })
        .unwrap_or_default();
    let usage_value = value.get("usage").unwrap_or(&Value::Null);
    let input_tokens = usage_value
        .get("input_tokens")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let output_tokens = usage_value
        .get("output_tokens")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let cache_read_tokens = usage_value
        .get("cache_read_input_tokens")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let cache_creation_tokens = usage_value
        .get("cache_creation_input_tokens")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let estimated_cost_usd = estimate_cost(
        pricing,
        input_tokens + cache_creation_tokens,
        output_tokens,
        cache_read_tokens,
    );

    Ok(CompletionResponse {
        text,
        usage: Usage {
            input_tokens,
            output_tokens,
            cache_read_tokens,
            cache_creation_tokens,
            estimated_cost_usd,
        },
    })
}

fn estimate_cost(
    pricing: PricingTable,
    input_tokens: u64,
    output_tokens: u64,
    cache_read_tokens: u64,
) -> f64 {
    (input_tokens as f64 / 1_000_000.0 * pricing.input_per_million)
        + (output_tokens as f64 / 1_000_000.0 * pricing.output_per_million)
        + (cache_read_tokens as f64 / 1_000_000.0 * pricing.cache_read_per_million)
}

/// OpenAI provider using the Responses API.
#[derive(Clone, Debug)]
pub struct OpenAiProvider {
    auth_method: AuthMethod,
    model: String,
    api_key: Option<String>,
    base_url: String,
    client: reqwest::Client,
    pricing: PricingTable,
}

impl OpenAiProvider {
    /// Creates an OpenAI provider skeleton without credentials.
    pub fn new(auth_method: AuthMethod) -> Self {
        Self::with_options("gpt-5.2", None, "https://api.openai.com", auth_method)
    }

    /// Creates an OpenAI provider with explicit API options.
    pub fn with_options(
        model: impl Into<String>,
        api_key: Option<String>,
        base_url: impl Into<String>,
        auth_method: AuthMethod,
    ) -> Self {
        Self {
            auth_method,
            model: model.into(),
            api_key,
            base_url: base_url.into(),
            client: reqwest::Client::new(),
            pricing: PricingTable {
                input_per_million: 1.25,
                output_per_million: 10.0,
                cache_read_per_million: 0.125,
            },
        }
    }

    /// Returns the configured model name.
    pub fn model(&self) -> &str {
        &self.model
    }

    /// Returns the configured OpenAI base URL.
    pub fn base_url(&self) -> &str {
        &self.base_url
    }
}

#[async_trait]
impl LlmProvider for OpenAiProvider {
    async fn complete(&self, request: CompletionRequest) -> PeriResult<CompletionResponse> {
        let api_key = self
            .api_key
            .as_deref()
            .ok_or_else(|| PeriError::Provider("missing OpenAI API key".to_string()))?;
        let payload = openai_responses_payload(&request);
        let endpoint = format!("{}/v1/responses", self.base_url.trim_end_matches('/'));
        let response = self
            .client
            .post(endpoint)
            .bearer_auth(api_key)
            .header("content-type", "application/json")
            .json(&payload)
            .send()
            .await
            .map_err(|err| PeriError::Provider(format!("OpenAI request failed: {err}")))?;

        let status = response.status();
        let body = response
            .text()
            .await
            .map_err(|err| PeriError::Provider(format!("OpenAI response read failed: {err}")))?;
        if !status.is_success() {
            return Err(PeriError::Provider(format!(
                "OpenAI request returned {status}: {body}"
            )));
        }
        parse_openai_response(&body, self.pricing)
    }

    fn supports_cache(&self) -> bool {
        true
    }

    fn supports_prefill(&self) -> bool {
        false
    }

    fn supports_thinking(&self) -> bool {
        true
    }

    fn pricing(&self) -> PricingTable {
        self.pricing
    }

    fn auth_method(&self) -> AuthMethod {
        self.auth_method.clone()
    }
}

fn openai_responses_payload(request: &CompletionRequest) -> Value {
    let input = request
        .messages
        .iter()
        .filter_map(|message| {
            if message.role == MessageRole::System {
                return None;
            }
            let role = match message.role {
                MessageRole::Assistant => "assistant",
                MessageRole::System | MessageRole::User | MessageRole::Tool => "user",
            };
            Some(json!({
                "role": role,
                "content": message.content
            }))
        })
        .collect::<Vec<_>>();
    let mut instructions = Vec::new();
    if let Some(system) = &request.system {
        instructions.push(system.clone());
    }
    instructions.extend(
        request
            .messages
            .iter()
            .filter(|message| message.role == MessageRole::System)
            .map(|message| message.content.clone()),
    );

    let mut payload = json!({
        "model": request.model,
        "input": input,
        "store": false
    });
    if let Some(max_tokens) = request.max_tokens {
        payload["max_output_tokens"] = json!(max_tokens);
    }
    if !instructions.is_empty() {
        payload["instructions"] = Value::String(instructions.join("\n\n"));
    }
    payload
}

fn parse_openai_response(body: &str, pricing: PricingTable) -> PeriResult<CompletionResponse> {
    let value = serde_json::from_str::<Value>(body)
        .map_err(|err| PeriError::Provider(format!("invalid OpenAI JSON: {err}")))?;
    let text = value
        .get("output_text")
        .and_then(Value::as_str)
        .map(str::to_string)
        .unwrap_or_else(|| openai_output_text(&value));
    let usage_value = value.get("usage").unwrap_or(&Value::Null);
    let input_tokens = usage_value
        .get("input_tokens")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let output_tokens = usage_value
        .get("output_tokens")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let cache_read_tokens = usage_value
        .get("input_tokens_details")
        .and_then(|details| details.get("cached_tokens"))
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let estimated_cost_usd = estimate_cost(pricing, input_tokens, output_tokens, cache_read_tokens);

    Ok(CompletionResponse {
        text,
        usage: Usage {
            input_tokens,
            output_tokens,
            cache_read_tokens,
            cache_creation_tokens: 0,
            estimated_cost_usd,
        },
    })
}

fn openai_output_text(value: &Value) -> String {
    value
        .get("output")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|item| item.get("type").and_then(Value::as_str) == Some("message"))
        .filter_map(|item| item.get("content").and_then(Value::as_array))
        .flatten()
        .filter_map(|content| {
            (content.get("type").and_then(Value::as_str) == Some("output_text"))
                .then(|| content.get("text").and_then(Value::as_str))
                .flatten()
        })
        .collect::<Vec<_>>()
        .join("")
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

    #[test]
    fn anthropic_payload_moves_system_to_top_level() {
        let payload = anthropic_payload(&CompletionRequest {
            model: "claude-sonnet-4-20250514".to_string(),
            system: Some("top".to_string()),
            messages: vec![
                LlmMessage::new(MessageRole::System, "inline"),
                LlmMessage::new(MessageRole::User, "hello"),
            ],
            max_tokens: Some(128),
            thinking: false,
        });

        assert_eq!(payload["system"], "top\n\ninline");
        assert_eq!(payload["messages"][0]["role"], "user");
    }

    #[test]
    fn openai_payload_uses_responses_shape() {
        let payload = openai_responses_payload(&CompletionRequest {
            model: "gpt-5.2".to_string(),
            system: Some("system".to_string()),
            messages: vec![LlmMessage::new(MessageRole::User, "hello")],
            max_tokens: Some(256),
            thinking: false,
        });

        assert_eq!(payload["model"], "gpt-5.2");
        assert_eq!(payload["instructions"], "system");
        assert_eq!(payload["max_output_tokens"], 256);
        assert_eq!(payload["input"][0]["role"], "user");
    }

    #[test]
    fn parses_anthropic_usage_and_text() {
        let response = parse_anthropic_response(
            r#"{
                "content":[{"type":"text","text":"hello"}],
                "usage":{
                    "input_tokens":10,
                    "cache_creation_input_tokens":2,
                    "cache_read_input_tokens":3,
                    "output_tokens":4
                }
            }"#,
            PricingTable {
                input_per_million: 3.0,
                output_per_million: 15.0,
                cache_read_per_million: 0.30,
            },
        )
        .unwrap();

        assert_eq!(response.text, "hello");
        assert_eq!(response.usage.input_tokens, 10);
        assert_eq!(response.usage.cache_creation_tokens, 2);
        assert!(response.usage.estimated_cost_usd > 0.0);
    }

    #[test]
    fn parses_openai_response_output_text() {
        let response = parse_openai_response(
            r#"{
                "output_text": "{\"action\":\"agent_done\"}",
                "usage": {
                    "input_tokens": 10,
                    "output_tokens": 5,
                    "input_tokens_details": {"cached_tokens": 2}
                }
            }"#,
            PricingTable::default(),
        )
        .unwrap();

        assert_eq!(response.text, "{\"action\":\"agent_done\"}");
        assert_eq!(response.usage.input_tokens, 10);
        assert_eq!(response.usage.output_tokens, 5);
        assert_eq!(response.usage.cache_read_tokens, 2);
    }

    #[test]
    fn parses_openai_response_output_items() {
        let response = parse_openai_response(
            r#"{
                "output": [{
                    "type": "message",
                    "content": [{"type": "output_text", "text": "ok"}]
                }]
            }"#,
            PricingTable::default(),
        )
        .unwrap();

        assert_eq!(response.text, "ok");
    }
}
