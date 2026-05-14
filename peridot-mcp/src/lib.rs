//! MCP client boundary and server configuration types.

use std::process::Stdio;
use std::time::Duration;

pub use peridot_common::{McpServerConfig, McpTransport};
use peridot_common::{PeriError, PeriResult};
use reqwest::{
    Client,
    header::{ACCEPT, AUTHORIZATION, CONTENT_TYPE, HeaderMap, HeaderValue},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command;
use tokio::time::timeout;

/// Protocol version sent during MCP initialization.
pub const MCP_PROTOCOL_VERSION: &str = "2025-11-25";

/// Tool exposed by an MCP server.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct McpTool {
    /// Tool name.
    pub name: String,
    /// Optional human-readable description.
    #[serde(default)]
    pub description: Option<String>,
    /// JSON schema for tool arguments.
    #[serde(default, rename = "inputSchema")]
    pub input_schema: Value,
}

/// Result returned by `tools/call`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct McpCallResult {
    /// Tool content payload.
    #[serde(default)]
    pub content: Vec<Value>,
    /// Whether the tool reported an application-level error.
    #[serde(default, rename = "isError")]
    pub is_error: bool,
}

/// MCP client.
#[derive(Clone, Debug)]
pub struct McpClient {
    config: McpServerConfig,
    timeout: Duration,
}

impl McpClient {
    /// Creates an MCP client.
    pub fn new(config: McpServerConfig) -> Self {
        Self {
            config,
            timeout: Duration::from_secs(30),
        }
    }

    /// Creates an MCP client with an explicit timeout.
    pub fn with_timeout(config: McpServerConfig, timeout: Duration) -> Self {
        Self { config, timeout }
    }

    /// Returns server config.
    pub fn config(&self) -> &McpServerConfig {
        &self.config
    }

    /// Initializes the server and lists exposed tools.
    pub async fn list_tools(&self) -> PeriResult<Vec<McpTool>> {
        match self.config.transport {
            McpTransport::Stdio => {
                let result = self.stdio_request("tools/list", json!({}), 2).await?;
                let parsed = result.get("tools").cloned().unwrap_or_else(|| json!([]));
                serde_json::from_value(parsed).map_err(|err| {
                    PeriError::Parse(format!("invalid MCP tools/list response: {err}"))
                })
            }
            McpTransport::Http => {
                let result = self.http_request("tools/list", json!({}), 2).await?;
                let parsed = result.get("tools").cloned().unwrap_or_else(|| json!([]));
                serde_json::from_value(parsed).map_err(|err| {
                    PeriError::Parse(format!("invalid MCP tools/list response: {err}"))
                })
            }
        }
    }

    /// Calls one MCP tool.
    pub async fn call_tool(&self, name: &str, arguments: Value) -> PeriResult<McpCallResult> {
        match self.config.transport {
            McpTransport::Stdio => {
                let result = self
                    .stdio_request(
                        "tools/call",
                        json!({
                            "name": name,
                            "arguments": arguments
                        }),
                        2,
                    )
                    .await?;
                serde_json::from_value(result).map_err(|err| {
                    PeriError::Parse(format!("invalid MCP tools/call response: {err}"))
                })
            }
            McpTransport::Http => {
                let result = self
                    .http_request(
                        "tools/call",
                        json!({
                            "name": name,
                            "arguments": arguments
                        }),
                        2,
                    )
                    .await?;
                serde_json::from_value(result).map_err(|err| {
                    PeriError::Parse(format!("invalid MCP tools/call response: {err}"))
                })
            }
        }
    }

    async fn http_request(&self, method: &str, params: Value, id: u64) -> PeriResult<Value> {
        let client = Client::builder()
            .timeout(self.timeout)
            .build()
            .map_err(|err| PeriError::Tool(format!("failed to build MCP HTTP client: {err}")))?;
        let mut session_id = None;
        let (initialize, session) = self
            .http_exchange(&client, initialize_request(1), None)
            .await?;
        session_id = session_id.or(session);
        ensure_success(&initialize)?;
        let _ = self
            .http_exchange(&client, initialized_notification(), session_id.as_deref())
            .await?;
        let (response, _) = self
            .http_exchange(
                &client,
                jsonrpc_request(id, method, params),
                session_id.as_deref(),
            )
            .await?;
        Ok(ensure_success(&response)?.clone())
    }

    async fn http_exchange(
        &self,
        client: &Client,
        message: Value,
        session_id: Option<&str>,
    ) -> PeriResult<(Value, Option<String>)> {
        let url = self.config.url.as_deref().ok_or_else(|| {
            PeriError::Config(format!(
                "http MCP server {} is missing url",
                self.config.name
            ))
        })?;
        let mut headers = HeaderMap::new();
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
        headers.insert(
            ACCEPT,
            HeaderValue::from_static("application/json, text/event-stream"),
        );
        headers.insert(
            "mcp-protocol-version",
            HeaderValue::from_static(MCP_PROTOCOL_VERSION),
        );
        if let Some(session_id) = session_id {
            let value = HeaderValue::from_str(session_id).map_err(|err| {
                PeriError::Config(format!("invalid MCP session id header value: {err}"))
            })?;
            headers.insert("mcp-session-id", value);
        }
        if let Some(auth) = self.config.auth.as_deref() {
            headers.insert(AUTHORIZATION, auth_header(auth)?);
        }
        let response = client
            .post(url)
            .headers(headers)
            .json(&message)
            .send()
            .await
            .map_err(|err| PeriError::Tool(format!("MCP HTTP request failed: {err}")))?;
        let session_id = response
            .headers()
            .get("mcp-session-id")
            .and_then(|value| value.to_str().ok())
            .map(str::to_string);
        let status = response.status();
        let body = response
            .text()
            .await
            .map_err(|err| PeriError::Tool(format!("failed to read MCP HTTP response: {err}")))?;
        if !status.is_success() {
            return Err(PeriError::Tool(format!(
                "MCP HTTP server returned {status}: {body}"
            )));
        }
        if body.trim().is_empty() {
            return Ok((json!({}), session_id));
        }
        Ok((parse_http_body(&body)?, session_id))
    }

    async fn stdio_request(&self, method: &str, params: Value, id: u64) -> PeriResult<Value> {
        let command = self.config.command.as_deref().ok_or_else(|| {
            PeriError::Config(format!(
                "stdio MCP server {} is missing command",
                self.config.name
            ))
        })?;
        let mut process = Command::new(command);
        process.args(&self.config.args);
        process.envs(&self.config.env);
        process
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = process.spawn().map_err(|err| {
            PeriError::Tool(format!(
                "failed to launch MCP server {}: {err}",
                self.config.name
            ))
        })?;
        let mut stdin = child.stdin.take().ok_or_else(|| {
            PeriError::Tool(format!(
                "failed to open stdin for MCP server {}",
                self.config.name
            ))
        })?;
        let stdout = child.stdout.take().ok_or_else(|| {
            PeriError::Tool(format!(
                "failed to open stdout for MCP server {}",
                self.config.name
            ))
        })?;
        let mut reader = BufReader::new(stdout).lines();

        write_message(&mut stdin, initialize_request(1)).await?;
        let initialize = read_response(&mut reader, 1, self.timeout).await?;
        ensure_success(&initialize)?;
        write_message(&mut stdin, initialized_notification()).await?;
        write_message(&mut stdin, jsonrpc_request(id, method, params)).await?;
        let response = read_response(&mut reader, id, self.timeout).await?;
        let result = ensure_success(&response)?.clone();
        let _ = child.kill().await;
        Ok(result)
    }
}

fn initialize_request(id: u64) -> Value {
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

fn initialized_notification() -> Value {
    json!({
        "jsonrpc": "2.0",
        "method": "notifications/initialized"
    })
}

fn jsonrpc_request(id: u64, method: &str, params: Value) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "method": method,
        "params": params
    })
}

async fn write_message(stdin: &mut tokio::process::ChildStdin, message: Value) -> PeriResult<()> {
    let mut line = serde_json::to_vec(&message)
        .map_err(|err| PeriError::Parse(format!("failed to encode MCP message: {err}")))?;
    line.push(b'\n');
    stdin
        .write_all(&line)
        .await
        .map_err(|err| PeriError::Tool(format!("failed to write MCP message: {err}")))?;
    stdin
        .flush()
        .await
        .map_err(|err| PeriError::Tool(format!("failed to flush MCP message: {err}")))
}

async fn read_response(
    reader: &mut tokio::io::Lines<BufReader<tokio::process::ChildStdout>>,
    id: u64,
    wait: Duration,
) -> PeriResult<Value> {
    loop {
        let line = timeout(wait, reader.next_line())
            .await
            .map_err(|_| PeriError::Tool(format!("timed out waiting for MCP response {id}")))?
            .map_err(|err| PeriError::Tool(format!("failed to read MCP response: {err}")))?
            .ok_or_else(|| PeriError::Tool("MCP server closed stdout".to_string()))?;
        let value = serde_json::from_str::<Value>(&line)
            .map_err(|err| PeriError::Parse(format!("invalid MCP JSON-RPC message: {err}")))?;
        if value.get("id").and_then(Value::as_u64) == Some(id) {
            return Ok(value);
        }
    }
}

fn ensure_success(value: &Value) -> PeriResult<&Value> {
    if let Some(error) = value.get("error") {
        return Err(PeriError::Tool(format!("MCP error response: {error}")));
    }
    value
        .get("result")
        .ok_or_else(|| PeriError::Parse("MCP response missing result".to_string()))
}

fn parse_http_body(body: &str) -> PeriResult<Value> {
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

fn auth_header(auth: &str) -> PeriResult<HeaderValue> {
    let (scheme, value) = auth
        .split_once(':')
        .ok_or_else(|| PeriError::Config("MCP auth must use scheme:value syntax".to_string()))?;
    match scheme {
        "bearer" => {
            let secret = expand_auth_value(value)?;
            HeaderValue::from_str(&format!("Bearer {secret}"))
                .map_err(|err| PeriError::Config(format!("invalid MCP bearer auth header: {err}")))
        }
        "basic" => {
            let secret = expand_auth_value(value)?;
            HeaderValue::from_str(&format!("Basic {secret}"))
                .map_err(|err| PeriError::Config(format!("invalid MCP basic auth header: {err}")))
        }
        other => Err(PeriError::Config(format!(
            "unsupported MCP auth scheme: {other}"
        ))),
    }
}

fn expand_auth_value(value: &str) -> PeriResult<String> {
    let trimmed = value.trim();
    if let Some(name) = trimmed
        .strip_prefix("${")
        .and_then(|value| value.strip_suffix('}'))
    {
        std::env::var(name)
            .map_err(|_| PeriError::Config(format!("MCP auth env var is not set: {name}")))
    } else {
        Ok(trimmed.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::os::unix::fs::PermissionsExt;
    use std::thread;

    #[test]
    fn builds_initialize_request() {
        let request = initialize_request(1);

        assert_eq!(request["method"], "initialize");
        assert_eq!(request["params"]["protocolVersion"], MCP_PROTOCOL_VERSION);
        assert_eq!(request["params"]["clientInfo"]["name"], "peridot-agent");
    }

    #[tokio::test]
    async fn lists_tools_from_stdio_server() {
        let root = std::env::temp_dir().join(format!("peridot-mcp-{}", std::process::id()));
        fs::create_dir_all(&root).unwrap();
        let server = root.join("server.sh");
        fs::write(
            &server,
            r#"#!/bin/sh
while IFS= read -r line; do
  case "$line" in
    *'"method":"initialize"'*)
      printf '{"jsonrpc":"2.0","id":1,"result":{"protocolVersion":"2025-11-25","capabilities":{"tools":{}},"serverInfo":{"name":"test","version":"0"}}}\n'
      ;;
    *'"method":"notifications/initialized"'*)
      ;;
    *'"method":"tools/list"'*)
      printf '{"jsonrpc":"2.0","id":2,"result":{"tools":[{"name":"demo","description":"Demo tool","inputSchema":{"type":"object"}}]}}\n'
      ;;
  esac
done
"#,
        )
        .unwrap();
        fs::set_permissions(&server, fs::Permissions::from_mode(0o755)).unwrap();
        let client = McpClient::with_timeout(
            McpServerConfig {
                name: "test".to_string(),
                transport: McpTransport::Stdio,
                command: Some(server.display().to_string()),
                args: Vec::new(),
                env: Default::default(),
                url: None,
                auth: None,
            },
            Duration::from_secs(2),
        );

        let tools = client.list_tools().await.unwrap();

        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].name, "demo");
        fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn calls_stdio_tool() {
        let root = std::env::temp_dir().join(format!("peridot-mcp-call-{}", std::process::id()));
        fs::create_dir_all(&root).unwrap();
        let server = root.join("server.sh");
        fs::write(
            &server,
            r#"#!/bin/sh
while IFS= read -r line; do
  case "$line" in
    *'"method":"initialize"'*)
      printf '{"jsonrpc":"2.0","id":1,"result":{"protocolVersion":"2025-11-25","capabilities":{"tools":{}},"serverInfo":{"name":"test","version":"0"}}}\n'
      ;;
    *'"method":"notifications/initialized"'*)
      ;;
    *'"method":"tools/call"'*)
      printf '{"jsonrpc":"2.0","id":2,"result":{"content":[{"type":"text","text":"called"}],"isError":false}}\n'
      ;;
  esac
done
"#,
        )
        .unwrap();
        fs::set_permissions(&server, fs::Permissions::from_mode(0o755)).unwrap();
        let client = McpClient::with_timeout(
            McpServerConfig {
                name: "test".to_string(),
                transport: McpTransport::Stdio,
                command: Some(server.display().to_string()),
                args: Vec::new(),
                env: Default::default(),
                url: None,
                auth: None,
            },
            Duration::from_secs(2),
        );

        let result = client.call_tool("demo", json!({"ok": true})).await.unwrap();

        assert!(!result.is_error);
        assert_eq!(result.content[0]["text"], "called");
        fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn lists_tools_from_http_server() {
        let url = spawn_http_server();
        let client = McpClient::with_timeout(
            McpServerConfig {
                name: "http-test".to_string(),
                transport: McpTransport::Http,
                command: None,
                args: Vec::new(),
                env: Default::default(),
                url: Some(url),
                auth: Some("bearer:test-token".to_string()),
            },
            Duration::from_secs(2),
        );

        let tools = client.list_tools().await.unwrap();

        assert_eq!(tools[0].name, "http_demo");
    }

    #[test]
    fn parses_sse_jsonrpc_body() {
        let value = parse_http_body(
            "event: message\ndata: {\"jsonrpc\":\"2.0\",\"id\":2,\"result\":{\"ok\":true}}\n\n",
        )
        .unwrap();

        assert_eq!(value["result"]["ok"], true);
    }

    #[test]
    fn builds_bearer_auth_header() {
        let header = auth_header("bearer:secret").unwrap();

        assert_eq!(header.to_str().unwrap(), "Bearer secret");
    }

    fn spawn_http_server() -> String {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        thread::spawn(move || {
            for _ in 0..3 {
                let (mut stream, _) = listener.accept().unwrap();
                let request = read_http_request(&mut stream);
                let request_headers = request.to_ascii_lowercase();
                assert!(request_headers.contains("authorization: bearer test-token"));
                assert!(request_headers.contains("mcp-protocol-version: 2025-11-25"));
                let body = request
                    .split("\r\n\r\n")
                    .nth(1)
                    .unwrap_or_default()
                    .to_string();
                if body.contains("\"method\":\"initialize\"") {
                    write_response(
                        &mut stream,
                        200,
                        Some("session-1"),
                        r#"{"jsonrpc":"2.0","id":1,"result":{"protocolVersion":"2025-11-25","capabilities":{"tools":{}},"serverInfo":{"name":"http","version":"0"}}}"#,
                    );
                } else if body.contains("\"method\":\"notifications/initialized\"") {
                    write_response(&mut stream, 202, None, "");
                } else if body.contains("\"method\":\"tools/list\"") {
                    assert!(request_headers.contains("mcp-session-id: session-1"));
                    write_response(
                        &mut stream,
                        200,
                        None,
                        r#"{"jsonrpc":"2.0","id":2,"result":{"tools":[{"name":"http_demo","description":"HTTP demo","inputSchema":{"type":"object"}}]}}"#,
                    );
                }
            }
        });
        url
    }

    fn read_http_request(stream: &mut std::net::TcpStream) -> String {
        let mut buffer = Vec::new();
        let mut temp = [0_u8; 1024];
        loop {
            let read = stream.read(&mut temp).unwrap();
            if read == 0 {
                break;
            }
            buffer.extend_from_slice(&temp[..read]);
            if buffer.windows(4).any(|window| window == b"\r\n\r\n") {
                break;
            }
        }
        let header = String::from_utf8_lossy(&buffer).to_string();
        let content_length = header
            .lines()
            .find_map(|line| {
                let (name, value) = line.split_once(':')?;
                name.eq_ignore_ascii_case("content-length")
                    .then(|| value.trim().parse::<usize>().ok())
                    .flatten()
            })
            .unwrap_or(0);
        let header_end = buffer
            .windows(4)
            .position(|window| window == b"\r\n\r\n")
            .map(|position| position + 4)
            .unwrap_or(buffer.len());
        while buffer.len().saturating_sub(header_end) < content_length {
            let read = stream.read(&mut temp).unwrap();
            if read == 0 {
                break;
            }
            buffer.extend_from_slice(&temp[..read]);
        }
        String::from_utf8_lossy(&buffer).to_string()
    }

    fn write_response(
        stream: &mut std::net::TcpStream,
        status: u16,
        session_id: Option<&str>,
        body: &str,
    ) {
        let reason = if status == 202 { "Accepted" } else { "OK" };
        let session_header = session_id
            .map(|value| format!("mcp-session-id: {value}\r\n"))
            .unwrap_or_default();
        let response = format!(
            "HTTP/1.1 {status} {reason}\r\ncontent-type: application/json\r\n{session_header}content-length: {}\r\nconnection: close\r\n\r\n{body}",
            body.len()
        );
        stream.write_all(response.as_bytes()).unwrap();
    }
}
