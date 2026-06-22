use std::process::Stdio;
use std::time::Duration;

use peridot_common::{McpServerConfig, PeriError, PeriResult};
use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command;
use tokio::time::timeout_at;

use crate::protocol::{
    INIT_REQUEST_ID, check_protocol_version, ensure_success, initialize_request,
    initialized_notification, jsonrpc_request,
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
        // stderr is not consumed; piping it risks a deadlock if the server
        // writes more than the pipe buffer holds.
        .stderr(Stdio::null())
        // tokio's Child does not kill on drop; ensure the process is reaped if
        // any `?` below returns early before the explicit kill on the happy path.
        .kill_on_drop(true);
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

    // Reserved internal id so the handshake can never collide with a caller id.
    write_message(&mut stdin, initialize_request(INIT_REQUEST_ID)).await?;
    let initialize = read_response(&mut reader, INIT_REQUEST_ID, request_timeout).await?;
    check_protocol_version(ensure_success(&initialize)?)?;
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
    // Single deadline for the whole exchange: a server that keeps emitting
    // unmatched lines just under the per-line timeout must still time out.
    let deadline = tokio::time::Instant::now() + wait;
    let numeric_id = Value::from(id);
    let string_id = Value::from(id.to_string());
    loop {
        let line = timeout_at(deadline, reader.next_line())
            .await
            .map_err(|_| PeriError::Tool(format!("timed out waiting for MCP response {id}")))?
            .map_err(|err| PeriError::Tool(format!("failed to read MCP response: {err}")))?
            .ok_or_else(|| PeriError::Tool("MCP server closed stdout".to_string()))?;
        // Non-JSON stdout (banners, logging) is noise, not a fatal error.
        let value = match serde_json::from_str::<Value>(&line) {
            Ok(value) => value,
            Err(_) => continue,
        };
        if let Some(response_id) = value.get("id")
            && (*response_id == numeric_id || *response_id == string_id)
        {
            return Ok(value);
        }
    }
}
