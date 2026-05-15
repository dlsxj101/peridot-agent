use peridot_common::{PeriError, PeriResult};
use peridot_llm::{
    CompletionRequest, CompletionResponse, CompletionStreamChunk, LlmProvider, Usage,
};

pub(crate) fn accumulate_usage(total: &mut Usage, usage: Usage) {
    total.input_tokens += usage.input_tokens;
    total.output_tokens += usage.output_tokens;
    total.cache_read_tokens += usage.cache_read_tokens;
    total.cache_creation_tokens += usage.cache_creation_tokens;
    total.estimated_cost_usd += usage.estimated_cost_usd;
}

pub(crate) async fn stream_completion<P>(
    provider: &P,
    request: CompletionRequest,
) -> PeriResult<CompletionResponse>
where
    P: LlmProvider + ?Sized,
{
    let chunks = provider.stream(request).await?;
    collect_stream_chunks(chunks)
}

fn collect_stream_chunks(chunks: Vec<CompletionStreamChunk>) -> PeriResult<CompletionResponse> {
    if chunks.is_empty() {
        return Err(PeriError::Provider(
            "provider stream returned no chunks".to_string(),
        ));
    }
    let mut text = String::new();
    let mut usage = Usage::default();
    let mut saw_done = false;
    for chunk in chunks {
        text.push_str(&chunk.delta);
        if chunk.done {
            saw_done = true;
        }
        if let Some(chunk_usage) = chunk.usage {
            usage = chunk_usage;
        }
    }
    if !saw_done {
        return Err(PeriError::Provider(
            "provider stream ended without a done chunk".to_string(),
        ));
    }
    Ok(CompletionResponse { text, usage })
}
