use async_trait::async_trait;
use peridot_common::PeriResult;
use serde::{Deserialize, Serialize};

use crate::{AuthMethod, CompletionRequest, CompletionResponse, CompletionStreamChunk};

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

/// Provider abstraction for chat completion and streaming support.
#[async_trait]
pub trait LlmProvider: Send + Sync {
    /// Complete one model request.
    async fn complete(&self, request: CompletionRequest) -> PeriResult<CompletionResponse>;

    /// Stream one model request as provider-neutral chunks.
    async fn stream(&self, request: CompletionRequest) -> PeriResult<Vec<CompletionStreamChunk>> {
        let response = self.complete(request).await?;
        Ok(vec![CompletionStreamChunk {
            delta: response.text,
            reasoning_delta: String::new(),
            tool_calls: response.tool_calls,
            done: true,
            usage: Some(response.usage),
        }])
    }

    /// True incremental streaming. The default implementation calls [`stream`] and
    /// replays the buffered chunks through the channel; HTTP providers should
    /// override this so chunks reach the receiver as the bytes arrive, giving the
    /// TUI a character-by-character typing effect instead of one big dump.
    async fn stream_chunks(
        &self,
        request: CompletionRequest,
        sender: tokio::sync::mpsc::UnboundedSender<CompletionStreamChunk>,
    ) -> PeriResult<()> {
        let chunks = self.stream(request).await?;
        for chunk in chunks {
            // Receiver gone → caller stopped listening; abort cleanly.
            if sender.send(chunk).is_err() {
                break;
            }
        }
        Ok(())
    }

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
