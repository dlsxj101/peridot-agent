//! Tool-call approval-grant matching.
//!
//! Decides whether a tool call is already covered by a confirmation grant the
//! operator made earlier in the session — a session-wide tool approval, an
//! exact `(tool, parameters)` grant, a `(tool, path)` scope, or (for
//! `shell_exec`) a normalized command grant. Split out of `agent.rs` so the
//! approval-key shape lives in one place; the harness loop calls
//! [`tool_call_has_confirmation_grant`] before prompting the operator again.

use peridot_common::{SecurityConfig, ToolCall};

/// Whether `call` is already covered by an approval grant in `security`, so the
/// harness can skip re-prompting. Checks, in order: a session-wide tool
/// approval, an exact `(tool, parameters)` grant, a `(tool, path)` scope, and
/// (for `shell_exec`) a normalized-command grant.
pub(crate) fn tool_call_has_confirmation_grant(call: &ToolCall, security: &SecurityConfig) -> bool {
    if security
        .approved_session_tools
        .iter()
        .any(|tool| tool == &call.name)
    {
        return true;
    }

    let call_key = approved_tool_call_key(&call.name, &call.parameters);
    if security
        .approved_tool_calls
        .iter()
        .any(|approved| approved == &call_key)
    {
        return true;
    }

    if let Some(path) = call
        .parameters
        .get("path")
        .and_then(serde_json::Value::as_str)
    {
        let path_key = approved_tool_path_key(&call.name, path);
        if security
            .approved_tool_path_scopes
            .iter()
            .any(|approved| approved == &path_key)
        {
            return true;
        }
    }

    if call.name == "shell_exec"
        && let Some(command) = call
            .parameters
            .get("command")
            .and_then(serde_json::Value::as_str)
    {
        let normalized = normalize_shell_command_for_approval(command);
        if security
            .approved_shell_commands
            .iter()
            .any(|approved| approved == &normalized)
        {
            return true;
        }
    }

    false
}

fn approved_tool_call_key(tool_name: &str, parameters: &serde_json::Value) -> String {
    let encoded = serde_json::to_string(parameters).unwrap_or_else(|_| parameters.to_string());
    format!("{tool_name}:{encoded}")
}

fn approved_tool_path_key(tool_name: &str, path: &str) -> String {
    format!("{tool_name}:{path}")
}

fn normalize_shell_command_for_approval(command: &str) -> String {
    command.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn call(name: &str, parameters: serde_json::Value) -> ToolCall {
        ToolCall::new(name, parameters)
    }

    #[test]
    fn session_wide_tool_approval_grants() {
        let security = SecurityConfig {
            approved_session_tools: vec!["file_write".to_string()],
            ..SecurityConfig::default()
        };
        assert!(tool_call_has_confirmation_grant(
            &call("file_write", serde_json::json!({"path": "a.rs"})),
            &security
        ));
        assert!(!tool_call_has_confirmation_grant(
            &call("file_delete", serde_json::json!({"path": "a.rs"})),
            &security
        ));
    }

    #[test]
    fn exact_call_key_and_path_scope_grant() {
        let params = serde_json::json!({"path": "src/lib.rs"});
        let security = SecurityConfig {
            approved_tool_calls: vec![approved_tool_call_key("file_write", &params)],
            approved_tool_path_scopes: vec![approved_tool_path_key("file_delete", "target")],
            ..SecurityConfig::default()
        };
        // Exact (tool, parameters) grant.
        assert!(tool_call_has_confirmation_grant(
            &call("file_write", params.clone()),
            &security
        ));
        // (tool, path) scope grant.
        assert!(tool_call_has_confirmation_grant(
            &call("file_delete", serde_json::json!({"path": "target"})),
            &security
        ));
        // Same tool, different path → not granted.
        assert!(!tool_call_has_confirmation_grant(
            &call("file_delete", serde_json::json!({"path": "src"})),
            &security
        ));
    }

    #[test]
    fn shell_command_grant_ignores_whitespace_differences() {
        let security = SecurityConfig {
            approved_shell_commands: vec!["cargo test --all".to_string()],
            ..SecurityConfig::default()
        };
        // Extra/normalized whitespace still matches the stored grant.
        assert!(tool_call_has_confirmation_grant(
            &call(
                "shell_exec",
                serde_json::json!({"command": "cargo   test    --all"})
            ),
            &security
        ));
        assert!(!tool_call_has_confirmation_grant(
            &call("shell_exec", serde_json::json!({"command": "cargo build"})),
            &security
        ));
    }
}
