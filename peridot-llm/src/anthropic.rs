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

pub(crate) fn parse_anthropic_response(
    body: &str,
    pricing: PricingTable,
) -> PeriResult<CompletionResponse> {
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
            reasoning_output_tokens: 0,
            estimated_cost_usd,
        },
    })
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
            Some("content_block_delta") => {
                if let Some(delta) = value
                    .get("delta")
                    .and_then(|delta| delta.get("text"))
                    .and_then(Value::as_str)
                {
                    chunks.push(CompletionStreamChunk {
                        delta: delta.to_string(),
                        done: false,
                        usage: None,
                    });
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

    let estimated_cost_usd = estimate_cost(
        pricing,
        input_tokens + cache_creation_tokens,
        output_tokens,
        cache_read_tokens,
    );
    chunks.push(CompletionStreamChunk {
        delta: String::new(),
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
use std::time::Duration;

use async_trait::async_trait;
use peridot_common::{PeriError, PeriResult};
use serde_json::{Value, json};

use crate::transport::{
    estimate_cost, read_streaming_response, should_retry_status, sse_data_events,
};
use crate::{
    AuthMethod, CompletionRequest, CompletionResponse, CompletionStreamChunk, LlmProvider,
    MessageRole, PricingTable, Usage,
};
