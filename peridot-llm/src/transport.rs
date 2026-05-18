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
    let mut buffer = Vec::new();
    while let Some(chunk) = stream.next().await {
        let chunk =
            chunk.map_err(|err| PeriError::Provider(format!("stream read failed: {err}")))?;
        buffer.extend_from_slice(&chunk);
        drain_sse_events(&mut buffer, &mut on_event)?;
    }
    drain_sse_tail(buffer, &mut on_event)?;
    Ok(())
}

#[derive(Clone, Copy)]
struct SseBoundary {
    end: usize,
}

fn find_sse_boundary_bytes(bytes: &[u8]) -> Option<SseBoundary> {
    if let Some(idx) = bytes.windows(4).position(|window| window == b"\r\n\r\n") {
        return Some(SseBoundary { end: idx + 4 });
    }
    bytes
        .windows(2)
        .position(|window| window == b"\n\n")
        .map(|idx| SseBoundary { end: idx + 2 })
}

fn drain_sse_events<F>(buffer: &mut Vec<u8>, on_event: &mut F) -> PeriResult<()>
where
    F: FnMut(&str) -> PeriResult<()>,
{
    while let Some(boundary) = find_sse_boundary_bytes(buffer) {
        let event_bytes: Vec<u8> = buffer.drain(..boundary.end).collect();
        let event_text = String::from_utf8(event_bytes)
            .map_err(|err| PeriError::Provider(format!("stream event was not UTF-8: {err}")))?;
        let data = collect_sse_data(&event_text);
        if !data.is_empty() {
            on_event(&data)?;
        }
    }
    Ok(())
}

fn drain_sse_tail<F>(buffer: Vec<u8>, on_event: &mut F) -> PeriResult<()>
where
    F: FnMut(&str) -> PeriResult<()>,
{
    let tail = String::from_utf8(buffer)
        .map_err(|err| PeriError::Provider(format!("stream tail was not UTF-8: {err}")))?;
    let data = collect_sse_data(&tail);
    if !data.is_empty() {
        on_event(&data)?;
    }
    Ok(())
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sse_drain_tolerates_utf8_split_across_chunks() {
        let mut buffer = Vec::new();
        let mut events = Vec::new();
        let event = "data: {\"delta\":\"안녕\"}\n\n".as_bytes();
        let split_at = event
            .windows("녕".as_bytes().len())
            .position(|window| window == "녕".as_bytes())
            .expect("test fixture should contain target syllable")
            + 1;

        buffer.extend_from_slice(&event[..split_at]);
        drain_sse_events(&mut buffer, &mut |data: &str| {
            events.push(data.to_string());
            Ok(())
        })
        .unwrap();
        assert!(events.is_empty());

        buffer.extend_from_slice(&event[split_at..]);
        drain_sse_events(&mut buffer, &mut |data: &str| {
            events.push(data.to_string());
            Ok(())
        })
        .unwrap();

        assert_eq!(events, vec!["{\"delta\":\"안녕\"}".to_string()]);
        assert!(buffer.is_empty());
    }
}
