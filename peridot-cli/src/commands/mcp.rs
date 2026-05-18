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
