use std::collections::HashMap;
use std::time::Duration;

use async_trait::async_trait;
use peridot_common::{PeriError, PeriResult};
use serde_json::{Value, json};

use crate::transport::{
    estimate_cost, read_streaming_response, should_retry_status, sse_data_events, stream_sse_events,
};
use crate::{
    AuthMethod, CompletionRequest, CompletionResponse, CompletionStreamChunk, LlmProvider,
    MessageRole, PricingTable, ToolChoice, ToolInvocation, Usage,
};

pub struct OpenAiProvider {
    auth_method: AuthMethod,
    model: String,
    api_key: Option<String>,
    base_url: String,
    client: reqwest::Client,
    max_retries: u8,
    pricing: PricingTable,
}

impl OpenAiProvider {
    /// Creates an OpenAI provider without credentials.
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
        Self::with_transport_options(model, api_key, base_url, auth_method, 120, 3)
    }

    /// Creates an OpenAI provider with explicit API transport options.
    pub fn with_transport_options(
        model: impl Into<String>,
        api_key: Option<String>,
        base_url: impl Into<String>,
        auth_method: AuthMethod,
        timeout_seconds: u64,
        max_retries: u8,
    ) -> Self {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(timeout_seconds.max(1)))
            .build()
            .unwrap_or_else(|_| reqwest::Client::new());
        Self {
            auth_method,
            model: model.into(),
            api_key,
            base_url: base_url.into(),
            client,
            max_retries,
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

    /// Returns configured retry count.
    pub fn max_retries(&self) -> u8 {
        self.max_retries
    }
}

#[async_trait]
impl LlmProvider for OpenAiProvider {
    async fn complete(&self, request: CompletionRequest) -> PeriResult<CompletionResponse> {
        let api_key = self
            .api_key
            .as_deref()
            .ok_or_else(|| PeriError::Provider("missing OpenAI API key".to_string()))?;
        let payload = openai_chat_payload(&request);
        let endpoint = format!(
            "{}/v1/chat/completions",
            self.base_url.trim_end_matches('/')
        );
        let mut last_error = None;
        for attempt in 0..=self.max_retries {
            let response = match self
                .client
                .post(&endpoint)
                .bearer_auth(api_key)
                .header("content-type", "application/json")
                .json(&payload)
                .send()
                .await
            {
                Ok(response) => response,
                Err(err) => {
                    last_error = Some(format!("OpenAI request failed: {err}"));
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
                    last_error = Some(format!("OpenAI response read failed: {err}"));
                    if attempt < self.max_retries {
                        continue;
                    }
                    break;
                }
            };
            if status.is_success() {
                return parse_openai_response(&body, self.pricing);
            }
            last_error = Some(format!("OpenAI request returned {status}: {body}"));
            if attempt < self.max_retries && should_retry_status(status) {
                continue;
            }
            break;
        }
        Err(PeriError::Provider(
            last_error.unwrap_or_else(|| "OpenAI request failed".to_string()),
        ))
    }

    async fn stream(&self, request: CompletionRequest) -> PeriResult<Vec<CompletionStreamChunk>> {
        let api_key = self
            .api_key
            .as_deref()
            .ok_or_else(|| PeriError::Provider("missing OpenAI API key".to_string()))?;
        let payload = openai_stream_payload(&request);
        let endpoint = format!(
            "{}/v1/chat/completions",
            self.base_url.trim_end_matches('/')
        );
        let mut last_error = None;
        for attempt in 0..=self.max_retries {
            let response = match self
                .client
                .post(&endpoint)
                .bearer_auth(api_key)
                .header("content-type", "application/json")
                .header("accept", "text/event-stream")
                .json(&payload)
                .send()
                .await
            {
                Ok(response) => response,
                Err(err) => {
                    last_error = Some(format!("OpenAI stream request failed: {err}"));
                    if attempt < self.max_retries {
                        continue;
                    }
                    break;
                }
            };

            let status = response.status();
            if status.is_success() {
                let body = read_streaming_response(response).await?;
                return parse_openai_stream(&body, self.pricing);
            }
            let body = response.text().await.unwrap_or_default();
            last_error = Some(format!("OpenAI stream returned {status}: {body}"));
            if attempt < self.max_retries && should_retry_status(status) {
                continue;
            }
            break;
        }
        Err(PeriError::Provider(
            last_error.unwrap_or_else(|| "OpenAI stream failed".to_string()),
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
            .ok_or_else(|| PeriError::Provider("missing OpenAI API key".to_string()))?;
        let payload = openai_stream_payload(&request);
        let endpoint = format!(
            "{}/v1/chat/completions",
            self.base_url.trim_end_matches('/')
        );
        let mut last_error = None;
        for attempt in 0..=self.max_retries {
            let response = match self
                .client
                .post(&endpoint)
                .bearer_auth(api_key)
                .header("content-type", "application/json")
                .header("accept", "text/event-stream")
                .json(&payload)
                .send()
                .await
            {
                Ok(response) => response,
                Err(err) => {
                    last_error = Some(format!("OpenAI stream request failed: {err}"));
                    if attempt < self.max_retries {
                        continue;
                    }
                    break;
                }
            };
            let status = response.status();
            if status.is_success() {
                return drive_openai_stream(response, self.pricing, sender).await;
            }
            let body = response.text().await.unwrap_or_default();
            last_error = Some(format!("OpenAI stream returned {status}: {body}"));
            if attempt < self.max_retries && should_retry_status(status) {
                continue;
            }
            break;
        }
        Err(PeriError::Provider(
            last_error.unwrap_or_else(|| "OpenAI stream failed".to_string()),
        ))
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
        if self.api_key.is_none() {
            AuthMethod::NotConfigured
        } else {
            self.auth_method.clone()
        }
    }
}

/// Builds a Chat Completions request body from a provider-neutral request.
///
/// Emits the canonical OpenAI wire format end-to-end:
/// - System prompts are merged into a single leading `role: "system"` message.
/// - Assistant turns that carried tool calls are emitted as `role: "assistant"`
///   with a `tool_calls` array, exactly the shape the model was trained on.
/// - Tool results are emitted as `role: "tool"` with a `tool_call_id` linking back
///   to the originating assistant call. This is the linkage that lets the model
///   recognise its own prior actions and avoid re-running them.
pub(crate) fn openai_chat_payload(request: &CompletionRequest) -> Value {
    let mut messages = Vec::new();
    let mut system_parts = Vec::new();
    if let Some(system) = &request.system {
        system_parts.push(system.clone());
    }
    for message in &request.messages {
        if matches!(message.role, MessageRole::System) {
            system_parts.push(message.content.clone());
        }
    }
    if !system_parts.is_empty() {
        messages.push(json!({
            "role": "system",
            "content": system_parts.join("\n\n"),
        }));
    }
    for message in &request.messages {
        match message.role {
            MessageRole::System => continue,
            MessageRole::Assistant => {
                let mut entry = serde_json::Map::new();
                entry.insert("role".to_string(), Value::String("assistant".to_string()));
                // OpenAI requires `content` to be string-or-null. When the
                // assistant emitted only tool calls we surface null so the
                // server-side validator accepts the message; otherwise we ship
                // the text the model produced (possibly empty after trimming).
                if message.content.is_empty() && !message.tool_calls.is_empty() {
                    entry.insert("content".to_string(), Value::Null);
                } else {
                    entry.insert(
                        "content".to_string(),
                        Value::String(message.content.clone()),
                    );
                }
                if !message.tool_calls.is_empty() {
                    let tool_calls = message
                        .tool_calls
                        .iter()
                        .map(|call| {
                            // OpenAI expects `arguments` as a JSON-encoded string,
                            // not a parsed object. We keep our internal Value form
                            // for ergonomic params, then serialise here.
                            let arguments = serde_json::to_string(&call.arguments)
                                .unwrap_or_else(|_| "{}".to_string());
                            json!({
                                "id": call.id,
                                "type": "function",
                                "function": {
                                    "name": call.name,
                                    "arguments": arguments,
                                },
                            })
                        })
                        .collect::<Vec<_>>();
                    entry.insert("tool_calls".to_string(), Value::Array(tool_calls));
                }
                messages.push(Value::Object(entry));
            }
            MessageRole::Tool => {
                if let Some(id) = message.tool_call_id.as_ref() {
                    messages.push(json!({
                        "role": "tool",
                        "tool_call_id": id,
                        "content": message.content,
                    }));
                } else {
                    // Defensive fallback. Pre-native callers can still ship tool
                    // observations as plain user content; this branch shouldn't
                    // fire for messages built by `ContextManager::to_messages`.
                    messages.push(json!({
                        "role": "user",
                        "content": message.content,
                    }));
                }
            }
            MessageRole::User => {
                if message.images.is_empty() {
                    messages.push(json!({
                        "role": "user",
                        "content": message.content,
                    }));
                } else {
                    // Multimodal turn: text part (if any) then image_url parts
                    // carrying base64 data URLs.
                    let mut parts: Vec<Value> = Vec::new();
                    if !message.content.is_empty() {
                        parts.push(json!({"type": "text", "text": message.content}));
                    }
                    for image in &message.images {
                        parts.push(json!({
                            "type": "image_url",
                            "image_url": {
                                "url": format!(
                                    "data:{};base64,{}",
                                    image.media_type, image.data
                                ),
                            },
                        }));
                    }
                    messages.push(json!({
                        "role": "user",
                        "content": parts,
                    }));
                }
            }
        }
    }

    let model = request
        .model
        .strip_suffix("-fast")
        .unwrap_or(&request.model)
        .to_string();
    let mut payload = json!({
        "model": model,
        "messages": messages,
    });
    if let Some(max_tokens) = request.max_tokens {
        payload["max_tokens"] = json!(max_tokens);
    }
    // Forward reasoning intensity for o-series / gpt-5 models. Chat-only
    // models ignore the field silently, so it's safe to send unconditionally
    // when the operator opts in. Legacy `thinking: true` callers fall
    // through to Medium when they leave `reasoning_effort` at its default.
    let effective_effort = if request.reasoning_effort != peridot_common::ReasoningEffort::Off {
        request.reasoning_effort
    } else if request.thinking {
        peridot_common::ReasoningEffort::Medium
    } else {
        peridot_common::ReasoningEffort::Off
    };
    if let Some(label) = effective_effort.openai_effort_label() {
        payload["reasoning"] = json!({ "effort": label });
    }
    if let Some(service_tier) = request
        .service_tier
        .as_deref()
        .and_then(crate::openai_codex::normalize_service_tier)
    {
        payload["service_tier"] = json!(service_tier);
    }
    if !request.tools.is_empty() {
        let tools = request
            .tools
            .iter()
            .map(|tool| {
                json!({
                    "type": "function",
                    "function": {
                        "name": tool.name,
                        "description": tool.description,
                        "parameters": tool.parameters,
                    }
                })
            })
            .collect::<Vec<_>>();
        payload["tools"] = Value::Array(tools);
        payload["tool_choice"] = match request.tool_choice {
            ToolChoice::Auto => Value::String("auto".to_string()),
            ToolChoice::None => Value::String("none".to_string()),
            ToolChoice::Required => Value::String("required".to_string()),
        };
    }
    payload
}

/// Builds the OpenAI/OpenRouter streaming Chat Completions request body.
///
/// OpenRouter accepts the same OpenAI-compatible payload shape and endpoint
/// suffix, so this helper deliberately stays provider-neutral at the wire level.
pub(crate) fn openai_stream_payload(request: &CompletionRequest) -> Value {
    let mut payload = openai_chat_payload(request);
    payload["stream"] = Value::Bool(true);
    payload["stream_options"] = json!({ "include_usage": true });
    payload
}

pub(crate) fn parse_openai_response(
    body: &str,
    pricing: PricingTable,
) -> PeriResult<CompletionResponse> {
    let value = serde_json::from_str::<Value>(body)
        .map_err(|err| PeriError::Provider(format!("invalid OpenAI JSON: {err}")))?;
    let message = value
        .get("choices")
        .and_then(Value::as_array)
        .and_then(|choices| choices.first())
        .and_then(|choice| choice.get("message"));
    let text = message
        .and_then(|message| message.get("content"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let tool_calls = message
        .and_then(|message| message.get("tool_calls"))
        .and_then(Value::as_array)
        .map(|calls| {
            calls
                .iter()
                .filter_map(parse_tool_call_object)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let usage_value = value.get("usage").unwrap_or(&Value::Null);
    let input_tokens = usage_value
        .get("prompt_tokens")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let output_tokens = usage_value
        .get("completion_tokens")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let cache_read_tokens = usage_value
        .get("prompt_tokens_details")
        .and_then(|details| details.get("cached_tokens"))
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let reasoning_output_tokens = usage_value
        .get("completion_tokens_details")
        .and_then(|details| details.get("reasoning_tokens"))
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let estimated_cost_usd = estimate_cost(pricing, input_tokens, output_tokens, cache_read_tokens);

    Ok(CompletionResponse {
        text,
        tool_calls,
        reasoning_content: None,
        usage: Usage {
            input_tokens,
            output_tokens,
            cache_read_tokens,
            cache_creation_tokens: 0,
            reasoning_output_tokens,
            estimated_cost_usd,
        },
    })
}

fn parse_tool_call_object(value: &Value) -> Option<ToolInvocation> {
    let id = value.get("id").and_then(Value::as_str)?.to_string();
    let function = value.get("function")?;
    let name = function.get("name").and_then(Value::as_str)?.to_string();
    let arguments = function
        .get("arguments")
        .and_then(Value::as_str)
        .and_then(|raw| serde_json::from_str::<Value>(raw).ok())
        .unwrap_or(Value::Null);
    Some(ToolInvocation {
        id,
        name,
        arguments,
    })
}

/// Assembles streaming tool_call deltas. OpenAI streams partial fragments keyed by
/// `index`; we accumulate name and argument JSON-string fragments per index, then
/// finalize on stream completion.
#[derive(Default)]
struct ToolCallAccumulator {
    id: Option<String>,
    name: String,
    arguments: String,
}

/// Drives the OpenAI Chat Completions SSE stream incrementally. Each `data:` event
/// is parsed in isolation so text deltas are pushed to `on_chunk` the moment they
/// arrive — that is what gives the TUI its character-by-character typing effect.
/// Tool-call deltas are accumulated across events because OpenAI splits a single
/// call's `arguments` JSON over many fragments; they are emitted only on stream
/// completion, alongside final usage accounting.
async fn drive_openai_stream(
    response: reqwest::Response,
    pricing: PricingTable,
    sender: tokio::sync::mpsc::UnboundedSender<CompletionStreamChunk>,
) -> PeriResult<()> {
    let mut tool_accumulators: HashMap<u64, ToolCallAccumulator> = HashMap::new();
    let mut input_tokens = 0u64;
    let mut output_tokens = 0u64;
    let mut cache_read_tokens = 0u64;
    let mut reasoning_output_tokens = 0u64;
    stream_sse_events(response, |data| {
        if data == "[DONE]" {
            return Ok(());
        }
        let value = serde_json::from_str::<Value>(data)
            .map_err(|err| PeriError::Provider(format!("invalid OpenAI stream JSON: {err}")))?;
        if let Some(usage) = value.get("usage") {
            input_tokens = usage
                .get("prompt_tokens")
                .and_then(Value::as_u64)
                .unwrap_or(input_tokens);
            output_tokens = usage
                .get("completion_tokens")
                .and_then(Value::as_u64)
                .unwrap_or(output_tokens);
            cache_read_tokens = usage
                .get("prompt_tokens_details")
                .and_then(|details| details.get("cached_tokens"))
                .and_then(Value::as_u64)
                .unwrap_or(cache_read_tokens);
            reasoning_output_tokens = usage
                .get("completion_tokens_details")
                .and_then(|details| details.get("reasoning_tokens"))
                .and_then(Value::as_u64)
                .unwrap_or(reasoning_output_tokens);
        }
        if let Some(choice) = value
            .get("choices")
            .and_then(Value::as_array)
            .and_then(|choices| choices.first())
        {
            let delta = choice.get("delta").unwrap_or(&Value::Null);
            if let Some(text) = delta.get("content").and_then(Value::as_str)
                && !text.is_empty()
            {
                let _ = sender.send(CompletionStreamChunk {
                    delta: text.to_string(),
                    reasoning_delta: String::new(),
                    tool_calls: Vec::new(),
                    done: false,
                    usage: None,
                });
            }
            if let Some(tool_calls) = delta.get("tool_calls").and_then(Value::as_array) {
                for partial in tool_calls {
                    let Some(index) = partial.get("index").and_then(Value::as_u64) else {
                        continue;
                    };
                    let entry = tool_accumulators.entry(index).or_default();
                    if let Some(id) = partial.get("id").and_then(Value::as_str)
                        && entry.id.is_none()
                    {
                        entry.id = Some(id.to_string());
                    }
                    if let Some(function) = partial.get("function") {
                        if let Some(name) = function.get("name").and_then(Value::as_str) {
                            entry.name.push_str(name);
                        }
                        if let Some(arguments) = function.get("arguments").and_then(Value::as_str) {
                            entry.arguments.push_str(arguments);
                        }
                    }
                }
            }
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
            let id = entry.id.unwrap_or_else(|| format!("tool_call_{index}"));
            if entry.name.is_empty() {
                return None;
            }
            let arguments = if entry.arguments.is_empty() {
                Value::Null
            } else {
                serde_json::from_str::<Value>(&entry.arguments).unwrap_or(Value::Null)
            };
            Some(ToolInvocation {
                id,
                name: entry.name,
                arguments,
            })
        })
        .collect::<Vec<_>>();
    let estimated_cost_usd = estimate_cost(pricing, input_tokens, output_tokens, cache_read_tokens);
    let _ = sender.send(CompletionStreamChunk {
        delta: String::new(),
        reasoning_delta: String::new(),
        tool_calls: assembled_tool_calls,
        done: true,
        usage: Some(Usage {
            input_tokens,
            output_tokens,
            cache_read_tokens,
            cache_creation_tokens: 0,
            reasoning_output_tokens,
            estimated_cost_usd,
        }),
    });
    Ok(())
}

pub(crate) fn parse_openai_stream(
    body: &str,
    pricing: PricingTable,
) -> PeriResult<Vec<CompletionStreamChunk>> {
    let mut chunks = Vec::new();
    let mut input_tokens = 0;
    let mut output_tokens = 0;
    let mut cache_read_tokens = 0;
    let mut reasoning_output_tokens = 0;
    let mut tool_accumulators: HashMap<u64, ToolCallAccumulator> = HashMap::new();

    for data in sse_data_events(body) {
        if data == "[DONE]" {
            break;
        }
        let value = serde_json::from_str::<Value>(&data)
            .map_err(|err| PeriError::Provider(format!("invalid OpenAI stream JSON: {err}")))?;
        if let Some(usage) = value.get("usage") {
            input_tokens = usage
                .get("prompt_tokens")
                .and_then(Value::as_u64)
                .unwrap_or(input_tokens);
            output_tokens = usage
                .get("completion_tokens")
                .and_then(Value::as_u64)
                .unwrap_or(output_tokens);
            cache_read_tokens = usage
                .get("prompt_tokens_details")
                .and_then(|details| details.get("cached_tokens"))
                .and_then(Value::as_u64)
                .unwrap_or(cache_read_tokens);
            reasoning_output_tokens = usage
                .get("completion_tokens_details")
                .and_then(|details| details.get("reasoning_tokens"))
                .and_then(Value::as_u64)
                .unwrap_or(reasoning_output_tokens);
        }
        let Some(choice) = value
            .get("choices")
            .and_then(Value::as_array)
            .and_then(|choices| choices.first())
        else {
            continue;
        };
        let delta = choice.get("delta").unwrap_or(&Value::Null);
        if let Some(text) = delta.get("content").and_then(Value::as_str)
            && !text.is_empty()
        {
            chunks.push(CompletionStreamChunk {
                delta: text.to_string(),
                reasoning_delta: String::new(),
                tool_calls: Vec::new(),
                done: false,
                usage: None,
            });
        }
        if let Some(tool_calls) = delta.get("tool_calls").and_then(Value::as_array) {
            for partial in tool_calls {
                let Some(index) = partial.get("index").and_then(Value::as_u64) else {
                    continue;
                };
                let entry = tool_accumulators.entry(index).or_default();
                if let Some(id) = partial.get("id").and_then(Value::as_str)
                    && entry.id.is_none()
                {
                    entry.id = Some(id.to_string());
                }
                if let Some(function) = partial.get("function") {
                    if let Some(name) = function.get("name").and_then(Value::as_str) {
                        entry.name.push_str(name);
                    }
                    if let Some(arguments) = function.get("arguments").and_then(Value::as_str) {
                        entry.arguments.push_str(arguments);
                    }
                }
            }
        }
    }

    let mut indices: Vec<u64> = tool_accumulators.keys().copied().collect();
    indices.sort_unstable();
    let assembled_tool_calls = indices
        .into_iter()
        .filter_map(|index| {
            let entry = tool_accumulators.remove(&index)?;
            let id = entry.id.unwrap_or_else(|| format!("tool_call_{index}"));
            if entry.name.is_empty() {
                return None;
            }
            let arguments = if entry.arguments.is_empty() {
                Value::Null
            } else {
                serde_json::from_str::<Value>(&entry.arguments).unwrap_or(Value::Null)
            };
            Some(ToolInvocation {
                id,
                name: entry.name,
                arguments,
            })
        })
        .collect::<Vec<_>>();

    let estimated_cost_usd = estimate_cost(pricing, input_tokens, output_tokens, cache_read_tokens);
    chunks.push(CompletionStreamChunk {
        delta: String::new(),
        reasoning_delta: String::new(),
        tool_calls: assembled_tool_calls,
        done: true,
        usage: Some(Usage {
            input_tokens,
            output_tokens,
            cache_read_tokens,
            cache_creation_tokens: 0,
            reasoning_output_tokens,
            estimated_cost_usd,
        }),
    });
    Ok(chunks)
}
