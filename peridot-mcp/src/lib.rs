//! MCP client boundary and server configuration types.

use std::process::Stdio;
use std::time::Duration;

pub use peridot_common::{McpServerConfig, McpTransport};
use peridot_common::{PeriError, PeriResult};
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
            McpTransport::Stdio => self.list_stdio_tools().await,
            McpTransport::Http => Err(PeriError::Config(
                "streamable HTTP MCP transport is not implemented yet".to_string(),
            )),
        }
    }

    async fn list_stdio_tools(&self) -> PeriResult<Vec<McpTool>> {
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
        write_message(&mut stdin, jsonrpc_request(2, "tools/list", json!({}))).await?;
        let tools = read_response(&mut reader, 2, self.timeout).await?;
        let result = ensure_success(&tools)?;
        let parsed = result.get("tools").cloned().unwrap_or_else(|| json!([]));
        let _ = child.kill().await;
        serde_json::from_value(parsed)
            .map_err(|err| PeriError::Parse(format!("invalid MCP tools/list response: {err}")))
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::os::unix::fs::PermissionsExt;

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
}
