use super::*;

pub(super) fn task_to_tool_call(task: &str) -> Option<ToolCall> {
    if let Ok(parsed) = parse_action(task) {
        return Some(parsed.tool_call);
    }

    let lower = task.to_lowercase();
    if lower.contains("hello.py") && lower.contains("hello world") {
        return Some(ToolCall::new(
            "file_write",
            serde_json::json!({
                "path": "hello.py",
                "content": "print(\"Hello World\")\n"
            }),
        ));
    }

    None
}
