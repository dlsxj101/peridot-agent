//! MCP server registration / probing slash command handlers and the
//! project-config TOML helpers they rely on, split out of the daemon
//! module. Parent (private) items are reached via `use super::*`.

use peridot_common::{McpTransport, PeridotConfig};
use serde_json::Value;
use std::path::{Path, PathBuf};

use super::*;

#[allow(clippy::too_many_arguments)]
pub(super) fn config_path(state: &DaemonState) -> PathBuf {
    state.project_root.join(".peridot/config.toml")
}

pub(super) fn handle_command_mcp_list(
    state: &DaemonState,
    raw_command: &str,
) -> Result<Value, String> {
    let path = config_path(state);
    let config = read_project_config(&path)?;
    let items = mcp_command_items(&config);
    Ok(serde_json::json!({
        "kind": "mcp",
        "title": "MCP Servers",
        "message": if items.is_empty() { "mcp: <none configured>".to_string() } else { format!("mcp: {} server(s)", items.len()) },
        "severity": "info",
        "command": raw_command,
        "items": items,
    }))
}

pub(super) fn handle_command_mcp_add(
    state: &DaemonState,
    raw_command: &str,
    name: &str,
    transport: &str,
    target: &str,
) -> Result<Value, String> {
    let path = config_path(state);
    let existing = read_project_config(&path)?;
    if existing.mcp.iter().any(|m| m.name == name) {
        return Err(format!(
            "mcp add: '{name}' already configured - use /mcp remove first"
        ));
    }
    let block = match transport.to_ascii_lowercase().as_str() {
        "stdio" => {
            let mut parts = target.split_whitespace();
            let Some(command) = parts.next() else {
                return Err("mcp add: stdio transport requires a command".to_string());
            };
            let args: Vec<&str> = parts.collect();
            let args_toml = if args.is_empty() {
                String::new()
            } else {
                let quoted = args
                    .iter()
                    .map(|a| format!("\"{}\"", escape_toml_string(a)))
                    .collect::<Vec<_>>()
                    .join(", ");
                format!("args = [{quoted}]\n")
            };
            format!(
                "\n[[mcp]]\nname = \"{}\"\ntransport = \"stdio\"\ncommand = \"{}\"\n{}",
                escape_toml_string(name),
                escape_toml_string(command),
                args_toml,
            )
        }
        "http" | "sse" => format!(
            "\n[[mcp]]\nname = \"{}\"\ntransport = \"http\"\nurl = \"{}\"\n",
            escape_toml_string(name),
            escape_toml_string(target),
        ),
        other => {
            return Err(format!(
                "mcp add: unknown transport '{other}' (use stdio or http)"
            ));
        }
    };
    let existing_content = match std::fs::read_to_string(&path) {
        Ok(content) => content,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(err) => return Err(format!("mcp add: read {}: {err}", path.display())),
    };
    let new_content = if existing_content.is_empty() {
        block.trim_start_matches('\n').to_string()
    } else if existing_content.ends_with('\n') {
        format!("{existing_content}{block}")
    } else {
        format!("{existing_content}\n{block}")
    };
    atomic_write(&path, &new_content)?;
    let updated = read_project_config(&path)?;
    let items = mcp_command_items(&updated);
    Ok(serde_json::json!({
        "kind": "mcp",
        "title": "MCP",
        "message": format!("mcp: added '{name}' to {}. Restart this session for it to take effect.", path.display()),
        "severity": "info",
        "command": raw_command,
        "items": items,
    }))
}

pub(super) fn handle_command_mcp_remove(
    state: &DaemonState,
    raw_command: &str,
    name: &str,
) -> Result<Value, String> {
    let path = config_path(state);
    let content = std::fs::read_to_string(&path)
        .map_err(|err| format!("mcp remove: read {}: {err}", path.display()))?;
    let Some(new_content) = remove_mcp_block(&content, name) else {
        return Err(format!("mcp remove: no server named '{name}'"));
    };
    atomic_write(&path, &new_content)?;
    let updated = read_project_config(&path)?;
    let items = mcp_command_items(&updated);
    Ok(serde_json::json!({
        "kind": "mcp",
        "title": "MCP",
        "message": format!("mcp: removed '{name}' from {}. Restart this session to drop its tools from the registry.", path.display()),
        "severity": "info",
        "command": raw_command,
        "items": items,
    }))
}

pub(super) async fn handle_command_mcp_test(
    state: &DaemonState,
    raw_command: &str,
    name: &str,
) -> Result<Value, String> {
    let path = config_path(state);
    let config = read_project_config(&path)?;
    let Some(entry) = config.mcp.iter().find(|m| m.name == name).cloned() else {
        return Err(format!("mcp test: no server named '{name}'"));
    };
    let client = peridot_mcp::McpClient::new(entry);
    let count = client
        .list_tools()
        .await
        .map_err(|err| format!("mcp test '{name}': {err}"))?
        .len();
    let items = mcp_command_items_with_probe(&config, Some((name, true, Some(count))));
    Ok(serde_json::json!({
        "kind": "mcp",
        "title": "MCP",
        "message": format!("mcp: '{name}' reachable - {count} tool(s) exposed"),
        "severity": "info",
        "command": raw_command,
        "items": items,
    }))
}

fn mcp_command_items(config: &PeridotConfig) -> Vec<Value> {
    mcp_command_items_with_probe(config, None)
}

pub(super) fn mcp_command_items_with_probe(
    config: &PeridotConfig,
    probe: Option<(&str, bool, Option<usize>)>,
) -> Vec<Value> {
    config
        .mcp
        .iter()
        .map(|entry| {
            let detail = match entry.transport {
                McpTransport::Stdio => {
                    let args = if entry.args.is_empty() {
                        String::new()
                    } else {
                        format!(" {}", entry.args.join(" "))
                    };
                    format!("{}{}", entry.command.clone().unwrap_or_default(), args)
                }
                McpTransport::Http => entry.url.clone().unwrap_or_default(),
            };
            let mut item = serde_json::json!({
                "label": entry.name,
                "detail": detail,
                "transport": entry.transport.to_string(),
            });
            if let Some((probe_name, connected, tool_count)) = probe
                && entry.name == probe_name
                && let Value::Object(map) = &mut item
            {
                map.insert("connected".to_string(), Value::Bool(connected));
                if let Some(tool_count) = tool_count {
                    map.insert("tool_count".to_string(), serde_json::json!(tool_count));
                }
            }
            item
        })
        .collect()
}

pub(super) fn read_project_config(path: &Path) -> Result<PeridotConfig, String> {
    match std::fs::read_to_string(path) {
        Ok(content) => toml::from_str::<PeridotConfig>(&content)
            .map_err(|err| format!("failed to parse {}: {err}", path.display())),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(PeridotConfig::default()),
        Err(err) => Err(format!("failed to read {}: {err}", path.display())),
    }
}

fn remove_mcp_block(content: &str, target: &str) -> Option<String> {
    let lines: Vec<&str> = content.lines().collect();
    let mut blocks: Vec<(usize, usize, Option<String>)> = Vec::new();
    let mut current_start: Option<usize> = None;
    let mut current_name: Option<String> = None;
    for (idx, line) in lines.iter().enumerate() {
        let trimmed = line.trim();
        if trimmed == "[[mcp]]" {
            if let Some(start) = current_start.take() {
                blocks.push((start, idx, current_name.take()));
            }
            current_start = Some(idx);
        } else if let Some(name_value) = trimmed
            .strip_prefix("name")
            .and_then(|s| s.trim_start().strip_prefix('='))
            .map(|s| s.trim().trim_matches('"'))
            && current_start.is_some()
            && current_name.is_none()
        {
            current_name = Some(name_value.to_string());
        } else if (trimmed.starts_with("[[") || trimmed.starts_with('['))
            && let Some(start) = current_start.take()
        {
            blocks.push((start, idx, current_name.take()));
        }
    }
    if let Some(start) = current_start.take() {
        blocks.push((start, lines.len(), current_name.take()));
    }
    let (start, end, _) = blocks
        .into_iter()
        .find(|(_, _, name)| name.as_deref() == Some(target))?;
    let mut kept: Vec<&str> = Vec::with_capacity(lines.len());
    kept.extend(lines.iter().take(start).copied());
    kept.extend(lines.iter().skip(end).copied());
    let mut result = kept.join("\n");
    if content.ends_with('\n') {
        result.push('\n');
    }
    Some(result)
}

fn escape_toml_string(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}

fn atomic_write(path: &Path, content: &str) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|err| format!("create {}: {err}", parent.display()))?;
    }
    let temp = path.with_extension("toml.tmp");
    std::fs::write(&temp, content).map_err(|err| format!("write {}: {err}", temp.display()))?;
    std::fs::rename(&temp, path)
        .map_err(|err| format!("rename {} -> {}: {err}", temp.display(), path.display()))
}
