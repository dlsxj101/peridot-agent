use futures_util::StreamExt;
use peridot_common::{PeriError, PeriResult};

use crate::PricingTable;

pub(crate) fn estimate_cost(
    pricing: PricingTable,
    input_tokens: u64,
    output_tokens: u64,
    cache_read_tokens: u64,
) -> f64 {
    (input_tokens as f64 / 1_000_000.0 * pricing.input_per_million)
        + (output_tokens as f64 / 1_000_000.0 * pricing.output_per_million)
        + (cache_read_tokens as f64 / 1_000_000.0 * pricing.cache_read_per_million)
}

pub(crate) fn should_retry_status(status: reqwest::StatusCode) -> bool {
    status == reqwest::StatusCode::REQUEST_TIMEOUT
        || status == reqwest::StatusCode::TOO_MANY_REQUESTS
        || status.is_server_error()
}

pub(crate) async fn read_streaming_response(response: reqwest::Response) -> PeriResult<String> {
    let mut body = Vec::new();
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk =
            chunk.map_err(|err| PeriError::Provider(format!("stream read failed: {err}")))?;
        body.extend_from_slice(&chunk);
    }
    String::from_utf8(body)
        .map_err(|err| PeriError::Provider(format!("stream response was not UTF-8: {err}")))
}

/// Consumes an SSE byte stream, yielding each event body (the joined `data:` payload
/// of a single event) as it arrives. Boundary is the canonical SSE blank line. Lines
/// starting with `:` are comments and skipped; non-`data:` lines are also ignored to
/// stay compatible with OpenAI's `event:` lines and similar metadata. Used by
/// providers to render responses incrementally instead of buffering the full body.
pub(crate) async fn stream_sse_events<F>(
    response: reqwest::Response,
    mut on_event: F,
) -> PeriResult<()>
where
    F: FnMut(&str) -> PeriResult<()>,
{
    let mut stream = response.bytes_stream();
    let mut buffer = String::new();
    while let Some(chunk) = stream.next().await {
        let chunk =
            chunk.map_err(|err| PeriError::Provider(format!("stream read failed: {err}")))?;
        let text = std::str::from_utf8(&chunk)
            .map_err(|err| PeriError::Provider(format!("stream chunk was not UTF-8: {err}")))?;
        buffer.push_str(text);
        while let Some(boundary) = find_sse_boundary(&buffer) {
            let event_text = buffer[..boundary.end].to_string();
            buffer = buffer[boundary.end..].to_string();
            let data = collect_sse_data(&event_text);
            if !data.is_empty() {
                on_event(&data)?;
            }
        }
    }
    let data = collect_sse_data(&buffer);
    if !data.is_empty() {
        on_event(&data)?;
    }
    Ok(())
}

#[derive(Clone, Copy)]
struct SseBoundary {
    end: usize,
}

/// Finds the byte index just past the next SSE event terminator (`\n\n` or `\r\n\r\n`)
/// in `text`. Returns `None` while the buffer still ends mid-event so the caller keeps
/// accumulating bytes.
fn find_sse_boundary(text: &str) -> Option<SseBoundary> {
    if let Some(idx) = text.find("\r\n\r\n") {
        return Some(SseBoundary {
            end: idx + "\r\n\r\n".len(),
        });
    }
    text.find("\n\n").map(|idx| SseBoundary {
        end: idx + "\n\n".len(),
    })
}

/// Joins all `data:` payload lines from one SSE event block into a single string,
/// matching the multi-line concatenation rule from the EventSource spec.
fn collect_sse_data(event_block: &str) -> String {
    let mut parts = Vec::new();
    for line in event_block.lines() {
        let line = line.trim_end_matches('\r');
        if line.is_empty() || line.starts_with(':') {
            continue;
        }
        if let Some(data) = line.strip_prefix("data:") {
            parts.push(data.trim_start().to_string());
        }
    }
    parts.join("\n")
}

/// OpenAI provider using the Responses API.
pub(crate) fn sse_data_events(body: &str) -> Vec<String> {
    let mut events = Vec::new();
    let mut current = Vec::new();
    for line in body.lines() {
        let line = line.trim_end_matches('\r');
        if line.is_empty() {
            if !current.is_empty() {
                events.push(current.join("\n"));
                current.clear();
            }
            continue;
        }
        if line.starts_with(':') {
            continue;
        }
        if let Some(data) = line.strip_prefix("data:") {
            current.push(data.trim_start().to_string());
        }
    }
    if !current.is_empty() {
        events.push(current.join("\n"));
    }
    events
}
