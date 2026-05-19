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
    ///
    /// **Wired:** consulted by `ClaudeProvider` to gate `cache_control`
    /// breakpoint marking (see `anthropic_payload_with_cache`, v0.6.0).
    fn supports_cache(&self) -> bool;

    /// Returns true when the provider supports response prefill — priming
    /// the assistant turn with seed text the model must continue from.
    /// This is the canonical Tool Masking primitive: by prefilling
    /// `{"tool_name": "verify_` the model is forced to pick a verifier
    /// tool, not file_write.
    ///
    /// **Intentionally not consulted in production today.** Reasoning:
    ///
    /// 1. Response prefill is an Anthropic Messages API-only feature.
    ///    OpenAI Chat Completions, OpenAI Codex (OAuth), and OpenRouter
    ///    have no equivalent — they accept an `assistant` message as
    ///    the last entry but do not constrain the next token to
    ///    continue it.
    /// 2. Peridot's first-class Claude path is API-key only;
    ///    Claude OAuth (the Claude.ai subscription path) is **not**
    ///    supported. SPEC §6.2 + 부록 B note that Claude-specific
    ///    optimisations are gated behind first-class Claude support,
    ///    and OAuth being absent means we deliberately stay at the
    ///    lowest common denominator instead of investing in a
    ///    Claude-API-key-only optimisation that the OAuth-subscription
    ///    majority can't reach.
    /// 3. Tool Masking is currently realised through system-prompt
    ///    directives + recovery messages instead of a real prefill
    ///    payload. `CompletionRequest` has no `prefill` field; adding
    ///    one is a deliberate follow-up tied to whenever Claude OAuth
    ///    lands.
    ///
    /// Provider impls should still return their honest capability so
    /// the trait surface stays accurate (Claude returns `true`,
    /// everyone else returns `false`); the harness simply does not act
    /// on the answer yet. Cache + thinking gating (`supports_cache` /
    /// `supports_thinking`) establishes the precedent prefill will
    /// follow when wired.
    fn supports_prefill(&self) -> bool;

    /// Returns true when the provider supports extended thinking controls.
    ///
    /// **Wired:** consulted by `HarnessAgent::run_turn_with_events` to
    /// AND-gate the thinking flag, so Goal mode against providers that
    /// don't support thinking (e.g. OpenAI Chat Completions) no longer
    /// sends a payload field the server ignores (v0.6.0).
    fn supports_thinking(&self) -> bool;

    /// Returns the provider pricing table — canonical $/M-token figures
    /// for input, output, and cache-read tokens. Each provider impl
    /// stamps this at construction from a hardcoded table, and the
    /// cost estimator inside `transport::estimate_cost` reads the same
    /// numbers, so the trait method is a single source of truth for
    /// external observers.
    ///
    /// **Wired:** surfaced by `peridot doctor` (check
    /// `provider:pricing`) so an operator can sanity-check the table
    /// without grepping source.
    fn pricing(&self) -> PricingTable;

    /// Returns the active auth method as it actually resolves at
    /// construction time. Providers must downgrade to
    /// `AuthMethod::NotConfigured` when credentials are missing —
    /// reporting `ApiKey` / `OAuth` while the credential field is empty
    /// would defeat the doctor's "right config, just no keys yet"
    /// signal.
    ///
    /// **Wired:** surfaced by `peridot doctor` (check
    /// `provider:auth_method`).
    fn auth_method(&self) -> AuthMethod;
}
