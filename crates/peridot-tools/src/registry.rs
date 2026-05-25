use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use peridot_agents::SubAgent;
use peridot_common::{
    AskUserAnswer, AskUserRequest, CancelToken, HooksConfig, PeriError, PeriResult,
    PermissionLevel, PermissionMode, SecurityConfig, ToolGroup, ToolResult,
};
use serde_json::Value;

/// Bridge that lets `agent_ask_user` wait for an actual user response
/// from an interactive front-end (TUI, REPL, etc). When no port is
/// attached the tool synthesises a default answer so headless paths,
/// tests, and mock providers keep working without UI plumbing.
#[async_trait]
pub trait AskUserPort: Send + Sync {
    /// Presents the request to the user and resolves to the chosen
    /// answer. Implementations may block until the user responds, time
    /// out and return `AskUserAnswer::Cancelled`, or apply any
    /// front-end-specific policy.
    async fn ask(&self, request: AskUserRequest) -> AskUserAnswer;
}

/// Bridge that routes `agent_message` between parent and child subagent
/// sessions. When no bus is attached (single-session run, tests, headless
/// jobs without spawn capability), `AgentMessageTool` returns a polite
/// noop so the model still sees a tool result instead of an error.
///
/// Implementations live in the harness layer (`peridot-cli` /
/// `SessionRouter`) because that's the first place that owns the per-session
/// lifetime + parent_id map.
#[async_trait]
pub trait AgentMessageBus: Send + Sync {
    /// Returns the calling session's id when the harness has wired
    /// session identity into the bus, otherwise `None`. The tool uses
    /// this to populate the "from" field on every outbound message
    /// without requiring the model to know its own id.
    fn current_session_id(&self) -> Option<String> {
        None
    }

    /// Forwards a message from the current session to its parent. Returns
    /// the resolved parent id (or an `Err` when the current session has
    /// no parent, e.g. the root foreground session).
    async fn send_to_parent(&self, from_session: &str, message: &str) -> PeriResult<String>;

    /// Forwards a message from the current session to a named child.
    /// `child_session` must be one of the children the router has
    /// registered against `from_session.parent_id`; otherwise the
    /// implementation should return an `Err` so a typo doesn't silently
    /// drop the note.
    async fn send_to_child(
        &self,
        from_session: &str,
        child_session: &str,
        message: &str,
    ) -> PeriResult<()>;

    /// Drains the inbox for `session`, returning the queued messages in
    /// FIFO order. The harness loop calls this at the start of every turn
    /// and folds the entries into context as `PlanReminder`s. An empty
    /// vec means "no new messages, carry on".
    async fn drain_inbox(&self, session: &str) -> Vec<InboxMessage>;
}

/// One queued message destined for an agent session's inbox.
#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct InboxMessage {
    /// Session id of the sender. The receiver renders this as
    /// `[from: <id>]` in the context entry so the model knows who is
    /// talking to it.
    pub from: String,
    /// Message body.
    pub body: String,
    /// Unix seconds when the router accepted the message.
    pub at_unix: u64,
}

/// Runtime context passed to tool implementations.
#[derive(Clone)]
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
    /// Optional subagent runner injected by the harness. When set,
    /// `AgentDelegateTool` runs the prompt through this runner (typically
    /// a bounded inner HarnessAgent loop) instead of only preparing a
    /// workspace via `LocalSubAgentRunner`. Tests and minimal harnesses
    /// leave it `None`, which keeps the legacy prepare-only behaviour.
    pub runner: Option<Arc<dyn SubAgent>>,
    /// Optional ask-user port injected by interactive front-ends. When
    /// set, `AgentAskUserTool` awaits a real user answer through this
    /// port; otherwise it falls back to a synthesised default so
    /// headless / mock / test paths keep running unchanged.
    pub ask_user_port: Option<Arc<dyn AskUserPort>>,
    /// Optional bus that delivers `agent_message` calls to the right
    /// parent or child session. None on single-session runs or in tests
    /// — the tool returns a polite noop in that case.
    pub message_bus: Option<Arc<dyn AgentMessageBus>>,
    /// Bounded context packet prepared by the parent harness for delegated
    /// subagents. It carries user intent, recent decisions, and evidence refs
    /// without replaying the full transcript.
    pub parent_context_packet: Option<String>,
}

impl std::fmt::Debug for ToolContext {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ToolContext")
            .field("project_root", &self.project_root)
            .field("permission_mode", &self.permission_mode)
            .field("denied_paths", &self.denied_paths)
            .field("hooks", &self.hooks)
            .field("security", &self.security)
            .field("cancel", &self.cancel)
            .field("runner", &self.runner.as_ref().map(|_| "Arc<dyn SubAgent>"))
            .field(
                "ask_user_port",
                &self.ask_user_port.as_ref().map(|_| "Arc<dyn AskUserPort>"),
            )
            .field(
                "message_bus",
                &self
                    .message_bus
                    .as_ref()
                    .map(|_| "Arc<dyn AgentMessageBus>"),
            )
            .field(
                "parent_context_packet",
                &self.parent_context_packet.as_ref().map(|_| "String"),
            )
            .finish()
    }
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
            runner: None,
            ask_user_port: None,
            message_bus: None,
            parent_context_packet: None,
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

    /// Attaches a subagent runner. When present, `AgentDelegateTool`
    /// dispatches through this runner instead of only preparing a
    /// workspace. The harness wires in `InnerLoopSubAgent` here so
    /// `agent_delegate` actually executes the delegated task; tests
    /// leave it `None` to keep the deterministic prepare-only path.
    pub fn with_subagent_runner(mut self, runner: Arc<dyn SubAgent>) -> Self {
        self.runner = Some(runner);
        self
    }

    /// Attaches an ask-user port. When present, `AgentAskUserTool`
    /// dispatches the question through this port and awaits the real
    /// user answer; otherwise the tool falls back to its synthesised
    /// default so headless / mock / test paths keep running.
    pub fn with_ask_user_port(mut self, port: Arc<dyn AskUserPort>) -> Self {
        self.ask_user_port = Some(port);
        self
    }

    /// Attaches an agent message bus. When present, `AgentMessageTool`
    /// routes through this bus to deliver notes to parent or child
    /// sessions; otherwise the tool returns a noop result with a hint.
    pub fn with_message_bus(mut self, bus: Arc<dyn AgentMessageBus>) -> Self {
        self.message_bus = Some(bus);
        self
    }

    /// Attaches a bounded parent context packet for subagent delegation.
    pub fn with_parent_context_packet(mut self, packet: impl Into<String>) -> Self {
        self.parent_context_packet = Some(packet.into());
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

    /// Finer-grained risk classification surfaced to the UI and used
    /// by class-based approval policies. Defaults to a derivation from
    /// [`PermissionLevel`] so existing tools keep working without
    /// explicit overrides; security-sensitive tools (shell, web_fetch,
    /// env-touching utilities) should override this with a specific
    /// [`RiskClass`].
    fn risk_class(&self) -> peridot_common::RiskClass {
        use peridot_common::RiskClass;
        match self.permission_level() {
            PermissionLevel::Read => RiskClass::ReadOnly,
            PermissionLevel::Write => RiskClass::LocalWrite,
            PermissionLevel::Destructive => RiskClass::Destructive,
            PermissionLevel::System => RiskClass::SecretAdjacent,
        }
    }

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
