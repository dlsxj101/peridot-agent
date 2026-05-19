use std::time::Duration;

use super::*;

pub(crate) async fn run_mcp_command(
    command: &McpCommand,
    config: &PeridotConfig,
    output: OutputFormat,
) -> Result<()> {
    match command {
        McpCommand::List => match output {
            OutputFormat::Json => println!(
                "{}",
                serde_json::to_string_pretty(&config.mcp.iter().map(mcp_json).collect::<Vec<_>>())?
            ),
            OutputFormat::Text => {
                for server in &config.mcp {
                    println!(
                        "{}\t{}\t{}\ttimeout={}s",
                        server.name,
                        server.transport,
                        mcp_target(server),
                        server.timeout_seconds
                    );
                }
            }
        },
        McpCommand::Test { name } => {
            let server = config
                .mcp
                .iter()
                .find(|server| server.name == *name)
                .with_context(|| format!("MCP server not found: {name}"))?;
            validate_mcp_server(server)?;
            let tools = McpClient::with_timeout(
                server.clone(),
                Duration::from_secs(server.timeout_seconds.max(1)),
            )
            .list_tools()
            .await?;
            print_json_or_text_result(
                serde_json::json!({
                    "name": server.name,
                    "transport": server.transport,
                    "target": mcp_target(server),
                    "timeout_seconds": server.timeout_seconds,
                    "configured": true,
                    "tools": tools
                }),
                format!(
                    "MCP server {} is configured for {} ({}) with timeout {}s and {} tools",
                    server.name,
                    server.transport,
                    mcp_target(server),
                    server.timeout_seconds,
                    tools.len()
                ),
                output,
            )?;
        }
        McpCommand::Doctor => {
            let mut reports = Vec::with_capacity(config.mcp.len());
            for server in &config.mcp {
                let validation = validate_mcp_server(server);
                let (configured, validation_error) = match &validation {
                    Ok(()) => (true, None),
                    Err(err) => (false, Some(err.to_string())),
                };
                let (health, latency_ms, tools_count): (&'static str, Option<u128>, Option<usize>) =
                    if configured {
                        let client = McpClient::with_timeout(
                            server.clone(),
                            Duration::from_secs(server.timeout_seconds.max(1)),
                        );
                        // health probe first; then a tools/list to capture catalogue size.
                        match client.health_check().await {
                            Ok(duration) => match client.list_tools().await {
                                Ok(tools) => ("ok", Some(duration.as_millis()), Some(tools.len())),
                                Err(_) => ("degraded", Some(duration.as_millis()), None),
                            },
                            Err(_) => ("unreachable", None, None),
                        }
                    } else {
                        ("invalid_config", None, None)
                    };
                reports.push(serde_json::json!({
                    "name": server.name,
                    "transport": server.transport.to_string(),
                    "target": mcp_target(server),
                    "configured": configured,
                    "validation_error": validation_error,
                    "health": health,
                    "latency_ms": latency_ms,
                    "tools_count": tools_count,
                    "default_permission": server.default_permission,
                    "schema_cache_seconds": server.schema_cache_seconds,
                }));
            }
            let text_summary = reports
                .iter()
                .map(|r| {
                    let name = r["name"].as_str().unwrap_or("");
                    let health = r["health"].as_str().unwrap_or("?");
                    let latency = r["latency_ms"]
                        .as_u64()
                        .map(|ms| format!("{ms}ms"))
                        .unwrap_or_else(|| "—".to_string());
                    let tools = r["tools_count"]
                        .as_u64()
                        .map(|n| n.to_string())
                        .unwrap_or_else(|| "—".to_string());
                    let perm = r["default_permission"].as_str().unwrap_or("system");
                    format!("{name}\t{health}\tlatency={latency}\ttools={tools}\tperm={perm}")
                })
                .collect::<Vec<_>>()
                .join("\n");
            print_json_or_text_result(
                serde_json::json!({ "servers": reports }),
                if text_summary.is_empty() {
                    "no MCP servers configured".to_string()
                } else {
                    text_summary
                },
                output,
            )?;
        }
    }
    Ok(())
}

pub(super) fn validate_mcp_server(server: &McpServerConfig) -> Result<()> {
    match server.transport {
        McpTransport::Stdio => {
            if server
                .command
                .as_deref()
                .unwrap_or_default()
                .trim()
                .is_empty()
            {
                anyhow::bail!("stdio MCP server {} is missing command", server.name);
            }
        }
        McpTransport::Http => {
            if server.url.as_deref().unwrap_or_default().trim().is_empty() {
                anyhow::bail!("http MCP server {} is missing url", server.name);
            }
        }
    }
    Ok(())
}

pub(super) fn mcp_target(server: &McpServerConfig) -> String {
    match server.transport {
        McpTransport::Stdio => {
            let mut parts = Vec::new();
            if let Some(command) = &server.command {
                parts.push(command.clone());
            }
            parts.extend(server.args.iter().cloned());
            parts.join(" ")
        }
        McpTransport::Http => server.url.clone().unwrap_or_default(),
    }
}

pub(super) fn mcp_json(server: &McpServerConfig) -> serde_json::Value {
    serde_json::json!({
        "name": server.name,
        "transport": server.transport,
        "target": mcp_target(server),
        "timeout_seconds": server.timeout_seconds,
        "configured": validate_mcp_server(server).is_ok()
    })
}
