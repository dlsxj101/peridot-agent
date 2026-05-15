use peridot_common::{PeriError, PeriResult};
use serde_json::{Value, json};

/// Protocol version sent during MCP initialization.
pub const MCP_PROTOCOL_VERSION: &str = "2025-11-25";

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

pub(crate) fn parse_http_body(body: &str) -> PeriResult<Value> {
    let trimmed = body.trim();
    if let Some(data) = parse_sse_data(trimmed) {
        return serde_json::from_str(&data)
            .map_err(|err| PeriError::Parse(format!("invalid MCP SSE JSON-RPC response: {err}")));
    }
    serde_json::from_str(trimmed)
        .map_err(|err| PeriError::Parse(format!("invalid MCP HTTP JSON-RPC response: {err}")))
}

fn parse_sse_data(body: &str) -> Option<String> {
    let mut chunks = Vec::new();
    for line in body.lines() {
        let line = line.trim_end();
        if let Some(value) = line.strip_prefix("data:") {
            chunks.push(value.trim_start());
        }
    }
    if chunks.is_empty() {
        None
    } else {
        Some(chunks.join("\n"))
    }
}
