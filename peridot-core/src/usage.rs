use peridot_common::{PeriError, PeriResult};
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
    let (stream_result, (text, tool_calls, usage, saw_done)) =
        tokio::join!(stream_future, receive_future);
    stream_result?;
    if !saw_done {
        return Err(PeriError::Provider(
            "provider stream ended without a done chunk".to_string(),
        ));
    }
    Ok(CompletionResponse {
        text,
        tool_calls,
        usage,
    })
}

