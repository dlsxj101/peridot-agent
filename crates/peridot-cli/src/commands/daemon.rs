//! `peridot daemon` — JSON-RPC over stdio server.
//!
//! Speaks line-delimited JSON-RPC 2.0 (`\n` framed) so VS Code / desktop
//! extensions can drive Peridot bidirectionally. Each line on stdin is
//! one request; each line on stdout is one response or notification.
//!
//! v0.0.1 surface (this release): just `peridot.version` and
//! `peridot.echo` so the extension scaffold can verify the publish
//! pipeline end-to-end before real agent work lands. The real
//! `session.start` / `approval.respond` / `ask_user.respond` methods
//! arrive in v0.1.0 once the extension WebView is ready to consume
//! them.
//!
//! Wire format:
//!
//! ```text
//! C→S  {"jsonrpc":"2.0","id":1,"method":"peridot.version"}
//! S→C  {"jsonrpc":"2.0","id":1,"result":{"version":"0.7.4"}}
//!
//! C→S  {"jsonrpc":"2.0","id":2,"method":"peridot.echo","params":{"text":"hi"}}
//! S→C  {"jsonrpc":"2.0","id":2,"result":{"echo":"hi"}}
//!
//! C→S  {"jsonrpc":"2.0","method":"shutdown"}
//! (server closes stdin loop and exits 0)
//! ```
//!
//! Server-pushed notifications (no `id`) land on stdout the moment the
//! corresponding `AgentRunEvent` fires. v0.0.1 has none; v0.1.0 forwards
//! `event` notifications whose `params.event` is the serialised
//! `AgentRunEvent` (already serde-tagged).

use std::io::{BufRead, BufReader, Write};
use std::path::Path;

use anyhow::Result;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// JSON-RPC 2.0 request envelope.
///
/// `id` is optional (notifications skip it). `params` is a free-form
/// Value so each method validates its own shape. We keep the type
/// boundary thin: extension code that crafts the wire bytes can stay
/// straightforward JSON without needing to know our Rust types.
#[derive(Debug, Deserialize)]
struct RpcRequest {
    /// MUST be `"2.0"`. Defaults to empty so a missing field maps to
    /// `-32600 Invalid Request` (protocol violation) instead of
    /// `-32700 Parse Error` (malformed bytes) — the spec distinguishes
    /// the two and clients debug them differently.
    #[serde(default)]
    jsonrpc: String,
    #[serde(default)]
    id: Option<Value>,
    method: String,
    #[serde(default)]
    params: Option<Value>,
}

/// JSON-RPC 2.0 success response.
#[derive(Debug, Serialize)]
struct RpcResponse {
    jsonrpc: &'static str,
    id: Value,
    result: Value,
}

/// JSON-RPC 2.0 error response. Codes follow the spec:
/// -32700 parse error, -32600 invalid request, -32601 method not
/// found, -32602 invalid params, -32603 internal error.
#[derive(Debug, Serialize)]
struct RpcErrorResponse {
    jsonrpc: &'static str,
    id: Value,
    error: RpcError,
}

#[derive(Debug, Serialize)]
struct RpcError {
    code: i32,
    message: String,
}

/// Public entry point invoked by `peridot daemon`. Drives the stdin
/// loop until the client sends `shutdown` (or EOF). `project_root` is
/// stashed for future methods (session.start needs it); v0.0.1 only
/// uses it for diagnostics.
pub(crate) fn run_daemon_command(_project_root: &Path) -> Result<()> {
    let stdin = std::io::stdin();
    let stdout = std::io::stdout();
    let mut reader = BufReader::new(stdin.lock());
    let mut out = stdout.lock();
    let mut line = String::new();
    loop {
        line.clear();
        let read = reader.read_line(&mut line)?;
        if read == 0 {
            // EOF — client closed pipe. Exit cleanly.
            break;
        }
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let request: RpcRequest = match serde_json::from_str(trimmed) {
            Ok(r) => r,
            Err(err) => {
                write_error(&mut out, Value::Null, -32700, format!("parse error: {err}"))?;
                continue;
            }
        };
        if request.jsonrpc != "2.0" {
            write_error(
                &mut out,
                request.id.unwrap_or(Value::Null),
                -32600,
                format!("expected jsonrpc=2.0, got {}", request.jsonrpc),
            )?;
            continue;
        }
        match request.method.as_str() {
            "peridot.version" => {
                write_response(
                    &mut out,
                    request.id.unwrap_or(Value::Null),
                    serde_json::json!({ "version": env!("CARGO_PKG_VERSION") }),
                )?;
            }
            "peridot.echo" => match request.params {
                Some(Value::Object(map)) => {
                    let echo = map.get("text").cloned().unwrap_or(Value::Null);
                    write_response(
                        &mut out,
                        request.id.unwrap_or(Value::Null),
                        serde_json::json!({ "echo": echo }),
                    )?;
                }
                _ => {
                    write_error(
                        &mut out,
                        request.id.unwrap_or(Value::Null),
                        -32602,
                        "params must be an object with a `text` field".to_string(),
                    )?;
                }
            },
            "shutdown" => {
                // Notification or request — both end the loop. If the
                // client sent an id, send an explicit `ok` back so the
                // peer can confirm a clean shutdown before EOF.
                if let Some(id) = request.id {
                    write_response(&mut out, id, serde_json::json!({ "shutdown": true }))?;
                }
                break;
            }
            other => {
                write_error(
                    &mut out,
                    request.id.unwrap_or(Value::Null),
                    -32601,
                    format!("method not found: {other}"),
                )?;
            }
        }
    }
    Ok(())
}

/// Writes one success line to stdout, flushing immediately so the
/// extension client sees the response in real time. Each response is
/// exactly one `\n`-terminated JSON value.
fn write_response<W: Write>(out: &mut W, id: Value, result: Value) -> Result<()> {
    let envelope = RpcResponse {
        jsonrpc: "2.0",
        id,
        result,
    };
    let line = serde_json::to_string(&envelope)?;
    writeln!(out, "{line}")?;
    out.flush()?;
    Ok(())
}

/// Writes one error line to stdout. Matches the success path's flush
/// discipline so the client gets the error without buffering delay.
fn write_error<W: Write>(out: &mut W, id: Value, code: i32, message: String) -> Result<()> {
    let envelope = RpcErrorResponse {
        jsonrpc: "2.0",
        id,
        error: RpcError { code, message },
    };
    let line = serde_json::to_string(&envelope)?;
    writeln!(out, "{line}")?;
    out.flush()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Drives one request line through the dispatcher and returns the
    /// stdout bytes the daemon would have written. Used by every
    /// per-method test so the wire format stays uniform.
    fn drive(line: &str) -> String {
        // The handler is gated on stdin/stdout I/O which we can't
        // easily mock without rewriting. Instead we replay the
        // dispatch logic against an in-memory writer by inlining the
        // critical branch. Keeps the tests pure and the production
        // path simple.
        let mut out: Vec<u8> = Vec::new();
        let request: RpcRequest = match serde_json::from_str(line.trim()) {
            Ok(r) => r,
            Err(err) => {
                write_error(&mut out, Value::Null, -32700, format!("parse error: {err}")).unwrap();
                return String::from_utf8(out).unwrap();
            }
        };
        if request.jsonrpc != "2.0" {
            write_error(
                &mut out,
                request.id.unwrap_or(Value::Null),
                -32600,
                format!("expected jsonrpc=2.0, got {}", request.jsonrpc),
            )
            .unwrap();
            return String::from_utf8(out).unwrap();
        }
        match request.method.as_str() {
            "peridot.version" => {
                write_response(
                    &mut out,
                    request.id.unwrap_or(Value::Null),
                    serde_json::json!({ "version": env!("CARGO_PKG_VERSION") }),
                )
                .unwrap();
            }
            "peridot.echo" => match request.params {
                Some(Value::Object(map)) => {
                    let echo = map.get("text").cloned().unwrap_or(Value::Null);
                    write_response(
                        &mut out,
                        request.id.unwrap_or(Value::Null),
                        serde_json::json!({ "echo": echo }),
                    )
                    .unwrap();
                }
                _ => {
                    write_error(
                        &mut out,
                        request.id.unwrap_or(Value::Null),
                        -32602,
                        "params must be an object with a `text` field".to_string(),
                    )
                    .unwrap();
                }
            },
            "shutdown" => {
                if let Some(id) = request.id {
                    write_response(&mut out, id, serde_json::json!({ "shutdown": true })).unwrap();
                }
            }
            other => {
                write_error(
                    &mut out,
                    request.id.unwrap_or(Value::Null),
                    -32601,
                    format!("method not found: {other}"),
                )
                .unwrap();
            }
        }
        String::from_utf8(out).unwrap()
    }

    #[test]
    fn version_method_returns_cargo_pkg_version() {
        let out = drive(r#"{"jsonrpc":"2.0","id":1,"method":"peridot.version"}"#);
        let parsed: Value = serde_json::from_str(out.trim()).unwrap();
        assert_eq!(parsed["jsonrpc"], "2.0");
        assert_eq!(parsed["id"], 1);
        assert_eq!(parsed["result"]["version"], env!("CARGO_PKG_VERSION"));
    }

    #[test]
    fn echo_method_returns_text_unchanged() {
        let out =
            drive(r#"{"jsonrpc":"2.0","id":2,"method":"peridot.echo","params":{"text":"hello"}}"#);
        let parsed: Value = serde_json::from_str(out.trim()).unwrap();
        assert_eq!(parsed["id"], 2);
        assert_eq!(parsed["result"]["echo"], "hello");
    }

    #[test]
    fn echo_without_text_field_returns_null_echo() {
        let out = drive(r#"{"jsonrpc":"2.0","id":3,"method":"peridot.echo","params":{}}"#);
        let parsed: Value = serde_json::from_str(out.trim()).unwrap();
        assert_eq!(parsed["id"], 3);
        assert!(parsed["result"]["echo"].is_null());
    }

    #[test]
    fn echo_with_non_object_params_returns_invalid_params_error() {
        let out =
            drive(r#"{"jsonrpc":"2.0","id":4,"method":"peridot.echo","params":"not-an-object"}"#);
        let parsed: Value = serde_json::from_str(out.trim()).unwrap();
        assert_eq!(parsed["id"], 4);
        assert_eq!(parsed["error"]["code"], -32602);
    }

    #[test]
    fn unknown_method_returns_method_not_found() {
        let out = drive(r#"{"jsonrpc":"2.0","id":5,"method":"session.start"}"#);
        let parsed: Value = serde_json::from_str(out.trim()).unwrap();
        assert_eq!(parsed["id"], 5);
        assert_eq!(parsed["error"]["code"], -32601);
        assert!(
            parsed["error"]["message"]
                .as_str()
                .unwrap()
                .contains("method not found")
        );
    }

    #[test]
    fn missing_jsonrpc_version_returns_invalid_request() {
        let out = drive(r#"{"id":6,"method":"peridot.version"}"#);
        let parsed: Value = serde_json::from_str(out.trim()).unwrap();
        assert_eq!(parsed["error"]["code"], -32600);
    }

    #[test]
    fn malformed_json_returns_parse_error_with_null_id() {
        let out = drive("not json at all");
        let parsed: Value = serde_json::from_str(out.trim()).unwrap();
        assert!(parsed["id"].is_null());
        assert_eq!(parsed["error"]["code"], -32700);
    }

    #[test]
    fn shutdown_with_id_returns_ack() {
        let out = drive(r#"{"jsonrpc":"2.0","id":7,"method":"shutdown"}"#);
        let parsed: Value = serde_json::from_str(out.trim()).unwrap();
        assert_eq!(parsed["id"], 7);
        assert_eq!(parsed["result"]["shutdown"], true);
    }

    #[test]
    fn shutdown_as_notification_produces_no_output() {
        let out = drive(r#"{"jsonrpc":"2.0","method":"shutdown"}"#);
        assert!(out.is_empty(), "notification shutdown should not respond");
    }
}
