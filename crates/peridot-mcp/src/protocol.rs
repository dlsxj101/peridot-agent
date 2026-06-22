use peridot_common::{PeriError, PeriResult};
use serde_json::{Value, json};

/// Protocol version sent during MCP initialization.
pub const MCP_PROTOCOL_VERSION: &str = "2025-11-25";

/// Reserved JSON-RPC id for the internal `initialize` handshake. Using
/// `u64::MAX` keeps it out of the range of caller-supplied ids (health
/// checks and tool calls), so a handshake response can never be mistaken
/// for a caller's response.
pub(crate) const INIT_REQUEST_ID: u64 = u64::MAX;

pub(crate) fn initialize_request(id: u64) -> Value {
    jsonrpc_request(
        id,
        "initialize",
        json!({
            "protocolVersion": MCP_PROTOCOL_VERSION,
            "capabilities": {},
            "clientInfo": {
                "name": "peridot-agent",
                "version": env!("CARGO_PKG_VERSION")
            }
        }),
    )
}

pub(crate) fn initialized_notification() -> Value {
    json!({
        "jsonrpc": "2.0",
        "method": "notifications/initialized"
    })
}

pub(crate) fn jsonrpc_request(id: u64, method: &str, params: Value) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "method": method,
        "params": params
    })
}

pub(crate) fn ensure_success(value: &Value) -> PeriResult<&Value> {
    if let Some(error) = value.get("error") {
        return Err(PeriError::Tool(format!("MCP error response: {error}")));
    }
    value
        .get("result")
        .ok_or_else(|| PeriError::Parse("MCP response missing result".to_string()))
}

/// Soft check of the server's negotiated protocol version against the one the
/// client advertised. Errors on a mismatch so callers fail loudly instead of
/// proceeding against an incompatible server; a missing field is tolerated
/// (some servers omit it) and treated as a match.
pub(crate) fn check_protocol_version(result: &Value) -> PeriResult<()> {
    match result.get("protocolVersion").and_then(Value::as_str) {
        Some(version) if version != MCP_PROTOCOL_VERSION => Err(PeriError::Tool(format!(
            "MCP server protocol version {version} does not match client version {MCP_PROTOCOL_VERSION}"
        ))),
        _ => Ok(()),
    }
}

/// Parses an MCP HTTP response body and returns the JSON-RPC object that
/// corresponds to the request with `expected_id`.
///
/// A `text/event-stream` body may carry several SSE events (e.g. a progress
/// notification followed by the result). Each event's `data:` block is parsed
/// as its own JSON object; the one whose `"id"` matches `expected_id` is
/// returned, and notifications / objects without a matching id are ignored.
/// Plain JSON bodies are parsed directly.
pub(crate) fn parse_http_body(body: &str, expected_id: u64) -> PeriResult<Value> {
    let trimmed = body.trim();
    let blocks = parse_sse_data(trimmed);
    if !blocks.is_empty() {
        let numeric_id = Value::from(expected_id);
        let string_id = Value::from(expected_id.to_string());
        let mut last_error: Option<String> = None;
        for block in &blocks {
            match serde_json::from_str::<Value>(block) {
                Ok(value) => {
                    if let Some(id) = value.get("id")
                        && (*id == numeric_id || *id == string_id)
                    {
                        return Ok(value);
                    }
                }
                Err(err) => last_error = Some(err.to_string()),
            }
        }
        return Err(PeriError::Parse(format!(
            "MCP SSE response had no JSON-RPC object with id {expected_id}{}",
            last_error
                .map(|err| format!(" (last parse error: {err})"))
                .unwrap_or_default()
        )));
    }
    serde_json::from_str(trimmed)
        .map_err(|err| PeriError::Parse(format!("invalid MCP HTTP JSON-RPC response: {err}")))
}

/// Splits an SSE body into its individual event `data:` payloads. A single SSE
/// event may span multiple `data:` lines, which are joined with `\n`; blank
/// lines delimit events.
fn parse_sse_data(body: &str) -> Vec<String> {
    let mut blocks = Vec::new();
    let mut current: Vec<&str> = Vec::new();
    for line in body.lines() {
        let line = line.trim_end();
        if line.is_empty() {
            if !current.is_empty() {
                blocks.push(current.join("\n"));
                current.clear();
            }
            continue;
        }
        if let Some(value) = line.strip_prefix("data:") {
            current.push(value.trim_start());
        }
    }
    if !current.is_empty() {
        blocks.push(current.join("\n"));
    }
    blocks
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn init_request_id_is_reserved_max() {
        assert_eq!(INIT_REQUEST_ID, u64::MAX);
        // The handshake id must sit outside the range of caller ids used by
        // the client (health checks / tools/list / tools/call use 1 and 2).
        assert_ne!(INIT_REQUEST_ID, 1);
        assert_ne!(INIT_REQUEST_ID, 2);
    }

    #[test]
    fn protocol_version_match_and_missing_are_ok() {
        check_protocol_version(&json!({ "protocolVersion": MCP_PROTOCOL_VERSION })).unwrap();
        // A missing field is tolerated.
        check_protocol_version(&json!({ "capabilities": {} })).unwrap();
    }

    #[test]
    fn protocol_version_mismatch_errors() {
        let result = check_protocol_version(&json!({ "protocolVersion": "1999-01-01" }));
        assert!(result.is_err());
    }
}
