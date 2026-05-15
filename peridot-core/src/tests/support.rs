use std::sync::Mutex;

use async_trait::async_trait;
use peridot_common::{PeriError, PeriResult};
use peridot_llm::{
    AuthMethod, CompletionRequest, CompletionResponse, CompletionStreamChunk, LlmProvider,
    PricingTable, Usage,
};

pub(crate) struct StaticProvider {
    responses: Mutex<Vec<String>>,
    cost_usd: f64,
}

pub(crate) struct StreamingOnlyProvider;

impl StaticProvider {
    pub(crate) fn new(responses: Vec<String>) -> Self {
        Self {
            responses: Mutex::new(responses.into_iter().rev().collect()),
            cost_usd: 0.0,
        }
    }

    pub(crate) fn with_cost(responses: Vec<String>, cost_usd: f64) -> Self {
        Self {
            responses: Mutex::new(responses.into_iter().rev().collect()),
            cost_usd,
        }
    }
}

#[async_trait]
impl LlmProvider for StaticProvider {
    async fn complete(&self, _request: CompletionRequest) -> PeriResult<CompletionResponse> {
        let text = self
            .responses
            .lock()
            .unwrap()
            .pop()
            .ok_or_else(|| PeriError::Provider("no response".to_string()))?;
        Ok(CompletionResponse {
            text,
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
                delta: "{\"action\":\"agent_done\",\"parameters\":".to_string(),
                done: false,
                usage: None,
            },
            CompletionStreamChunk {
                delta: "{\"summary\":\"streamed\"}}".to_string(),
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
