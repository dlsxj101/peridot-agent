use std::collections::HashMap;
use std::time::Duration;

use async_trait::async_trait;
use peridot_common::{PeriError, PeriResult};
use serde_json::{Value, json};

use crate::transport::{
    estimate_cost, read_streaming_response, should_retry_status, sse_data_events,
    stream_sse_events,
};
use crate::{
    AuthMethod, CompletionRequest, CompletionResponse, CompletionStreamChunk, LlmProvider,
    MessageRole, PricingTable, ToolChoice, ToolInvocation, Usage,
};

pub struct ClaudeProvider {
    model: String,
    api_key: Option<String>,
    base_url: String,
    client: reqwest::Client,
    max_retries: u8,
    pricing: PricingTable,
}

impl ClaudeProvider {
    /// Creates a Claude provider without credentials.
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
        Self::with_transport_options(model, api_key, base_url, 120, 3)
    }

    /// Creates a Claude provider with explicit API transport options.
    pub fn with_transport_options(
        model: impl Into<String>,
        api_key: Option<String>,
        base_url: impl Into<String>,
        timeout_seconds: u64,
        max_retries: u8,
    ) -> Self {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(timeout_seconds.max(1)))
            .build()
            .unwrap_or_else(|_| reqwest::Client::new());
        Self {
            model: model.into(),
            api_key,
            base_url: base_url.into(),
            client,
            max_retries,
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

    /// Returns configured retry count.
    pub fn max_retries(&self) -> u8 {
        self.max_retries
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
        let mut last_error = None;
        for attempt in 0..=self.max_retries {
            let response = match self
                .client
                .post(&endpoint)
                .header("x-api-key", api_key)
                .header("anthropic-version", "2023-06-01")
                .header("content-type", "application/json")
                .json(&payload)
                .send()
                .await
            {
                Ok(response) => response,
                Err(err) => {
                    last_error = Some(format!("Anthropic request failed: {err}"));
                    if attempt < self.max_retries {
                        continue;
                    }
                    break;
                }
            };

            let status = response.status();
            let body = match response.text().await {
                Ok(body) => body,
                Err(err) => {
                    last_error = Some(format!("Anthropic response read failed: {err}"));
                    if attempt < self.max_retries {
                        continue;
                    }
                    break;
                }
            };
            if status.is_success() {
                return parse_anthropic_response(&body, self.pricing);
            }
            last_error = Some(format!("Anthropic request returned {status}: {body}"));
            if attempt < self.max_retries && should_retry_status(status) {
                continue;
            }
            break;
        }
        Err(PeriError::Provider(
            last_error.unwrap_or_else(|| "Anthropic request failed".to_string()),
        ))
    }

    async fn stream(&self, request: CompletionRequest) -> PeriResult<Vec<CompletionStreamChunk>> {
        let api_key = self
            .api_key
            .as_deref()
            .ok_or_else(|| PeriError::Provider("missing Anthropic API key".to_string()))?;
        let mut payload = anthropic_payload(&request);
        payload["stream"] = Value::Bool(true);
        let endpoint = format!("{}/v1/messages", self.base_url.trim_end_matches('/'));
        let mut last_error = None;
        for attempt in 0..=self.max_retries {
            let response = match self
                .client
                .post(&endpoint)
                .header("x-api-key", api_key)
                .header("anthropic-version", "2023-06-01")
                .header("content-type", "application/json")
                .header("accept", "text/event-stream")
                .json(&payload)
                .send()
                .await
            {
                Ok(response) => response,
                Err(err) => {
                    last_error = Some(format!("Anthropic stream request failed: {err}"));
                    if attempt < self.max_retries {
                        continue;
                    }
                    break;
                }
            };

            let status = response.status();
            if status.is_success() {
                let body = read_streaming_response(response).await?;
                return parse_anthropic_stream(&body, self.pricing);
            }
            let body = response.text().await.unwrap_or_default();
            last_error = Some(format!("Anthropic stream returned {status}: {body}"));
            if attempt < self.max_retries && should_retry_status(status) {
                continue;
            }
            break;
        }
        Err(PeriError::Provider(
            last_error.unwrap_or_else(|| "Anthropic stream failed".to_string()),
        ))
    }

    async fn stream_chunks(
        &self,
        request: CompletionRequest,
        sender: tokio::sync::mpsc::UnboundedSender<CompletionStreamChunk>,
    ) -> PeriResult<()> {
        let api_key = self
            .api_key
            .as_deref()
            .ok_or_else(|| PeriError::Provider("missing Anthropic API key".to_string()))?;
        let mut payload = anthropic_payload(&request);
        payload["stream"] = Value::Bool(true);
        let endpoint = format!("{}/v1/messages", self.base_url.trim_end_matches('/'));
        let mut last_error = None;
        for attempt in 0..=self.max_retries {
            let response = match self
                .client
                .post(&endpoint)
                .header("x-api-key", api_key)
                .header("anthropic-version", "2023-06-01")
                .header("content-type", "application/json")
                .header("accept", "text/event-stream")
                .json(&payload)
                .send()
                .await
            {
                Ok(response) => response,
                Err(err) => {
                    last_error = Some(format!("Anthropic stream request failed: {err}"));
                    if attempt < self.max_retries {
                        continue;
                    }
                    break;
                }
            };
            let status = response.status();
            if status.is_success() {
                return drive_anthropic_stream(response, self.pricing, sender).await;
            }
            let body = response.text().await.unwrap_or_default();
            last_error = Some(format!("Anthropic stream returned {status}: {body}"));
            if attempt < self.max_retries && should_retry_status(status) {
                continue;
            }
            break;
        }
        Err(PeriError::Provider(
            last_error.unwrap_or_else(|| "Anthropic stream failed".to_string()),
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
        if self.api_key.is_some() {
            AuthMethod::ApiKey
        } else {
            AuthMethod::NotConfigured
        }
    }
}

/// Builds an Anthropic Messages request body.
///
/// Native tool calling uses content blocks rather than the OpenAI-style flat
/// `tool_calls` array:
/// - Assistant turns mix `text` and `tool_use` content blocks.
/// - Tool results come back as a `user` turn whose content is one or more
///   `tool_result` blocks paired by `tool_use_id`.
///
/// Plain chat messages with no tool metadata stay as a single-string content
/// for backward compatibility with conversation histories that predate the
/// native protocol.
pub(crate) fn anthropic_payload(request: &CompletionRequest) -> Value {
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
            MessageRole::Assistant => {
                if message.tool_calls.is_empty() {
                    return Some(json!({
                        "role": "assistant",
                        "content": message.content,
                    }));
                }
                let mut blocks: Vec<Value> = Vec::new();
                if !message.content.is_empty() {
                    blocks.push(json!({
                        "type": "text",
                        "text": message.content,
                    }));
                }
                for call in &message.tool_calls {
                    blocks.push(json!({
                        "type": "tool_use",
                        "id": call.id,
                        "name": call.name,
                        "input": call.arguments,
                    }));
                }
                Some(json!({
                    "role": "assistant",
                    "content": blocks,
                }))
            }
            MessageRole::Tool => {
                if let Some(id) = message.tool_call_id.as_ref() {
                    Some(json!({
                        "role": "user",
                        "content": [{
                            "type": "tool_result",
                            "tool_use_id": id,
                            "content": message.content,
                        }],
                    }))
                } else {
                    // No id → treat as a plain user message. Same defensive fallback
                    // as the OpenAI path for callers that haven't migrated yet.
                    Some(json!({
                        "role": "user",
                        "content": message.content,
                    }))
                }
            }
            MessageRole::User => Some(json!({
                "role": "user",
                "content": message.content,
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

    if !request.tools.is_empty() {
        let tools = request
            .tools
            .iter()
            .map(|tool| {
                json!({
                    "name": tool.name,
                    "description": tool.description,
                    "input_schema": tool.parameters,
                })
            })
            .collect::<Vec<_>>();
        payload["tools"] = Value::Array(tools);
        payload["tool_choice"] = match request.tool_choice {
            ToolChoice::Auto => json!({ "type": "auto" }),
            ToolChoice::None => json!({ "type": "none" }),
            ToolChoice::Required => json!({ "type": "any" }),
        };
    }

    payload
}

pub(crate) fn parse_anthropic_response(
    body: &str,
    pricing: PricingTable,
) -> PeriResult<CompletionResponse> {
    let value = serde_json::from_str::<Value>(body)
        .map_err(|err| PeriError::Provider(format!("invalid Anthropic JSON: {err}")))?;
    let content = value
        .get("content")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let mut text = String::new();
    let mut tool_calls = Vec::new();
    for part in content {
        match part.get("type").and_then(Value::as_str) {
            Some("text") => {
                if let Some(chunk) = part.get("text").and_then(Value::as_str) {
                    text.push_str(chunk);
                }
            }
            Some("tool_use") => {
                if let Some(invocation) = parse_anthropic_tool_use(&part) {
                    tool_calls.push(invocation);
                }
            }
            _ => {}
        }
    }
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
        tool_calls,
        usage: Usage {
            input_tokens,
            output_tokens,
            cache_read_tokens,
            cache_creation_tokens,
            reasoning_output_tokens: 0,
            estimated_cost_usd,
        },
    })
}

fn parse_anthropic_tool_use(value: &Value) -> Option<ToolInvocation> {
    let id = value.get("id").and_then(Value::as_str)?.to_string();
    let name = value.get("name").and_then(Value::as_str)?.to_string();
    let arguments = value.get("input").cloned().unwrap_or(Value::Null);
    Some(ToolInvocation {
        id,
        name,
        arguments,
    })
}

#[derive(Default)]
struct AnthropicToolUseAccumulator {
    id: Option<String>,
    name: Option<String>,
    arguments_json: String,
}

/// Drives the Anthropic Messages SSE stream incrementally. `text_delta` events are
/// forwarded to `on_chunk` as they arrive so the TUI can render the model's reply
/// while it is still being generated. Tool-use `input_json_delta` fragments are
/// accumulated per content-block index and emitted as one parsed call on completion.
async fn drive_anthropic_stream(
    response: reqwest::Response,
    pricing: PricingTable,
    sender: tokio::sync::mpsc::UnboundedSender<CompletionStreamChunk>,
) -> PeriResult<()> {
    let mut input_tokens = 0u64;
    let mut output_tokens = 0u64;
    let mut cache_read_tokens = 0u64;
    let mut cache_creation_tokens = 0u64;
    let mut tool_accumulators: HashMap<u64, AnthropicToolUseAccumulator> = HashMap::new();
    stream_sse_events(response, |data| {
        if data == "[DONE]" {
            return Ok(());
        }
        let value = serde_json::from_str::<Value>(data)
            .map_err(|err| PeriError::Provider(format!("invalid Anthropic stream JSON: {err}")))?;
        match value.get("type").and_then(Value::as_str) {
            Some("message_start") => {
                if let Some(usage) = value
                    .get("message")
                    .and_then(|message| message.get("usage"))
                {
                    input_tokens = usage
                        .get("input_tokens")
                        .and_then(Value::as_u64)
                        .unwrap_or(input_tokens);
                    cache_read_tokens = usage
                        .get("cache_read_input_tokens")
                        .and_then(Value::as_u64)
                        .unwrap_or(cache_read_tokens);
                    cache_creation_tokens = usage
                        .get("cache_creation_input_tokens")
                        .and_then(Value::as_u64)
                        .unwrap_or(cache_creation_tokens);
                }
            }
            Some("content_block_start") => {
                let Some(index) = value.get("index").and_then(Value::as_u64) else {
                    return Ok(());
                };
                let Some(block) = value.get("content_block") else {
                    return Ok(());
                };
                if block.get("type").and_then(Value::as_str) == Some("tool_use") {
                    let entry = tool_accumulators.entry(index).or_default();
                    entry.id = block
                        .get("id")
                        .and_then(Value::as_str)
                        .map(str::to_string);
                    entry.name = block
                        .get("name")
                        .and_then(Value::as_str)
                        .map(str::to_string);
                }
            }
            Some("content_block_delta") => {
                let Some(index) = value.get("index").and_then(Value::as_u64) else {
                    return Ok(());
                };
                let Some(delta) = value.get("delta") else {
                    return Ok(());
                };
                match delta.get("type").and_then(Value::as_str) {
                    Some("text_delta") => {
                        if let Some(text) = delta.get("text").and_then(Value::as_str)
                            && !text.is_empty()
                        {
                            let _ = sender.send(CompletionStreamChunk {
                                delta: text.to_string(),
                                tool_calls: Vec::new(),
                                done: false,
                                usage: None,
                            });
                        }
                    }
                    Some("input_json_delta") => {
                        if let Some(partial) = delta.get("partial_json").and_then(Value::as_str) {
                            let entry = tool_accumulators.entry(index).or_default();
                            entry.arguments_json.push_str(partial);
                        }
                    }
                    _ => {}
                }
            }
            Some("message_delta") => {
                if let Some(usage) = value.get("usage") {
                    output_tokens = usage
                        .get("output_tokens")
                        .and_then(Value::as_u64)
                        .unwrap_or(output_tokens);
                }
            }
            _ => {}
        }
        Ok(())
    })
    .await?;

    let mut indices: Vec<u64> = tool_accumulators.keys().copied().collect();
    indices.sort_unstable();
    let assembled_tool_calls = indices
        .into_iter()
        .filter_map(|index| {
            let entry = tool_accumulators.remove(&index)?;
            let id = entry.id?;
            let name = entry.name?;
            let arguments = if entry.arguments_json.is_empty() {
                Value::Null
            } else {
                serde_json::from_str::<Value>(&entry.arguments_json).unwrap_or(Value::Null)
            };
            Some(ToolInvocation {
                id,
                name,
                arguments,
            })
        })
        .collect::<Vec<_>>();
    let estimated_cost_usd = estimate_cost(
        pricing,
        input_tokens + cache_creation_tokens,
        output_tokens,
        cache_read_tokens,
    );
    let _ = sender.send(CompletionStreamChunk {
        delta: String::new(),
        tool_calls: assembled_tool_calls,
        done: true,
        usage: Some(Usage {
            input_tokens,
            output_tokens,
            cache_read_tokens,
            cache_creation_tokens,
            reasoning_output_tokens: 0,
            estimated_cost_usd,
        }),
    });
    Ok(())
}

pub(crate) fn parse_anthropic_stream(
    body: &str,
    pricing: PricingTable,
) -> PeriResult<Vec<CompletionStreamChunk>> {
    let mut chunks = Vec::new();
    let mut input_tokens = 0;
    let mut output_tokens = 0;
    let mut cache_read_tokens = 0;
    let mut cache_creation_tokens = 0;
    let mut tool_accumulators: HashMap<u64, AnthropicToolUseAccumulator> = HashMap::new();

    for data in sse_data_events(body) {
        if data == "[DONE]" {
            break;
        }
        let value = serde_json::from_str::<Value>(&data)
            .map_err(|err| PeriError::Provider(format!("invalid Anthropic stream JSON: {err}")))?;
        match value.get("type").and_then(Value::as_str) {
            Some("message_start") => {
                if let Some(usage) = value
                    .get("message")
                    .and_then(|message| message.get("usage"))
                {
                    input_tokens = usage
                        .get("input_tokens")
                        .and_then(Value::as_u64)
                        .unwrap_or(input_tokens);
                    cache_read_tokens = usage
                        .get("cache_read_input_tokens")
                        .and_then(Value::as_u64)
                        .unwrap_or(cache_read_tokens);
                    cache_creation_tokens = usage
                        .get("cache_creation_input_tokens")
                        .and_then(Value::as_u64)
                        .unwrap_or(cache_creation_tokens);
                }
            }
            Some("content_block_start") => {
                let Some(index) = value.get("index").and_then(Value::as_u64) else {
                    continue;
                };
                let Some(block) = value.get("content_block") else {
                    continue;
                };
                if block.get("type").and_then(Value::as_str) == Some("tool_use") {
                    let entry = tool_accumulators.entry(index).or_default();
                    entry.id = block
                        .get("id")
                        .and_then(Value::as_str)
                        .map(str::to_string);
                    entry.name = block
                        .get("name")
                        .and_then(Value::as_str)
                        .map(str::to_string);
                }
            }
            Some("content_block_delta") => {
                let Some(index) = value.get("index").and_then(Value::as_u64) else {
                    continue;
                };
                let Some(delta) = value.get("delta") else {
                    continue;
                };
                match delta.get("type").and_then(Value::as_str) {
                    Some("text_delta") => {
                        if let Some(text) = delta.get("text").and_then(Value::as_str)
                            && !text.is_empty()
                        {
                            chunks.push(CompletionStreamChunk {
                                delta: text.to_string(),
                                tool_calls: Vec::new(),
                                done: false,
                                usage: None,
                            });
                        }
                    }
                    Some("input_json_delta") => {
                        if let Some(partial) = delta.get("partial_json").and_then(Value::as_str) {
                            let entry = tool_accumulators.entry(index).or_default();
                            entry.arguments_json.push_str(partial);
                        }
                    }
                    _ => {}
                }
            }
            Some("message_delta") => {
                if let Some(usage) = value.get("usage") {
                    output_tokens = usage
                        .get("output_tokens")
                        .and_then(Value::as_u64)
                        .unwrap_or(output_tokens);
                }
            }
            _ => {}
        }
    }

    let mut indices: Vec<u64> = tool_accumulators.keys().copied().collect();
    indices.sort_unstable();
    let assembled_tool_calls = indices
        .into_iter()
        .filter_map(|index| {
            let entry = tool_accumulators.remove(&index)?;
            let id = entry.id?;
            let name = entry.name?;
            let arguments = if entry.arguments_json.is_empty() {
                Value::Null
            } else {
                serde_json::from_str::<Value>(&entry.arguments_json).unwrap_or(Value::Null)
            };
            Some(ToolInvocation {
                id,
                name,
                arguments,
            })
        })
        .collect::<Vec<_>>();

    let estimated_cost_usd = estimate_cost(
        pricing,
        input_tokens + cache_creation_tokens,
        output_tokens,
        cache_read_tokens,
    );
    chunks.push(CompletionStreamChunk {
        delta: String::new(),
        tool_calls: assembled_tool_calls,
        done: true,
        usage: Some(Usage {
            input_tokens,
            output_tokens,
            cache_read_tokens,
            cache_creation_tokens,
            reasoning_output_tokens: 0,
            estimated_cost_usd,
        }),
    });
    Ok(chunks)
}
