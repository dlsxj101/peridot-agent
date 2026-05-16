use std::fs;
use std::path::Path;

use async_trait::async_trait;
use peridot_common::{PeriError, PeriResult, PermissionLevel, ToolGroup, ToolResult};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::path::ensure_within_project;
use crate::{Tool, ToolContext};

/// Built-in plan creation tool.
#[derive(Clone, Debug)]
pub struct PlanCreateTool;

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub(crate) struct PlanFile {
    pub(crate) objective: String,
    pub(crate) steps: Vec<PlanStep>,
    pub(crate) updates: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub(crate) struct PlanStep {
    pub(crate) id: usize,
    pub(crate) text: String,
    pub(crate) status: String,
}

#[async_trait]
impl Tool for PlanCreateTool {
    fn name(&self) -> &str {
        "plan_create"
    }

    fn group(&self) -> ToolGroup {
        ToolGroup::Plan
    }

    fn description(&self) -> &str {
        "Create a todo.md plan in the project root"
    }

    fn parameters_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "objective": {"type": "string", "description": "High-level objective for the plan"},
                "steps": {
                    "type": "array",
                    "items": {
                        "oneOf": [
                            {"type": "string"},
                            {
                                "type": "object",
                                "properties": {"text": {"type": "string"}},
                                "required": ["text"]
                            }
                        ]
                    },
                    "description": "Ordered list of plan steps"
                }
            },
            "required": ["objective", "steps"],
            "additionalProperties": false,
        })
    }

    async fn execute(&self, params: Value, ctx: &ToolContext) -> PeriResult<ToolResult> {
        let objective = params
            .get("objective")
            .and_then(Value::as_str)
            .unwrap_or("Peridot task");
        let steps = params
            .get("steps")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let plan = PlanFile {
            objective: objective.to_string(),
            steps: steps
                .iter()
                .enumerate()
                .map(|(idx, step)| PlanStep {
                    id: idx + 1,
                    text: plan_step_text(step),
                    status: "pending".to_string(),
                })
                .collect(),
            updates: Vec::new(),
        };
        let markdown_path =
            ensure_within_project(&ctx.project_root, &ctx.project_root.join("todo.md"))?;
        let json_path =
            ensure_within_project(&ctx.project_root, &ctx.project_root.join("todo.json"))?;
        fs::write(&markdown_path, render_plan_markdown(&plan)).map_err(|err| {
            PeriError::Tool(format!(
                "failed to write {}: {err}",
                markdown_path.display()
            ))
        })?;
        write_plan_json(&json_path, &plan)?;
        Ok(ToolResult::success(
            "created todo.md and todo.json",
            serde_json::json!({ "markdown_path": markdown_path, "json_path": json_path }),
        ))
    }

    fn permission_level(&self) -> PermissionLevel {
        PermissionLevel::Write
    }

    fn can_run_concurrent(&self) -> bool {
        false
    }
}

/// Built-in plan update tool.
#[derive(Clone, Debug)]
pub struct PlanUpdateTool;

#[async_trait]
impl Tool for PlanUpdateTool {
    fn name(&self) -> &str {
        "plan_update"
    }

    fn group(&self) -> ToolGroup {
        ToolGroup::Plan
    }

    fn description(&self) -> &str {
        "Append a short progress update to todo.md"
    }

    fn parameters_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "update": {"type": "string", "description": "Short progress note appended to the plan"},
                "step": {"type": "integer", "minimum": 1, "description": "1-based step index to mark"},
                "status": {
                    "type": "string",
                    "enum": ["pending", "in_progress", "done"],
                    "description": "Status to set on the named step"
                }
            },
            "additionalProperties": false,
        })
    }

    async fn execute(&self, params: Value, ctx: &ToolContext) -> PeriResult<ToolResult> {
        let update = params.get("update").and_then(Value::as_str).unwrap_or("");
        let markdown_path =
            ensure_within_project(&ctx.project_root, &ctx.project_root.join("todo.md"))?;
        let json_path =
            ensure_within_project(&ctx.project_root, &ctx.project_root.join("todo.json"))?;
        let mut plan = read_plan_file(&json_path).unwrap_or_else(|| PlanFile {
            objective: "Peridot task".to_string(),
            steps: Vec::new(),
            updates: Vec::new(),
        });
        if let Some(step_id) = params.get("step").and_then(Value::as_u64) {
            let status = params
                .get("status")
                .and_then(Value::as_str)
                .unwrap_or("done")
                .to_string();
            if let Some(step) = plan
                .steps
                .iter_mut()
                .find(|step| step.id == step_id as usize)
            {
                step.status = status;
            }
        }
        if !update.trim().is_empty() {
            plan.updates.push(update.to_string());
        }
        fs::write(&markdown_path, render_plan_markdown(&plan)).map_err(|err| {
            PeriError::Tool(format!(
                "failed to write {}: {err}",
                markdown_path.display()
            ))
        })?;
        write_plan_json(&json_path, &plan)?;
        Ok(ToolResult::success(
            "updated todo.md and todo.json",
            serde_json::json!({ "markdown_path": markdown_path, "json_path": json_path }),
        ))
    }

    fn permission_level(&self) -> PermissionLevel {
        PermissionLevel::Write
    }

    fn can_run_concurrent(&self) -> bool {
        false
    }
}

fn plan_step_text(value: &Value) -> String {
    value
        .as_str()
        .or_else(|| value.get("text").and_then(Value::as_str))
        .unwrap_or("unnamed step")
        .to_string()
}

fn read_plan_file(path: &Path) -> Option<PlanFile> {
    fs::read_to_string(path)
        .ok()
        .and_then(|content| serde_json::from_str(&content).ok())
}

fn write_plan_json(path: &Path, plan: &PlanFile) -> PeriResult<()> {
    let content = serde_json::to_string_pretty(plan)
        .map_err(|err| PeriError::Parse(format!("failed to serialize plan: {err}")))?;
    fs::write(path, content)
        .map_err(|err| PeriError::Tool(format!("failed to write {}: {err}", path.display())))
}

fn render_plan_markdown(plan: &PlanFile) -> String {
    let mut markdown = format!("# Plan\n\nObjective: {}\n\n", plan.objective);
    for step in &plan.steps {
        markdown.push_str(&format!(
            "{}. [{}] {}\n",
            step.id,
            markdown_status_marker(&step.status),
            step.text
        ));
    }
    if !plan.updates.is_empty() {
        markdown.push_str("\n## Updates\n");
        for update in &plan.updates {
            markdown.push_str(&format!("- {update}\n"));
        }
    }
    markdown
}

fn markdown_status_marker(status: &str) -> &'static str {
    match status {
        "done" | "completed" => "x",
        "in_progress" | "active" => ">",
        _ => " ",
    }
}
