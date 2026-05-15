pub fn parse_action(text: &str) -> PeriResult<ParsedAction> {
    if let Ok(value) = serde_json::from_str::<Value>(text) {
        return parse_action_value(value);
    }

    if let Some(block) = first_json_code_block(text)
        && let Ok(value) = serde_json::from_str::<Value>(block)
    {
        return parse_action_value(value);
    }

    if let Some(object) = first_json_object(text)
        && let Ok(value) = serde_json::from_str::<Value>(&object)
    {
        return parse_action_value(value);
    }

    if let Some(action) = extract_action_key(text) {
        return Ok(ParsedAction {
            thinking: None,
            tool_call: ToolCall::new(action, Value::Null),
        });
    }

    Err(PeriError::Parse(
        "assistant response did not contain a recoverable action".to_string(),
    ))
}

fn parse_action_value(value: Value) -> PeriResult<ParsedAction> {
    let action = value
        .get("action")
        .and_then(Value::as_str)
        .ok_or_else(|| PeriError::Parse("missing action field".to_string()))?;
    let thinking = value
        .get("thinking")
        .and_then(Value::as_str)
        .map(str::to_string);
    let parameters = value.get("parameters").cloned().unwrap_or(Value::Null);
    Ok(ParsedAction {
        thinking,
        tool_call: ToolCall::new(action, parameters),
    })
}

fn first_json_code_block(text: &str) -> Option<&str> {
    let start = text.find("```")?;
    let after_fence = &text[start + 3..];
    let content_start = after_fence.find('\n').map_or(0, |idx| idx + 1);
    let after_lang = &after_fence[content_start..];
    let end = after_lang.find("```")?;
    Some(after_lang[..end].trim())
}

pub(crate) fn first_json_object(text: &str) -> Option<String> {
    let start = text.find('{')?;
    let mut depth = 0usize;
    let mut in_string = false;
    let mut escaped = false;

    for (offset, ch) in text[start..].char_indices() {
        if in_string {
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == '"' {
                in_string = false;
            }
            continue;
        }

        match ch {
            '"' => in_string = true,
            '{' => depth += 1,
            '}' => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    return Some(text[start..start + offset + ch.len_utf8()].to_string());
                }
            }
            _ => {}
        }
    }

    None
}

fn extract_action_key(text: &str) -> Option<String> {
    let needle = "\"action\"";
    let start = text.find(needle)? + needle.len();
    let after_key = text[start..].trim_start();
    let after_colon = after_key.strip_prefix(':')?.trim_start();
    let action = after_colon.strip_prefix('"')?;
    let end = action.find('"')?;
    Some(action[..end].to_string())
}

/// Provider pricing information.
use peridot_common::{PeriError, PeriResult, ToolCall};
use serde_json::Value;

use crate::ParsedAction;
