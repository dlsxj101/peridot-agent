use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use peridot_common::{
    HooksConfig, PeriError, PeriResult, PermissionLevel, PermissionMode, SecurityConfig, ToolGroup,
    ToolResult,
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

    /// Returns the number of registered tools.
    pub fn len(&self) -> usize {
        self.tools.len()
    }

    /// Returns true when no tools are registered.
    pub fn is_empty(&self) -> bool {
        self.tools.is_empty()
    }
}
