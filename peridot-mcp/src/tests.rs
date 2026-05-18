use std::fs;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::os::unix::fs::PermissionsExt;
use std::thread;
use std::time::Duration;

use peridot_common::{McpServerConfig, McpTransport};
use serde_json::json;

use crate::McpClient;
use crate::http::auth_header;
use crate::protocol::{MCP_PROTOCOL_VERSION, initialize_request, parse_http_body};

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
            timeout_seconds: 30,
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
            timeout_seconds: 30,
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
            timeout_seconds: 30,
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
