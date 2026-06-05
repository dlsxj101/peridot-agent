//! Assistant-message display parsing.
//!
//! The assistant turn is provider text that may end in a JSON action block
//! (`{"action": …, "parameters": …}`). This module extracts the user-visible
//! line from it: an `agent_ask_user` surfaces its question, `agent_done` its
//! summary, plain `agent_message` / `respond` its text, and tool-call actions
//! produce nothing (the tool events already render them). Free-form text with
//! no JSON action is shown as-is. Split out of `state.rs` so the (JSON-aware)
//! parsing lives in one place; [`TuiState`] calls [`parse_assistant_content`]
//! when recording an assistant transcript entry.
//!
//! [`TuiState`]: super::TuiState

/// Parsed view of an assistant message split into a user-visible line and the
/// raw payload.
pub(crate) struct ParsedAssistant {
    pub(crate) display: Option<String>,
}

/// Extracts the user-visible portion of an assistant message.
///
/// If the message ends in a JSON action block, the action drives what (if
/// anything) is shown: `agent_ask_user` surfaces the question, `agent_done` the
/// summary, and tool-call actions produce no visible line because the tool
/// events already report them. Free-form text without a JSON action is shown
/// as-is.
pub(crate) fn parse_assistant_content(content: &str) -> ParsedAssistant {
    if let Some(json_str) = last_balanced_json_object(content)
        && let Ok(value) = serde_json::from_str::<serde_json::Value>(&json_str)
        && let Some(action) = value.get("action").and_then(serde_json::Value::as_str)
    {
        let params = value.get("parameters");
        let display = match action {
            "agent_ask_user" => params
                .and_then(|p| {
                    p.get("question")
                        .or_else(|| p.get("prompt"))
                        .or_else(|| p.get("message"))
                })
                .and_then(serde_json::Value::as_str)
                .map(|text| format!("ask: {text}")),
            "agent_done" => Some(
                params
                    .and_then(|p| p.get("summary").or_else(|| p.get("message")))
                    .and_then(serde_json::Value::as_str)
                    .map(|text| format!("done: {text}"))
                    .unwrap_or_else(|| "done".to_string()),
            ),
            "agent_message" | "respond" | "reply" => params
                .and_then(|p| {
                    p.get("message")
                        .or_else(|| p.get("text"))
                        .or_else(|| p.get("content"))
                })
                .and_then(serde_json::Value::as_str)
                .map(str::to_string),
            _ => None,
        };
        return ParsedAssistant { display };
    }
    ParsedAssistant {
        display: Some(content.to_string()),
    }
}

/// Returns the textual representation of the last balanced top-level JSON object
/// in `text`.
fn last_balanced_json_object(text: &str) -> Option<String> {
    let bytes = text.as_bytes();
    let mut end: Option<usize> = None;
    let mut in_string = false;
    let mut escaped = false;
    for (idx, &byte) in bytes.iter().enumerate() {
        if in_string {
            if escaped {
                escaped = false;
            } else if byte == b'\\' {
                escaped = true;
            } else if byte == b'"' {
                in_string = false;
            }
            continue;
        }
        match byte {
            b'"' => in_string = true,
            b'}' => end = Some(idx),
            _ => {}
        }
    }
    let end = end?;
    let mut depth: usize = 0;
    let mut in_string = false;
    let mut start: Option<usize> = None;
    for idx in (0..=end).rev() {
        let byte = bytes[idx];
        if in_string {
            if byte == b'"' && !is_escaped_quote(bytes, idx) {
                in_string = false;
            }
            continue;
        }
        if byte == b'"' && !is_escaped_quote(bytes, idx) {
            in_string = true;
            continue;
        }
        match byte {
            b'}' => depth += 1,
            b'{' => {
                if depth == 0 {
                    break;
                }
                depth -= 1;
                if depth == 0 {
                    start = Some(idx);
                    break;
                }
            }
            _ => {}
        }
    }
    let start = start?;
    Some(text[start..=end].to_string())
}

fn is_escaped_quote(bytes: &[u8], idx: usize) -> bool {
    let mut backslashes = 0usize;
    let mut cursor = idx;
    while cursor > 0 && bytes[cursor - 1] == b'\\' {
        backslashes += 1;
        cursor -= 1;
    }
    backslashes % 2 == 1
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn agent_done_action_surfaces_its_summary() {
        let content =
            r#"Here you go. {"action": "agent_done", "parameters": {"summary": "all set"}}"#;
        assert_eq!(
            parse_assistant_content(content).display.as_deref(),
            Some("done: all set")
        );
    }

    #[test]
    fn free_form_text_is_shown_as_is() {
        let content = "just a normal reply with no action";
        assert_eq!(
            parse_assistant_content(content).display.as_deref(),
            Some("just a normal reply with no action")
        );
    }

    #[test]
    fn tool_call_action_produces_no_visible_line() {
        let content = r#"{"action": "shell_exec", "parameters": {"command": "ls"}}"#;
        assert_eq!(parse_assistant_content(content).display, None);
    }

    #[test]
    fn last_balanced_object_ignores_braces_inside_strings() {
        // The `}` inside the string must not be treated as the object end.
        let text = r#"prefix {"action": "agent_message", "parameters": {"message": "a } b"}}"#;
        assert_eq!(
            parse_assistant_content(text).display.as_deref(),
            Some("a } b")
        );
    }
}
