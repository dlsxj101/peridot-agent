use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use peridot_common::{
    CancelToken, HooksConfig, PeriError, PeriResult, PermissionLevel, PermissionMode,
    SecurityConfig, ToolGroup, ToolResult,
};
use serde_json::Value;

/// Runtime context passed to tool implementations.
#[derive(Clone, Debug)]
pub struct ToolContext {
    /// Project root used for sandbox and hook execution.
    pub project_root: PathBuf,
    /// Active permission mode.
    pub permission_mode: PermissionMode,
    /// Project-local path prefixes that must not be modified.
    pub denied_paths: Vec<PathBuf>,
    /// User hook definitions active for this tool call.
    pub hooks: HooksConfig,
    /// Security and sandbox settings active for this tool call.
    pub security: SecurityConfig,
    /// Cancellation handle propagated from the agent loop. Long-running
    /// tools (notably `shell_exec`) poll this between sub-steps so the
    /// operator's Esc interrupt aborts the in-flight command instead of
    /// only landing between turns.
    pub cancel: Option<CancelToken>,
}

impl ToolContext {
    /// Creates a tool context.
    pub fn new(project_root: impl Into<PathBuf>, permission_mode: PermissionMode) -> Self {
        Self {
            project_root: project_root.into(),
            permission_mode,
            denied_paths: Vec::new(),
            hooks: HooksConfig::default(),
            security: SecurityConfig::default(),
            cancel: None,
        }
    }

    /// Adds denied path prefixes to the context.
    pub fn with_denied_paths(mut self, denied_paths: impl IntoIterator<Item = PathBuf>) -> Self {
        self.denied_paths = denied_paths.into_iter().collect();
        self
    }

    /// Adds hook definitions to the context.
    pub fn with_hooks(mut self, hooks: HooksConfig) -> Self {
        self.hooks = hooks;
        self
    }

    /// Adds security configuration to the context.
    pub fn with_security(mut self, security: SecurityConfig) -> Self {
        self.security = security;
        self
    }

    /// Attaches the agent loop's cancel token. Used by `shell_exec` (and
    /// any future long-running tool) to abort mid-flight when the user
    /// hits Esc.
    pub fn with_cancel(mut self, cancel: CancelToken) -> Self {
        self.cancel = Some(cancel);
        self
    }
}

/// Tool implementation contract.
#[async_trait]
pub trait Tool: Send + Sync {
    /// Stable tool name exposed to the model.
    fn name(&self) -> &str;

    /// Logical tool group.
    fn group(&self) -> ToolGroup;

    /// Human-readable tool description.
    fn description(&self) -> &str;

    /// JSON Schema describing the tool's parameter object. Defaults to a permissive
    /// object schema so existing tools keep working; concrete tools should override
    /// this with a properly typed schema so the model can call them correctly.
    fn parameters_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "additionalProperties": true,
        })
    }

    /// Executes the tool with JSON parameters.
    async fn execute(&self, params: Value, ctx: &ToolContext) -> PeriResult<ToolResult>;

    /// Validates JSON parameters before execution.
    fn validate_params(&self, _params: &Value) -> PeriResult<()> {
        Ok(())
    }

    /// Permission category declared by the tool.
    fn permission_level(&self) -> PermissionLevel;

    /// Whether this tool is read-only.
    fn is_read_only(&self) -> bool {
        self.permission_level() == PermissionLevel::Read
    }

    /// Whether this tool can safely run concurrently with other tools.
    fn can_run_concurrent(&self) -> bool {
        self.is_read_only()
    }

    /// Whether this tool modifies workspace or session state.
    fn modifies_state(&self) -> bool {
        !self.is_read_only()
    }

    /// Whether this tool needs user confirmation in the provided permission mode.
    fn requires_confirmation(&self, mode: PermissionMode) -> bool {
        match mode {
            PermissionMode::Safe => self.permission_level() != PermissionLevel::Read,
            PermissionMode::Auto => matches!(
                self.permission_level(),
                PermissionLevel::Destructive | PermissionLevel::System
            ),
            PermissionMode::Yolo => false,
        }
    }
}

/// Provider-neutral descriptor used to surface registered tools to LLM providers.
#[derive(Clone, Debug)]
pub struct ToolDescriptor {
    /// Tool name reported to the provider.
    pub name: String,
    /// Tool description shown to the model.
    pub description: String,
    /// JSON Schema for the tool's parameter object.
    pub parameters: Value,
}

/// Deterministically ordered tool registry.
#[derive(Clone, Default)]
pub struct ToolRegistry {
    tools: BTreeMap<String, Arc<dyn Tool>>,
}

impl ToolRegistry {
    /// Creates an empty registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers a tool by its stable name.
    pub fn register<T>(&mut self, tool: T) -> PeriResult<()>
    where
        T: Tool + 'static,
    {
        let name = tool.name().to_string();
        if self.tools.contains_key(&name) {
            return Err(PeriError::Tool(format!("tool already registered: {name}")));
        }
        self.tools.insert(name, Arc::new(tool));
        Ok(())
    }

    /// Returns a tool by name.
    pub fn get(&self, name: &str) -> Option<Arc<dyn Tool>> {
        self.tools.get(name).cloned()
    }

    /// Returns registered tool names in deterministic order.
    pub fn names(&self) -> Vec<&str> {
        self.tools.keys().map(String::as_str).collect()
    }

    /// Returns the registered tools' name, description, and parameter schema in
    /// deterministic order. Each entry is shaped to drop into the provider-neutral
    /// `ToolDefinition` consumed by [`peridot_llm::CompletionRequest::tools`].
    pub fn descriptors(&self) -> Vec<ToolDescriptor> {
        self.tools
            .values()
            .map(|tool| ToolDescriptor {
                name: tool.name().to_string(),
                description: tool.description().to_string(),
                parameters: tool.parameters_schema(),
            })
            .collect()
    }

    /// Returns the number of registered tools.
    pub fn len(&self) -> usize {
        self.tools.len()
    }

    /// Returns true when no tools are registered.
    pub fn is_empty(&self) -> bool {
        self.tools.is_empty()
    }
}
