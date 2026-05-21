use std::fs;
use std::path::{Path, PathBuf};

use async_trait::async_trait;
use peridot_agents::{
    LocalSubAgentRunner, ModelTier, SubAgent, SubAgentKind, SubAgentPolicy, SubAgentTask,
};
use peridot_common::{
    AskUserAnswer, AskUserRequest, PeriError, PeriResult, PermissionLevel, ToolGroup, ToolResult,
    peridot_home_dir,
};
use peridot_memory::{ErrorResolution, MemoryStore, StoredSkill};
use serde::Serialize;
use serde_json::Value;

use crate::hooks::{HookRunner, HookVariables};
use crate::path::{ensure_within_project, required_str};
use crate::{Tool, ToolContext};

/// Built-in scratchpad tool.
#[derive(Clone, Debug)]
pub struct AgentScratchpadTool;

#[async_trait]
impl Tool for AgentScratchpadTool {
    fn name(&self) -> &str {
        "agent_scratchpad"
    }

    fn group(&self) -> ToolGroup {
        ToolGroup::Agent
    }

    fn description(&self) -> &str {
        "Append a note to the project-local scratchpad"
    }

    fn parameters_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "note": {"type": "string", "description": "Text appended to .peridot/scratchpad.md"}
            },
            "required": ["note"],
            "additionalProperties": false,
        })
    }

    async fn execute(&self, params: Value, ctx: &ToolContext) -> PeriResult<ToolResult> {
        let note = required_str(&params, "note")?;
        let dir = ensure_within_project(&ctx.project_root, &ctx.project_root.join(".peridot"))?;
        fs::create_dir_all(&dir)
            .map_err(|err| PeriError::Tool(format!("failed to create {}: {err}", dir.display())))?;
        let path = ensure_within_project(&ctx.project_root, &dir.join("scratchpad.md"))?;
        let mut content = fs::read_to_string(&path).unwrap_or_default();
        content.push_str(note);
        content.push('\n');
        fs::write(&path, content)
            .map_err(|err| PeriError::Tool(format!("failed to write {}: {err}", path.display())))?;
        Ok(ToolResult::success(
            "updated scratchpad",
            serde_json::json!({ "path": path }),
        ))
    }

    fn permission_level(&self) -> PermissionLevel {
        PermissionLevel::Write
    }

    fn can_run_concurrent(&self) -> bool {
        false
    }
}

/// Built-in user question tool with deterministic fallback behavior.
#[derive(Clone, Debug)]
pub struct AgentAskUserTool;

#[async_trait]
impl Tool for AgentAskUserTool {
    fn name(&self) -> &str {
        "agent_ask_user"
    }

    fn group(&self) -> ToolGroup {
        ToolGroup::Agent
    }

    fn description(&self) -> &str {
        "Ask the user a question only when a decision cannot be safely inferred. Do NOT use for greetings or small talk; reply with a plain text message instead."
    }

    fn parameters_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "question": {"type": "string", "description": "The question shown to the user"},
                "kind": {
                    "type": "string",
                    "enum": ["free_form", "single_select"],
                    "description": "Whether the answer is free-form text or a choice from `choices`"
                },
                "choices": {
                    "type": "array",
                    "items": {"type": "string"},
                    "description": "Discrete options when kind is single_select"
                },
                "default_index": {
                    "type": "integer",
                    "minimum": 0,
                    "description": "Default option index for single_select"
                },
                "default": {"type": "string", "description": "Default answer for free_form"},
                "explanation": {"type": "string", "description": "Optional rationale shown to the user"}
            },
            "required": ["question"],
            "additionalProperties": false,
        })
    }

    async fn execute(&self, params: Value, ctx: &ToolContext) -> PeriResult<ToolResult> {
        let question = required_str(&params, "question")?;
        let kind = ask_user_kind(&params);
        let choices = ask_user_choices(&params);
        let default_index = params
            .get("default_index")
            .and_then(Value::as_u64)
            .map(|value| value as usize);
        let display_choices = ask_user_display_choices(&kind, &choices);
        let explanation = params
            .get("explanation")
            .and_then(Value::as_str)
            .unwrap_or("Peridot needs this answer to continue without guessing.")
            .to_string();
        run_ask_user_triggered_hook(ctx, question, &kind)?;

        // When the harness has wired in an interactive `AskUserPort`
        // (TUI, REPL, etc), forward the request and await the real
        // answer. Headless / mock / test paths leave the port unset and
        // fall back to the synthesised default below so existing
        // behaviour is preserved.
        if let Some(port) = ctx.ask_user_port.clone() {
            let request = build_ask_user_request(question, &kind, &choices, default_index, &params);
            let answer = port.ask(request.clone()).await;
            let resolved_default = default_ask_user_answer(&params, &choices, default_index);
            let (answer_text, source) = match &answer {
                AskUserAnswer::Cancelled => (resolved_default, "default"),
                other => (other.to_display_string(&request), "user"),
            };
            return Ok(ToolResult::success(
                if answer_text.is_empty() {
                    format!("asked user: {question}")
                } else {
                    format!("asked user: {question} -> {answer_text}")
                },
                serde_json::json!({
                    "question": question,
                    "kind": kind,
                    "choices": choices,
                    "display_choices": display_choices,
                    "default_index": default_index,
                    "explanation": explanation,
                    "answer": answer_text,
                    "source": source,
                }),
            ));
        }

        let answer = default_ask_user_answer(&params, &choices, default_index);
        Ok(ToolResult::success(
            if answer.is_empty() {
                format!("asked user: {question}")
            } else {
                format!("asked user: {question} -> {answer}")
            },
            serde_json::json!({
                "question": question,
                "kind": kind,
                "choices": choices,
                "display_choices": display_choices,
                "default_index": default_index,
                "explanation": explanation,
                "answer": answer,
                "source": "default"
            }),
        ))
    }

    fn validate_params(&self, params: &Value) -> PeriResult<()> {
        let _ = required_str(params, "question")?;
        Ok(())
    }

    fn permission_level(&self) -> PermissionLevel {
        PermissionLevel::Read
    }
}

fn run_ask_user_triggered_hook(ctx: &ToolContext, question: &str, kind: &str) -> PeriResult<()> {
    let mut variables = HookVariables::new();
    variables.insert(
        "project_root".to_string(),
        ctx.project_root.display().to_string(),
    );
    variables.insert(
        "workspace".to_string(),
        ctx.project_root.display().to_string(),
    );
    variables.insert("question".to_string(), question.to_string());
    variables.insert("kind".to_string(), kind.to_string());
    HookRunner::new(&ctx.project_root, ctx.hooks.clone())
        .run_event_hooks("ask_user_triggered", &variables)?;
    Ok(())
}

fn first_choice(params: &Value) -> Option<&str> {
    params
        .get("choices")
        .or_else(|| params.get("options"))
        .and_then(Value::as_array)
        .and_then(|choices| choices.first())
        .and_then(Value::as_str)
}

fn ask_user_kind(params: &Value) -> String {
    params
        .get("kind")
        .or_else(|| params.get("type"))
        .and_then(Value::as_str)
        .unwrap_or_else(|| {
            if ask_user_choices(params).is_empty() {
                "free_form"
            } else {
                "single_select"
            }
        })
        .to_string()
}

fn ask_user_choices(params: &Value) -> Vec<String> {
    params
        .get("choices")
        .or_else(|| params.get("options"))
        .and_then(Value::as_array)
        .map(|choices| {
            choices
                .iter()
                .filter_map(Value::as_str)
                .map(ToString::to_string)
                .collect()
        })
        .unwrap_or_default()
}

fn default_ask_user_answer(
    params: &Value,
    choices: &[String],
    default_index: Option<usize>,
) -> String {
    if let Some(default) = params.get("default").and_then(Value::as_str) {
        return default.to_string();
    }
    if let Some(index) = default_index
        && let Some(choice) = choices.get(index)
    {
        return choice.clone();
    }
    first_choice(params).unwrap_or("").to_string()
}

fn ask_user_display_choices(kind: &str, choices: &[String]) -> Vec<String> {
    if choices.is_empty() || kind == "free_form" {
        return Vec::new();
    }
    choices
        .iter()
        .cloned()
        .chain(["[o] Other".to_string(), "[?] Explain".to_string()])
        .collect()
}

/// Build the structured `AskUserRequest` sent to an interactive port.
/// Tool parameters keep their loose JSON shape for backwards
/// compatibility; this function pins them down to one of the three
/// canonical variants.
fn build_ask_user_request(
    question: &str,
    kind: &str,
    choices: &[String],
    default_index: Option<usize>,
    params: &Value,
) -> AskUserRequest {
    match kind {
        "single_select" => AskUserRequest::SingleSelect {
            question: question.to_string(),
            options: choices.to_vec(),
            default_index,
        },
        "multi_select" => AskUserRequest::MultiSelect {
            question: question.to_string(),
            options: choices.to_vec(),
            min: params
                .get("min")
                .and_then(Value::as_u64)
                .map(|value| value as usize)
                .unwrap_or(0),
            max: params
                .get("max")
                .and_then(Value::as_u64)
                .map(|value| value as usize),
        },
        _ => AskUserRequest::FreeForm {
            question: question.to_string(),
            hint: params
                .get("hint")
                .and_then(Value::as_str)
                .map(str::to_string),
            default: params
                .get("default")
                .and_then(Value::as_str)
                .map(str::to_string),
        },
    }
}

/// Built-in subagent delegation tool.
#[derive(Clone, Debug)]
pub struct AgentDelegateTool;

#[async_trait]
impl Tool for AgentDelegateTool {
    fn name(&self) -> &str {
        "agent_delegate"
    }

    fn group(&self) -> ToolGroup {
        ToolGroup::Agent
    }

    fn description(&self) -> &str {
        "Prepare a fork, worktree, or teammate subagent for a bounded task"
    }

    fn parameters_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "prompt": {"type": "string", "description": "Task description handed to the subagent"},
                "kind": {
                    "type": "string",
                    "enum": ["fork", "worktree", "teammate"],
                    "description": "Subagent isolation kind"
                },
                "model_tier": {
                    "type": "string",
                    "enum": ["haiku", "main", "opus"],
                    "description": "Cost/capability tier for the subagent"
                }
            },
            "required": ["prompt"],
            "additionalProperties": false,
        })
    }

    async fn execute(&self, params: Value, ctx: &ToolContext) -> PeriResult<ToolResult> {
        let prompt = required_str(&params, "prompt")?.to_string();
        let (kind, model_tier) = subagent_selection(&params, &prompt)?;
        let task = SubAgentTask {
            prompt: prompt.clone(),
            kind: kind.clone(),
            model_tier: Some(model_tier),
        };
        let result = match dispatch_subagent(ctx, task).await {
            Ok(result) => result,
            Err(err) => {
                run_subagent_failed_hook(ctx, &kind, &prompt, &err.to_string())?;
                return Err(err);
            }
        };
        run_subagent_completed_hook(ctx, &result.kind, &prompt)?;
        Ok(ToolResult::success(
            result.summary.clone(),
            serde_json::json!(result),
        ))
    }

    fn validate_params(&self, params: &Value) -> PeriResult<()> {
        let _ = required_str(params, "prompt")?;
        Ok(())
    }

    fn permission_level(&self) -> PermissionLevel {
        PermissionLevel::Write
    }

    fn can_run_concurrent(&self) -> bool {
        false
    }
}

async fn dispatch_subagent(
    ctx: &ToolContext,
    task: SubAgentTask,
) -> PeriResult<peridot_agents::SubAgentResult> {
    if let Some(runner) = ctx.runner.clone() {
        return runner.run(task).await;
    }
    let fallback = LocalSubAgentRunner::new(
        &ctx.project_root,
        ctx.project_root.join(".peridot/worktrees"),
    );
    fallback.run(task).await
}

fn subagent_selection(params: &Value, prompt: &str) -> PeriResult<(SubAgentKind, ModelTier)> {
    let policy = SubAgentPolicy;
    let (default_kind, default_tier) = policy.select(prompt);
    let kind = match params.get("kind").and_then(Value::as_str) {
        Some("fork") => SubAgentKind::Fork,
        Some("worktree") => SubAgentKind::Worktree,
        Some("teammate") => SubAgentKind::Teammate,
        Some(value) => {
            return Err(PeriError::Config(format!(
                "unsupported subagent kind: {value}"
            )));
        }
        None => default_kind,
    };
    let tier = match params.get("model_tier").and_then(Value::as_str) {
        Some("haiku") => ModelTier::Haiku,
        Some("main") => ModelTier::Main,
        Some("opus") => ModelTier::Opus,
        Some(value) => {
            return Err(PeriError::Config(format!(
                "unsupported subagent model tier: {value}"
            )));
        }
        None => default_tier,
    };
    Ok((kind, tier))
}

fn run_subagent_completed_hook(
    ctx: &ToolContext,
    kind: &SubAgentKind,
    task: &str,
) -> PeriResult<()> {
    run_subagent_event_hook(ctx, "subagent_completed", kind, task, None)
}

fn run_subagent_failed_hook(
    ctx: &ToolContext,
    kind: &SubAgentKind,
    task: &str,
    error_message: &str,
) -> PeriResult<()> {
    run_subagent_event_hook(ctx, "subagent_failed", kind, task, Some(error_message))
}

fn run_subagent_event_hook(
    ctx: &ToolContext,
    event: &str,
    kind: &SubAgentKind,
    task: &str,
    error_message: Option<&str>,
) -> PeriResult<()> {
    let mut variables = HookVariables::new();
    variables.insert(
        "project_root".to_string(),
        ctx.project_root.display().to_string(),
    );
    variables.insert(
        "workspace".to_string(),
        ctx.project_root.display().to_string(),
    );
    variables.insert("agent_type".to_string(), format!("{kind:?}").to_lowercase());
    variables.insert("task".to_string(), task.to_string());
    if let Some(error_message) = error_message {
        variables.insert("error_message".to_string(), error_message.to_string());
    }
    HookRunner::new(&ctx.project_root, ctx.hooks.clone()).run_event_hooks(event, &variables)?;
    Ok(())
}

/// Built-in memory search tool.
#[derive(Clone, Debug)]
pub struct AgentMemorySearchTool;

#[derive(Clone, Debug, Serialize)]
pub(crate) struct MemoryLayerSearchResult {
    pub(crate) scope: String,
    pub(crate) skills: Vec<StoredSkill>,
    pub(crate) error_resolution: Option<ErrorResolution>,
}

/// Body-free skill summary returned by `agent_memory_search`. The model
/// reads these to decide which skill is worth pulling in full via
/// `skill_view`.
#[derive(Clone, Debug, Serialize)]
struct SkillSearchSummary {
    name: String,
    scope: String,
    description: String,
}

#[async_trait]
impl Tool for AgentMemorySearchTool {
    fn name(&self) -> &str {
        "agent_memory_search"
    }

    fn group(&self) -> ToolGroup {
        ToolGroup::Agent
    }

    fn description(&self) -> &str {
        "Search project and global learned skills and known error resolutions"
    }

    fn parameters_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "query": {"type": "string", "description": "Search query against memory layers"}
            },
            "required": ["query"],
            "additionalProperties": false,
        })
    }

    async fn execute(&self, params: Value, ctx: &ToolContext) -> PeriResult<ToolResult> {
        let query = required_str(&params, "query")?;
        let layers = search_memory_layers(&ctx.project_root, query)?;
        // L0 disclosure: return skill metadata only (name/scope/description)
        // and let the model pull bodies via `skill_view` when needed. This
        // keeps the token cost of a search proportional to skill count, not
        // total body size.
        let skills: Vec<SkillSearchSummary> = layers
            .iter()
            .flat_map(|layer| layer.skills.iter())
            .map(|skill| SkillSearchSummary {
                name: skill.name.clone(),
                scope: skill.scope.clone(),
                description: skill
                    .body
                    .lines()
                    .next()
                    .unwrap_or("")
                    .trim_start_matches('#')
                    .trim()
                    .to_string(),
            })
            .collect();
        let error_resolutions = layers
            .iter()
            .filter_map(|layer| layer.error_resolution.clone())
            .collect::<Vec<_>>();
        Ok(ToolResult::success(
            format!(
                "memory search returned {} skill summaries and {} error resolutions across {} layers (use skill_view for bodies)",
                skills.len(),
                error_resolutions.len(),
                layers.len()
            ),
            serde_json::json!({
                "query": query,
                "skills": skills,
                "error_resolutions": error_resolutions,
                "layers": layers
            }),
        ))
    }

    fn validate_params(&self, params: &Value) -> PeriResult<()> {
        let _ = required_str(params, "query")?;
        Ok(())
    }

    fn permission_level(&self) -> PermissionLevel {
        PermissionLevel::Read
    }
}

fn search_memory_layers(
    project_root: &Path,
    query: &str,
) -> PeriResult<Vec<MemoryLayerSearchResult>> {
    let mut layers = Vec::new();
    let project_path = project_root.join(".peridot/memory.db");
    layers.push(search_memory_layer("project", project_path, query)?);
    if let Some(global_path) = global_memory_path()
        && global_path != project_root.join(".peridot/memory.db")
        && global_path.exists()
    {
        layers.push(search_memory_layer("global", global_path, query)?);
    }
    Ok(layers)
}

pub(crate) fn search_memory_layer(
    scope: &str,
    path: PathBuf,
    query: &str,
) -> PeriResult<MemoryLayerSearchResult> {
    let store = MemoryStore::new(path);
    Ok(MemoryLayerSearchResult {
        scope: scope.to_string(),
        skills: store.search_skills(query)?,
        error_resolution: store.get_error_resolution(query)?,
    })
}

fn global_memory_path() -> Option<PathBuf> {
    peridot_home_dir().map(|home| home.join("memory.db"))
}

/// Built-in completion declaration tool.
#[derive(Clone, Debug)]
pub struct AgentDoneTool;

#[async_trait]
impl Tool for AgentDoneTool {
    fn name(&self) -> &str {
        "agent_done"
    }

    fn group(&self) -> ToolGroup {
        ToolGroup::Agent
    }

    fn description(&self) -> &str {
        "Declare the active task complete. Use this both when finishing real work AND when responding to a casual user message (use `summary` for the reply text)."
    }

    fn parameters_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "summary": {"type": "string", "description": "Short summary of the work completed or the reply shown to the user"}
            },
            "required": ["summary"],
            "additionalProperties": false,
        })
    }

    async fn execute(&self, params: Value, _ctx: &ToolContext) -> PeriResult<ToolResult> {
        let summary = params
            .get("summary")
            .and_then(Value::as_str)
            .unwrap_or("done")
            .to_string();
        Ok(ToolResult::success(summary, Value::Null))
    }

    fn permission_level(&self) -> PermissionLevel {
        PermissionLevel::Read
    }
}

/// Built-in inter-subagent messaging tool. Routes a short note from the
/// current session to its `parent` or to a named `child:<session_id>`.
/// Requires `ctx.message_bus` to be wired in by the harness — when it
/// isn't (single-session run, tests), the tool returns a polite noop so
/// the model still gets a tool result instead of a hard error.
///
/// Schema:
/// ```json
/// {
///   "target": "parent" | "child:<session_id>",
///   "message": "<body, plain text>"
/// }
/// ```
#[derive(Clone, Debug)]
pub struct AgentMessageTool;

#[async_trait]
impl Tool for AgentMessageTool {
    fn name(&self) -> &str {
        "agent_message"
    }

    fn group(&self) -> ToolGroup {
        ToolGroup::Agent
    }

    fn description(&self) -> &str {
        "Send a short note to a parent or child subagent session. \
         Use `target: \"parent\"` to notify the spawning session, or \
         `target: \"child:<session_id>\"` to message a named child. \
         Messages are delivered to the recipient's inbox and surface at \
         the start of their next turn as a `[parent message]` / \
         `[child message]` PlanReminder. Do NOT use for greetings; this \
         is for coordination signals like 'pause', 'stop', 'report status', \
         or short context handoffs."
    }

    fn parameters_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "target": {
                    "type": "string",
                    "description": "Recipient: \"parent\" or \"child:<session_id>\"",
                },
                "message": {
                    "type": "string",
                    "description": "Body of the note (plain text, ≤ 500 chars recommended)",
                },
            },
            "required": ["target", "message"],
            "additionalProperties": false,
        })
    }

    async fn execute(&self, params: Value, ctx: &ToolContext) -> PeriResult<ToolResult> {
        let target = required_str(&params, "target")?.to_string();
        let message = required_str(&params, "message")?.to_string();
        let Some(bus) = ctx.message_bus.clone() else {
            // No bus wired in — common in single-session runs and tests.
            // Return success with a clear hint so the model can see why
            // its note didn't go anywhere.
            return Ok(ToolResult::success(
                "agent_message: no bus configured (single-session run); note was not delivered"
                    .to_string(),
                serde_json::json!({ "delivered": false, "target": target }),
            ));
        };
        let from = bus
            .current_session_id()
            .unwrap_or_else(|| "anonymous".to_string());
        if target == "parent" {
            match bus.send_to_parent(&from, &message).await {
                Ok(parent_id) => Ok(ToolResult::success(
                    format!("agent_message delivered to parent {parent_id}"),
                    serde_json::json!({
                        "delivered": true,
                        "target": "parent",
                        "parent_id": parent_id,
                    }),
                )),
                Err(err) => Ok(ToolResult::failure(format!(
                    "agent_message: failed to reach parent: {err}"
                ))),
            }
        } else if let Some(child_id) = target.strip_prefix("child:") {
            let child_id = child_id.trim();
            if child_id.is_empty() {
                return Err(PeriError::Config(
                    "agent_message: `child:` target requires a session id (e.g. `child:teammate-42`)"
                        .to_string(),
                ));
            }
            match bus.send_to_child(&from, child_id, &message).await {
                Ok(()) => Ok(ToolResult::success(
                    format!("agent_message delivered to child {child_id}"),
                    serde_json::json!({
                        "delivered": true,
                        "target": "child",
                        "child_id": child_id,
                    }),
                )),
                Err(err) => Ok(ToolResult::failure(format!(
                    "agent_message: failed to reach child {child_id}: {err}"
                ))),
            }
        } else {
            Err(PeriError::Config(format!(
                "agent_message: unsupported target `{target}` (expected `parent` or `child:<id>`)"
            )))
        }
    }

    fn validate_params(&self, params: &Value) -> PeriResult<()> {
        let _ = required_str(params, "target")?;
        let _ = required_str(params, "message")?;
        Ok(())
    }

    fn permission_level(&self) -> PermissionLevel {
        PermissionLevel::Write
    }

    fn can_run_concurrent(&self) -> bool {
        true
    }
}
