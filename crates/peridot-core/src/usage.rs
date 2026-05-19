use peridot_common::{CancelToken, PeriError, PeriResult};
use peridot_llm::{
    CompletionRequest, CompletionResponse, CompletionStreamChunk, LlmProvider, ToolInvocation,
    Usage,
};

/// Accumulates one turn's [`Usage`] into a running total. Public so the
/// committee loop in `peridot-cli` can reuse the same arithmetic as the
/// standard agent loop.
pub fn accumulate_usage(total: &mut Usage, usage: Usage) {
    total.input_tokens += usage.input_tokens;
    total.output_tokens += usage.output_tokens;
    total.cache_read_tokens += usage.cache_read_tokens;
    total.cache_creation_tokens += usage.cache_creation_tokens;
    total.reasoning_output_tokens += usage.reasoning_output_tokens;
    total.estimated_cost_usd += usage.estimated_cost_usd;
}

pub(crate) async fn stream_completion_with_chunks<P, F>(
    provider: &P,
    request: CompletionRequest,
    cancel: Option<&CancelToken>,
    mut on_chunk: F,
) -> PeriResult<CompletionResponse>
where
    P: LlmProvider + ?Sized,
    F: FnMut(&CompletionStreamChunk),
{
    // Drive the provider and the receiver loop concurrently on the same task. The
    // provider sends each parsed chunk into the channel as bytes arrive (true
    // incremental streaming); the receiver loop drains chunks into `on_chunk` for
    // live UI updates and into the final `CompletionResponse` accumulator. When the
    // provider future completes its `sender` drops and the receiver loop exits
    // cleanly via `recv().await -> None`.
    //
    // The `cancel` token is raced against the producer + consumer pair via
    // `tokio::select!`. When the operator presses Esc the token flips, the
    // race resolves, the streaming future is dropped, reqwest aborts the
    // connection, and we surface a `PeriError::Tool("interrupted by user")`
    // that the agent loop maps to `StopReason::Interrupted` and an
    // `AgentRunEvent::Interrupted`. Without this race the LLM call could
    // run to completion (seconds) before the next inter-turn cancel check
    // fired — which is the "Esc doesn't actually stop anything" UX bug.
    let (sender, mut receiver) = tokio::sync::mpsc::unbounded_channel::<CompletionStreamChunk>();
    let stream_future = provider.stream_chunks(request, sender);
    let receive_future = async {
        let mut text = String::new();
        let mut tool_calls: Vec<ToolInvocation> = Vec::new();
        let mut usage = Usage::default();
        let mut saw_done = false;
        while let Some(chunk) = receiver.recv().await {
            on_chunk(&chunk);
            text.push_str(&chunk.delta);
            if !chunk.tool_calls.is_empty() {
                tool_calls.extend(chunk.tool_calls);
            }
            if chunk.done {
                saw_done = true;
            }
            if let Some(chunk_usage) = chunk.usage {
                usage = chunk_usage;
            }
        }
        (text, tool_calls, usage, saw_done)
    };
    let joined = async {
        let (stream_result, payload) = tokio::join!(stream_future, receive_future);
        stream_result.map(|_| payload)
    };
    let (text, tool_calls, usage, saw_done) = match cancel {
        Some(token) => {
            tokio::select! {
                result = joined => result?,
                _ = token.cancelled() => {
                    return Err(PeriError::Tool("interrupted by user".to_string()));
                }
            }
        }
        None => joined.await?,
    };
    if !saw_done {
        return Err(PeriError::Provider(
            "provider stream ended without a done chunk".to_string(),
        ));
    }
    Ok(CompletionResponse {
        text,
        tool_calls,
        reasoning_content: None,
        usage,
    })
}
