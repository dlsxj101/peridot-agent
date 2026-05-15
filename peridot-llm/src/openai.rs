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
        let payload = openai_responses_payload(&request);
        let endpoint = format!("{}/v1/responses", self.base_url.trim_end_matches('/'));
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
        let mut payload = openai_responses_payload(&request);
        payload["stream"] = Value::Bool(true);
        let endpoint = format!("{}/v1/responses", self.base_url.trim_end_matches('/'));
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

pub(crate) fn openai_responses_payload(request: &CompletionRequest) -> Value {
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

pub(crate) fn parse_openai_response(
    body: &str,
    pricing: PricingTable,
) -> PeriResult<CompletionResponse> {
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

pub(crate) fn parse_openai_stream(
    body: &str,
    pricing: PricingTable,
) -> PeriResult<Vec<CompletionStreamChunk>> {
    let mut chunks = Vec::new();
    let mut input_tokens = 0;
    let mut output_tokens = 0;
    let mut cache_read_tokens = 0;

    for data in sse_data_events(body) {
        if data == "[DONE]" {
            break;
        }
        let value = serde_json::from_str::<Value>(&data)
            .map_err(|err| PeriError::Provider(format!("invalid OpenAI stream JSON: {err}")))?;
        match value.get("type").and_then(Value::as_str) {
            Some("response.output_text.delta") => {
                if let Some(delta) = value.get("delta").and_then(Value::as_str) {
                    chunks.push(CompletionStreamChunk {
                        delta: delta.to_string(),
                        done: false,
                        usage: None,
                    });
                }
            }
            Some("response.completed") => {
                if let Some(usage) = value
                    .get("response")
                    .and_then(|response| response.get("usage"))
                {
                    input_tokens = usage
                        .get("input_tokens")
                        .and_then(Value::as_u64)
                        .unwrap_or(input_tokens);
                    output_tokens = usage
                        .get("output_tokens")
                        .and_then(Value::as_u64)
                        .unwrap_or(output_tokens);
                    cache_read_tokens = usage
                        .get("input_tokens_details")
                        .and_then(|details| details.get("cached_tokens"))
                        .and_then(Value::as_u64)
                        .unwrap_or(cache_read_tokens);
                }
            }
            _ => {}
        }
    }

    let estimated_cost_usd = estimate_cost(pricing, input_tokens, output_tokens, cache_read_tokens);
    chunks.push(CompletionStreamChunk {
        delta: String::new(),
        done: true,
        usage: Some(Usage {
            input_tokens,
            output_tokens,
            cache_read_tokens,
            cache_creation_tokens: 0,
            estimated_cost_usd,
        }),
    });
    Ok(chunks)
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
