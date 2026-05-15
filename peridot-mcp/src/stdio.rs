use std::process::Stdio;
use std::time::Duration;

use peridot_common::{McpServerConfig, PeriError, PeriResult};
use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command;
use tokio::time::timeout;

use crate::protocol::{
    ensure_success, initialize_request, initialized_notification, jsonrpc_request,
};

pub(crate) async fn stdio_request(
    config: &McpServerConfig,
    request_timeout: Duration,
    method: &str,
    params: Value,
    id: u64,
) -> PeriResult<Value> {
    let command = config.command.as_deref().ok_or_else(|| {
        PeriError::Config(format!(
            "stdio MCP server {} is missing command",
            config.name
        ))
    })?;
    let mut process = Command::new(command);
    process.args(&config.args);
    process.envs(&config.env);
    process
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = process.spawn().map_err(|err| {
        PeriError::Tool(format!(
            "failed to launch MCP server {}: {err}",
            config.name
        ))
    })?;
    let mut stdin = child.stdin.take().ok_or_else(|| {
        PeriError::Tool(format!(
            "failed to open stdin for MCP server {}",
            config.name
        ))
    })?;
    let stdout = child.stdout.take().ok_or_else(|| {
        PeriError::Tool(format!(
            "failed to open stdout for MCP server {}",
            config.name
        ))
    })?;
    let mut reader = BufReader::new(stdout).lines();

    write_message(&mut stdin, initialize_request(1)).await?;
    let initialize = read_response(&mut reader, 1, request_timeout).await?;
    ensure_success(&initialize)?;
    write_message(&mut stdin, initialized_notification()).await?;
    write_message(&mut stdin, jsonrpc_request(id, method, params)).await?;
    let response = read_response(&mut reader, id, request_timeout).await?;
    let result = ensure_success(&response)?.clone();
    let _ = child.kill().await;
    Ok(result)
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
