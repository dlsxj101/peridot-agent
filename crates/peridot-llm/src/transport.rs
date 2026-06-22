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

/// Parses a `Retry-After` response header in the integer-seconds form (what
/// Anthropic and OpenAI send on 429/503). The HTTP-date form is not parsed
/// (returns `None`) — the local exponential backoff covers that case. Capped at
/// 60s so a malformed or hostile value can't wedge a retry for minutes.
pub(crate) fn parse_retry_after(
    headers: &reqwest::header::HeaderMap,
) -> Option<std::time::Duration> {
    let raw = headers.get(reqwest::header::RETRY_AFTER)?.to_str().ok()?;
    let secs: u64 = raw.trim().parse().ok()?;
    Some(std::time::Duration::from_secs(secs.min(60)))
}

/// Exponential backoff delay applied *before* a retry attempt. `attempt` is the
/// upcoming attempt number (the first retry, i.e. the loop's second iteration,
/// is `attempt = 1`). Doubles from 250ms and caps at 8s.
///
/// Pure and deterministic so it can be unit-tested; jitter is layered on top in
/// [`backoff_before_retry`] from the wall clock rather than baked in here.
pub(crate) fn retry_backoff_delay(attempt: u32) -> std::time::Duration {
    let exp = attempt.saturating_sub(1).min(5);
    let base_ms = 250u64.saturating_mul(1u64 << exp).min(8_000);
    std::time::Duration::from_millis(base_ms)
}

/// Sleeps for [`retry_backoff_delay`] plus up to ~20% wall-clock jitter, so the
/// previous behaviour (immediate `continue`, hammering a rate-limited or
/// erroring upstream and resynchronizing concurrent sessions into a thundering
/// herd) is replaced with backed-off, de-synchronized retries.
///
/// When the upstream returned a `Retry-After` hint (see [`parse_retry_after`])
/// asking for a *longer* wait than the local schedule, that hint wins — we never
/// retry sooner than the server asked. A shorter or absent hint leaves the
/// backed-off, jittered delay untouched.
pub(crate) async fn backoff_before_retry(attempt: u32, retry_after: Option<std::time::Duration>) {
    let base = retry_backoff_delay(attempt);
    let jitter_window_ms = (base.as_millis() as u64 / 5).max(1);
    let extra_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| u64::from(d.subsec_nanos()))
        .unwrap_or(0)
        % (jitter_window_ms + 1);
    let computed = base + std::time::Duration::from_millis(extra_ms);
    let delay = retry_after.map_or(computed, |hint| hint.max(computed));
    tokio::time::sleep(delay).await;
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
    fn retry_backoff_doubles_and_caps() {
        assert_eq!(retry_backoff_delay(0).as_millis(), 250); // pre-first-retry guard
        assert_eq!(retry_backoff_delay(1).as_millis(), 250);
        assert_eq!(retry_backoff_delay(2).as_millis(), 500);
        assert_eq!(retry_backoff_delay(3).as_millis(), 1_000);
        assert_eq!(retry_backoff_delay(4).as_millis(), 2_000);
        // Caps at 8s no matter how high the attempt climbs.
        assert_eq!(retry_backoff_delay(6).as_millis(), 8_000);
        assert_eq!(retry_backoff_delay(50).as_millis(), 8_000);
    }

    #[test]
    fn sse_drain_tolerates_utf8_split_across_chunks() {
        let mut buffer = Vec::new();
        let mut events = Vec::new();
        let event = "data: {\"delta\":\"안녕\"}\n\n".as_bytes();
        let split_at = event
            .windows("녕".len())
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
