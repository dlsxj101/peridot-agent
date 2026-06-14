use std::collections::HashMap;
use std::time::Duration;

use async_trait::async_trait;
use peridot_common::{PeriError, PeriResult};
use serde_json::{Value, json};

use crate::transport::{
    backoff_before_retry, estimate_cost, should_retry_status, stream_sse_events,
};
use crate::{
    AuthMethod, CompletionRequest, CompletionResponse, CompletionStreamChunk, LlmProvider,
    MessageRole, PricingTable, ToolChoice, ToolInvocation, Usage,
};

const DEFAULT_CODEX_BASE_URL: &str = "https://chatgpt.com/backend-api/codex";

/// OpenAI Codex provider using ChatGPT/Codex OAuth subscription transport.
///
/// This is intentionally separate from [`crate::OpenAiProvider`]. Codex OAuth
/// access tokens are not OpenAI Platform API keys; they are sent to the
/// ChatGPT Codex backend (`/backend-api/codex/responses`) with the matching
/// ChatGPT account id.
pub struct OpenAiCodexProvider {
    model: String,
    access_token: String,
    account_id: String,
    base_url: String,
    client: reqwest::Client,
    max_retries: u8,
    pricing: PricingTable,
}

impl OpenAiCodexProvider {
    /// Creates a Codex OAuth provider with the default ChatGPT Codex backend.
    pub fn new(
        model: impl Into<String>,
        access_token: impl Into<String>,
        account_id: impl Into<String>,
    ) -> Self {
        Self::with_transport_options(
            model,
            access_token,
            account_id,
            DEFAULT_CODEX_BASE_URL,
            120,
            3,
        )
    }

    /// Creates a Codex OAuth provider with explicit transport options.
    pub fn with_transport_options(
        model: impl Into<String>,
        access_token: impl Into<String>,
        account_id: impl Into<String>,
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
            access_token: access_token.into(),
            account_id: account_id.into(),
            base_url: base_url.into(),
            client,
            max_retries,
            pricing: PricingTable::default(),
        }
    }

    /// Returns the configured backend base URL.
    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    fn endpoint(&self) -> String {
        openai_codex_responses_url(&self.base_url)
    }
}

#[async_trait]
impl LlmProvider for OpenAiCodexProvider {
    async fn complete(&self, request: CompletionRequest) -> PeriResult<CompletionResponse> {
        let chunks = self.stream(request).await?;
        let mut text = String::new();
        let mut reasoning = String::new();
        let mut tool_calls = Vec::new();
        let mut usage = Usage::default();
        for chunk in chunks {
            text.push_str(&chunk.delta);
            reasoning.push_str(&chunk.reasoning_delta);
            if !chunk.tool_calls.is_empty() {
                tool_calls = chunk.tool_calls;
            }
            if let Some(chunk_usage) = chunk.usage {
                usage = chunk_usage;
            }
        }
        Ok(CompletionResponse {
            text,
            tool_calls,
            reasoning_content: (!reasoning.is_empty()).then_some(reasoning),
            usage,
        })
    }

    async fn stream(&self, request: CompletionRequest) -> PeriResult<Vec<CompletionStreamChunk>> {
        let response = self.send_streaming_request(request).await?;
        let mut chunks = Vec::new();
        drive_openai_codex_stream(response, self.pricing, |chunk| {
            chunks.push(chunk);
            Ok(())
        })
        .await?;
        Ok(chunks)
    }

    async fn stream_chunks(
        &self,
        request: CompletionRequest,
        sender: tokio::sync::mpsc::UnboundedSender<CompletionStreamChunk>,
    ) -> PeriResult<()> {
        let response = self.send_streaming_request(request).await?;
        drive_openai_codex_stream(response, self.pricing, |chunk| {
            if sender.send(chunk).is_err() {
                return Err(PeriError::Provider(
                    "OpenAI Codex stream receiver closed".to_string(),
                ));
            }
            Ok(())
        })
        .await
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
        if self.access_token.is_empty() {
            AuthMethod::NotConfigured
        } else {
            AuthMethod::OAuth
        }
    }
}

impl OpenAiCodexProvider {
    async fn send_streaming_request(
        &self,
        mut request: CompletionRequest,
    ) -> PeriResult<reqwest::Response> {
        if self.access_token.trim().is_empty() {
            return Err(PeriError::Provider(
                "missing OpenAI Codex OAuth access token".to_string(),
            ));
        }
        if self.account_id.trim().is_empty() {
            return Err(PeriError::Provider(
                "missing OpenAI Codex ChatGPT account id".to_string(),
            ));
        }
        if request.model.trim().is_empty() {
            request.model = self.model.clone();
        }
        let payload = openai_codex_payload(&request);
        let endpoint = self.endpoint();
        let mut last_error = None;
        for attempt in 0..=self.max_retries {
            // Back off before every retry after the first attempt.
            if attempt > 0 {
                backoff_before_retry(u32::from(attempt)).await;
            }
            let response = match self
                .client
                .post(&endpoint)
                .bearer_auth(&self.access_token)
                .header("chatgpt-account-id", &self.account_id)
                .header("originator", "peridot")
                .header("OpenAI-Beta", "responses=experimental")
                .header("accept", "text/event-stream")
                .header("content-type", "application/json")
                .json(&payload)
                .send()
                .await
            {
                Ok(response) => response,
                Err(err) => {
                    last_error = Some(format!("OpenAI Codex request failed: {err}"));
                    if attempt < self.max_retries {
                        continue;
                    }
                    break;
                }
            };

            let status = response.status();
            if status.is_success() {
                return Ok(response);
            }
            let body = response.text().await.unwrap_or_default();
            last_error = Some(format_openai_codex_error(status, &body));
            if attempt < self.max_retries && should_retry_status(status) {
                continue;
            }
            break;
        }
        Err(PeriError::Provider(last_error.unwrap_or_else(|| {
            "OpenAI Codex request failed".to_string()
        })))
    }
}

pub(crate) fn openai_codex_responses_url(base_url: &str) -> String {
    let normalized = if base_url.trim().is_empty() {
        DEFAULT_CODEX_BASE_URL
    } else {
        base_url.trim()
    }
    .trim_end_matches('/');
    if normalized.ends_with("/codex/responses") {
        normalized.to_string()
    } else if normalized.ends_with("/codex") {
        format!("{normalized}/responses")
    } else {
        format!("{normalized}/codex/responses")
    }
}

pub(crate) fn openai_codex_payload(request: &CompletionRequest) -> Value {
    let mut instructions = Vec::new();
    if let Some(system) = &request.system
        && !system.trim().is_empty()
    {
        instructions.push(system.clone());
    }
    for message in &request.messages {
        if matches!(message.role, MessageRole::System) && !message.content.trim().is_empty() {
            instructions.push(message.content.clone());
        }
    }

    let mut input = Vec::new();
    for message in &request.messages {
        match message.role {
            MessageRole::System => {}
            MessageRole::User => {
                let mut content = vec![json!({"type": "input_text", "text": message.content})];
                // Responses API image parts (multimodal input).
                for image in &message.images {
                    content.push(json!({
                        "type": "input_image",
                        "image_url": format!("data:{};base64,{}", image.media_type, image.data),
                    }));
                }
                input.push(json!({
                    "type": "message",
                    "role": "user",
                    "content": content,
                }));
            }
            MessageRole::Assistant => {
                if !message.content.is_empty() {
                    input.push(json!({
                        "type": "message",
                        "role": "assistant",
                        "content": [{"type": "output_text", "text": message.content}],
                    }));
                }
                for call in &message.tool_calls {
                    input.push(json!({
                        "type": "function_call",
                        "call_id": call.id,
                        "name": call.name,
                        "arguments": serde_json::to_string(&call.arguments)
                            .unwrap_or_else(|_| "{}".to_string()),
                    }));
                }
            }
            MessageRole::Tool => {
                if let Some(call_id) = &message.tool_call_id {
                    input.push(json!({
                        "type": "function_call_output",
                        "call_id": call_id,
                        "output": message.content,
                    }));
                } else {
                    input.push(json!({
                        "type": "message",
                        "role": "user",
                        "content": [{"type": "input_text", "text": message.content}],
                    }));
                }
            }
        }
    }

    let (model, service_tier) =
        codex_model_and_service_tier(&request.model, request.service_tier.as_deref());
    let mut payload = json!({
        "model": model,
        "store": false,
        "stream": true,
        "instructions": if instructions.is_empty() {
            "You are a helpful coding assistant.".to_string()
        } else {
            instructions.join("\n\n")
        },
        "input": input,
        "text": { "verbosity": "low" },
        "include": ["reasoning.encrypted_content"],
        "tool_choice": "auto",
        "parallel_tool_calls": false,
    });
    // ChatGPT's Codex OAuth backend currently rejects `max_output_tokens`
    // even though the public Responses API accepts it. Let the backend apply
    // its native limits instead of failing every turn before generation.
    let effective_effort = if request.reasoning_effort != peridot_common::ReasoningEffort::Off {
        request.reasoning_effort
    } else if request.thinking {
        peridot_common::ReasoningEffort::Medium
    } else {
        peridot_common::ReasoningEffort::Off
    };
    if let Some(label) = effective_effort.openai_effort_label() {
        payload["reasoning"] = json!({ "effort": label, "summary": "auto" });
    }
    if let Some(service_tier) = service_tier {
        payload["service_tier"] = json!(service_tier);
    }
    if !request.tools.is_empty() {
        payload["tools"] = Value::Array(
            request
                .tools
                .iter()
                .map(|tool| {
                    json!({
                        "type": "function",
                        "name": tool.name,
                        "description": tool.description,
                        "parameters": tool.parameters,
                    })
                })
                .collect(),
        );
        payload["tool_choice"] = match request.tool_choice {
            ToolChoice::Auto => Value::String("auto".to_string()),
            ToolChoice::None => Value::String("none".to_string()),
            ToolChoice::Required => Value::String("required".to_string()),
        };
    }
    payload
}

pub(crate) fn codex_model_and_service_tier(
    model: &str,
    configured_tier: Option<&str>,
) -> (String, Option<&'static str>) {
    let trimmed = model.trim();
    let lower = trimmed.to_ascii_lowercase();
    let (base_model, alias_tier) = if lower.ends_with("-fast") {
        (
            &trimmed[..trimmed.len().saturating_sub("-fast".len())],
            Some("priority"),
        )
    } else {
        (trimmed, None)
    };
    let tier = configured_tier
        .and_then(normalize_service_tier)
        .or(alias_tier);
    let model = if base_model.is_empty() {
        "gpt-5.4".to_string()
    } else {
        base_model.to_string()
    };
    (model, tier)
}

pub(crate) fn normalize_service_tier(value: &str) -> Option<&'static str> {
    match value.trim().to_ascii_lowercase().as_str() {
        "" | "off" | "none" | "standard" | "default" => None,
        "fast" | "priority" => Some("priority"),
        _ => None,
    }
}

async fn drive_openai_codex_stream<F>(
    response: reqwest::Response,
    pricing: PricingTable,
    mut on_chunk: F,
) -> PeriResult<()>
where
    F: FnMut(CompletionStreamChunk) -> PeriResult<()>,
{
    let mut text = String::new();
    let mut reasoning = String::new();
    let mut tool_calls: HashMap<String, ToolInvocation> = HashMap::new();
    let mut usage = Usage::default();
    stream_sse_events(response, |data| {
        let value = serde_json::from_str::<Value>(data).map_err(|err| {
            PeriError::Provider(format!("invalid OpenAI Codex stream JSON: {err}"))
        })?;
        let event_type = value
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or_default();
        match event_type {
            "error" => {
                return Err(PeriError::Provider(format!(
                    "OpenAI Codex error: {}",
                    value
                        .get("message")
                        .and_then(Value::as_str)
                        .or_else(|| value.get("code").and_then(Value::as_str))
                        .unwrap_or("unknown error")
                )));
            }
            "response.failed" => {
                return Err(PeriError::Provider(format!(
                    "OpenAI Codex response failed: {}",
                    value
                        .pointer("/response/error/message")
                        .and_then(Value::as_str)
                        .or_else(|| value
                            .pointer("/response/error/code")
                            .and_then(Value::as_str))
                        .unwrap_or("unknown error")
                )));
            }
            "response.output_text.delta" | "response.output_item.output_text.delta" => {
                if let Some(delta) = value.get("delta").and_then(Value::as_str) {
                    text.push_str(delta);
                    on_chunk(CompletionStreamChunk {
                        delta: delta.to_string(),
                        reasoning_delta: String::new(),
                        tool_calls: Vec::new(),
                        done: false,
                        usage: None,
                    })?;
                }
            }
            "response.reasoning_summary_text.delta" | "response.reasoning_text.delta" => {
                if let Some(delta) = value.get("delta").and_then(Value::as_str) {
                    reasoning.push_str(delta);
                    on_chunk(CompletionStreamChunk {
                        delta: String::new(),
                        reasoning_delta: delta.to_string(),
                        tool_calls: Vec::new(),
                        done: false,
                        usage: None,
                    })?;
                }
            }
            "response.output_item.done" => {
                if let Some(call) = value.get("item").and_then(parse_codex_function_call) {
                    tool_calls.insert(call.id.clone(), call);
                }
            }
            "response.completed" | "response.done" | "response.incomplete" => {
                if let Some(response) = value.get("response") {
                    collect_codex_response_output(
                        response,
                        &mut text,
                        &mut reasoning,
                        &mut tool_calls,
                    );
                    usage =
                        parse_codex_usage(response.get("usage").unwrap_or(&Value::Null), pricing);
                }
            }
            _ => {}
        }
        Ok(())
    })
    .await?;

    on_chunk(CompletionStreamChunk {
        delta: String::new(),
        reasoning_delta: String::new(),
        tool_calls: tool_calls.into_values().collect(),
        done: true,
        usage: Some(usage),
    })
}

fn collect_codex_response_output(
    response: &Value,
    text: &mut String,
    reasoning: &mut String,
    tool_calls: &mut HashMap<String, ToolInvocation>,
) {
    let Some(output) = response.get("output").and_then(Value::as_array) else {
        return;
    };
    for item in output {
        if let Some(call) = parse_codex_function_call(item) {
            tool_calls.insert(call.id.clone(), call);
            continue;
        }
        if item.get("type").and_then(Value::as_str) == Some("reasoning") {
            if let Some(summary) = item.get("summary").and_then(Value::as_array) {
                for part in summary {
                    if let Some(part_text) = part.get("text").and_then(Value::as_str) {
                        reasoning.push_str(part_text);
                    }
                }
            }
            continue;
        }
        let Some(content) = item.get("content").and_then(Value::as_array) else {
            continue;
        };
        for part in content {
            let part_type = part.get("type").and_then(Value::as_str).unwrap_or_default();
            if matches!(part_type, "output_text" | "text")
                && let Some(part_text) = part.get("text").and_then(Value::as_str)
                && !text.contains(part_text)
            {
                text.push_str(part_text);
            }
        }
    }
}

fn parse_codex_function_call(value: &Value) -> Option<ToolInvocation> {
    if value.get("type").and_then(Value::as_str) != Some("function_call") {
        return None;
    }
    let id = value
        .get("call_id")
        .or_else(|| value.get("id"))
        .and_then(Value::as_str)?
        .to_string();
    let name = value.get("name").and_then(Value::as_str)?.to_string();
    let arguments = value
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

fn parse_codex_usage(value: &Value, pricing: PricingTable) -> Usage {
    let input_tokens = value
        .get("input_tokens")
        .or_else(|| value.get("prompt_tokens"))
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let output_tokens = value
        .get("output_tokens")
        .or_else(|| value.get("completion_tokens"))
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let cache_read_tokens = value
        .get("input_tokens_details")
        .or_else(|| value.get("prompt_tokens_details"))
        .and_then(|details| details.get("cached_tokens"))
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let reasoning_output_tokens = value
        .get("output_tokens_details")
        .or_else(|| value.get("completion_tokens_details"))
        .and_then(|details| details.get("reasoning_tokens"))
        .and_then(Value::as_u64)
        .unwrap_or(0);
    Usage {
        input_tokens,
        output_tokens,
        cache_read_tokens,
        cache_creation_tokens: 0,
        reasoning_output_tokens,
        estimated_cost_usd: estimate_cost(pricing, input_tokens, output_tokens, cache_read_tokens),
    }
}

fn format_openai_codex_error(status: reqwest::StatusCode, body: &str) -> String {
    if let Ok(value) = serde_json::from_str::<Value>(body)
        && let Some(error) = value.get("error")
    {
        let code = error
            .get("code")
            .or_else(|| error.get("type"))
            .and_then(Value::as_str)
            .unwrap_or_default();
        if status == reqwest::StatusCode::TOO_MANY_REQUESTS
            || code.contains("usage_limit")
            || code.contains("rate_limit")
        {
            return "OpenAI Codex usage limit reached for this ChatGPT account".to_string();
        }
        if let Some(message) = error.get("message").and_then(Value::as_str) {
            return format!("OpenAI Codex request returned {status}: {message}");
        }
    }
    format!("OpenAI Codex request returned {status}: {body}")
}
