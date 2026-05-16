use std::fs;
use std::path::{Path, PathBuf};

use async_trait::async_trait;
use peridot_agents::{
    LocalSubAgentRunner, ModelTier, SubAgent, SubAgentKind, SubAgentPolicy, SubAgentTask,
};
use peridot_common::{PeriError, PeriResult, PermissionLevel, ToolGroup, ToolResult};
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
        let answer = default_ask_user_answer(&params, &choices, default_index);
        let display_choices = ask_user_display_choices(&kind, &choices);
        let explanation = params
            .get("explanation")
            .and_then(Value::as_str)
            .unwrap_or("Peridot needs this answer to continue without guessing.")
            .to_string();
        run_ask_user_triggered_hook(ctx, question, &kind)?;
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
        let runner = LocalSubAgentRunner::new(
            &ctx.project_root,
            ctx.project_root.join(".peridot/worktrees"),
        );
        let result = match runner
            .run(SubAgentTask {
                prompt: prompt.clone(),
                kind: kind.clone(),
                model_tier: Some(model_tier),
            })
            .await
        {
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
        let skills = layers
            .iter()
            .flat_map(|layer| layer.skills.clone())
            .collect::<Vec<_>>();
        let error_resolutions = layers
            .iter()
            .filter_map(|layer| layer.error_resolution.clone())
            .collect::<Vec<_>>();
        Ok(ToolResult::success(
            format!(
                "memory search returned {} skills and {} error resolutions across {} layers",
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
    if let Some(home) = std::env::var_os("PERIDOT_HOME") {
        return Some(PathBuf::from(home).join("memory.db"));
    }
    std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".peridot/memory.db"))
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
