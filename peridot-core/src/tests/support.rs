use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

use async_trait::async_trait;
use peridot_common::{PeriError, PeriResult};
use peridot_llm::{
    AuthMethod, CompletionRequest, CompletionResponse, CompletionStreamChunk, LlmProvider,
    PricingTable, ToolInvocation, Usage,
};
use serde_json::Value;

pub(crate) struct StaticProvider {
    responses: Mutex<Vec<String>>,
    /// Pre-baked completions that bypass the legacy `action`-envelope parser
    /// in [`interpret_static_response`]. Used by tests that need to assert
    /// behaviour for completions carrying BOTH text AND tool calls — a shape
    /// the legacy string envelopes cannot produce. Drained before the string
    /// queue on each `complete` / `stream` call.
    custom_completions: Mutex<Vec<(String, Vec<ToolInvocation>)>>,
    cost_usd: f64,
    call_counter: AtomicU64,
    parse_failures_remaining: AtomicU64,
}

pub(crate) struct StreamingOnlyProvider;

impl StaticProvider {
    pub(crate) fn new(responses: Vec<String>) -> Self {
        Self {
            responses: Mutex::new(responses.into_iter().rev().collect()),
            custom_completions: Mutex::new(Vec::new()),
            cost_usd: 0.0,
            call_counter: AtomicU64::new(0),
            parse_failures_remaining: AtomicU64::new(0),
        }
    }

    pub(crate) fn with_cost(responses: Vec<String>, cost_usd: f64) -> Self {
        Self {
            responses: Mutex::new(responses.into_iter().rev().collect()),
            custom_completions: Mutex::new(Vec::new()),
            cost_usd,
            call_counter: AtomicU64::new(0),
            parse_failures_remaining: AtomicU64::new(0),
        }
    }

    /// Returns a `PeriError::Parse` for the first `failures` requests before falling
    /// back to the queued responses. Used to exercise the recovery path that injects
    /// a format reminder after repeated parse errors from the provider.
    pub(crate) fn with_initial_parse_errors(responses: Vec<String>, failures: u64) -> Self {
        Self {
            responses: Mutex::new(responses.into_iter().rev().collect()),
            custom_completions: Mutex::new(Vec::new()),
            cost_usd: 0.0,
            call_counter: AtomicU64::new(0),
            parse_failures_remaining: AtomicU64::new(failures),
        }
    }

    /// Returns a provider that completes ONCE with `text` plus an explicit
    /// `tool_calls` entry built from `tool_name` + `tool_arguments`. Used to
    /// reproduce the "model streamed a reply AND called agent_done" shape
    /// where qwen-style providers duplicate the answer in both channels.
    pub(crate) fn new_text_with_tool_call(
        text: String,
        tool_name: String,
        tool_arguments: serde_json::Value,
    ) -> Self {
        let invocation = ToolInvocation {
            id: "call_text_plus_tool".to_string(),
            name: tool_name,
            arguments: tool_arguments,
        };
        Self {
            responses: Mutex::new(Vec::new()),
            custom_completions: Mutex::new(vec![(text, vec![invocation])]),
            cost_usd: 0.0,
            call_counter: AtomicU64::new(0),
            parse_failures_remaining: AtomicU64::new(0),
        }
    }
}

/// Interprets a canned response string. Legacy `{"action": "...", "parameters": {...}}`
/// envelopes — what the prompted-JSON parser used to consume — are converted into a
/// native tool call so existing test fixtures keep exercising the same scenarios on the
/// rewritten agent loop. Plain strings pass through as a text-only assistant reply.
fn interpret_static_response(text: &str, call_index: u64) -> (String, Vec<ToolInvocation>) {
    let trimmed = text.trim();
    let Ok(value) = serde_json::from_str::<Value>(trimmed) else {
        return (text.to_string(), Vec::new());
    };
    let Some(action) = value.get("action").and_then(Value::as_str) else {
        return (text.to_string(), Vec::new());
    };
    let parameters = value
        .get("parameters")
        .cloned()
        .unwrap_or_else(|| Value::Object(Default::default()));
    let invocation = ToolInvocation {
        id: format!("call_{call_index}"),
        name: action.to_string(),
        arguments: parameters,
    };
    (String::new(), vec![invocation])
}

#[async_trait]
impl LlmProvider for StaticProvider {
    async fn complete(&self, _request: CompletionRequest) -> PeriResult<CompletionResponse> {
        if self
            .parse_failures_remaining
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |count| {
                if count > 0 { Some(count - 1) } else { None }
            })
            .is_ok()
        {
            return Err(PeriError::Parse(
                "static provider injected parse error".to_string(),
            ));
        }
        // Pre-baked completions take priority — drained FIFO so tests can chain
        // a `text + agent_done` shape without going through the legacy
        // string-envelope parser.
        if let Some((response_text, tool_calls)) =
            self.custom_completions.lock().unwrap().pop()
        {
            self.call_counter.fetch_add(1, Ordering::SeqCst);
            return Ok(CompletionResponse {
                text: response_text,
                tool_calls,
                usage: Usage {
                    input_tokens: 1,
                    output_tokens: 1,
                    cache_read_tokens: 0,
                    cache_creation_tokens: 0,
                    reasoning_output_tokens: 0,
                    estimated_cost_usd: self.cost_usd,
                },
            });
        }
        let text = self
            .responses
            .lock()
            .unwrap()
            .pop()
            .ok_or_else(|| PeriError::Provider("no response".to_string()))?;
        let call_index = self.call_counter.fetch_add(1, Ordering::SeqCst);
        let (response_text, tool_calls) = interpret_static_response(&text, call_index);
        Ok(CompletionResponse {
            text: response_text,
            tool_calls,
            usage: Usage {
                input_tokens: 1,
                output_tokens: 1,
                cache_read_tokens: 0,
                cache_creation_tokens: 0,
                reasoning_output_tokens: 0,
                estimated_cost_usd: self.cost_usd,
            },
        })
    }

    fn supports_cache(&self) -> bool {
        false
    }

    fn supports_prefill(&self) -> bool {
        false
    }

    fn supports_thinking(&self) -> bool {
        false
    }

    fn pricing(&self) -> PricingTable {
        PricingTable::default()
    }

    fn auth_method(&self) -> AuthMethod {
        AuthMethod::NotConfigured
    }
}

#[async_trait]
impl LlmProvider for StreamingOnlyProvider {
    async fn complete(&self, _request: CompletionRequest) -> PeriResult<CompletionResponse> {
        Err(PeriError::Provider(
            "complete should not be used for streamed turns".to_string(),
        ))
    }

    async fn stream(&self, _request: CompletionRequest) -> PeriResult<Vec<CompletionStreamChunk>> {
        Ok(vec![
            CompletionStreamChunk {
                delta: "streamed agent_done".to_string(),
                tool_calls: Vec::new(),
                done: false,
                usage: None,
            },
            CompletionStreamChunk {
                delta: String::new(),
                tool_calls: vec![peridot_llm::ToolInvocation {
                    id: "call_0".to_string(),
                    name: "agent_done".to_string(),
                    arguments: serde_json::json!({"summary": "streamed"}),
                }],
                done: true,
                usage: Some(Usage {
                    input_tokens: 2,
                    output_tokens: 3,
                    cache_read_tokens: 1,
                    cache_creation_tokens: 0,
                    reasoning_output_tokens: 0,
                    estimated_cost_usd: 0.04,
                }),
            },
        ])
    }

    fn supports_cache(&self) -> bool {
        false
    }

    fn supports_prefill(&self) -> bool {
        false
    }

    fn supports_thinking(&self) -> bool {
        false
    }

    fn pricing(&self) -> PricingTable {
        PricingTable::default()
    }

    fn auth_method(&self) -> AuthMethod {
        AuthMethod::NotConfigured
    }
}
