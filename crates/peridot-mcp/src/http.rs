use std::time::Duration;

use peridot_common::{McpServerConfig, PeriError, PeriResult};
use reqwest::{
    Client,
    header::{ACCEPT, AUTHORIZATION, CONTENT_TYPE, HeaderMap, HeaderValue},
};
use serde_json::Value;

use crate::protocol::{
    MCP_PROTOCOL_VERSION, ensure_success, initialize_request, initialized_notification,
    jsonrpc_request, parse_http_body,
};

pub(crate) async fn http_request(
    config: &McpServerConfig,
    request_timeout: Duration,
    method: &str,
    params: Value,
    id: u64,
) -> PeriResult<Value> {
    let client = Client::builder()
        .timeout(request_timeout)
        .build()
        .map_err(|err| PeriError::Tool(format!("failed to build MCP HTTP client: {err}")))?;
    let mut session_id = None;
    let (initialize, session) = http_exchange(config, &client, initialize_request(1), None).await?;
    session_id = session_id.or(session);
    ensure_success(&initialize)?;
    let _ = http_exchange(
        config,
        &client,
        initialized_notification(),
        session_id.as_deref(),
    )
    .await?;
    let (response, _) = http_exchange(
        config,
        &client,
        jsonrpc_request(id, method, params),
        session_id.as_deref(),
    )
    .await?;
    Ok(ensure_success(&response)?.clone())
}

async fn http_exchange(
    config: &McpServerConfig,
    client: &Client,
    message: Value,
    session_id: Option<&str>,
) -> PeriResult<(Value, Option<String>)> {
    let url = config.url.as_deref().ok_or_else(|| {
        PeriError::Config(format!("http MCP server {} is missing url", config.name))
    })?;
    let headers = request_headers(config, session_id)?;
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
        return Ok((serde_json::json!({}), session_id));
    }
    Ok((parse_http_body(&body)?, session_id))
}

fn request_headers(config: &McpServerConfig, session_id: Option<&str>) -> PeriResult<HeaderMap> {
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
    if let Some(auth) = config.auth.as_deref() {
        headers.insert(AUTHORIZATION, auth_header(auth)?);
    }
    Ok(headers)
}

pub(crate) fn auth_header(auth: &str) -> PeriResult<HeaderValue> {
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
