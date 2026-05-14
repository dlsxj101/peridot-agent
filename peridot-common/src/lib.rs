//! Shared domain types for Peridot crates.

use std::collections::BTreeMap;
use std::fmt;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;

/// Result alias used by Peridot domain crates.
pub type PeriResult<T> = Result<T, PeriError>;

/// Cross-crate error type for deterministic domain failures.
#[derive(Debug, Error)]
pub enum PeriError {
    /// Configuration is missing, malformed, or unsupported.
    #[error("configuration error: {0}")]
    Config(String),
    /// A model provider could not complete the requested operation.
    #[error("provider error: {0}")]
    Provider(String),
    /// A tool failed before producing a successful observation.
    #[error("tool error: {0}")]
    Tool(String),
    /// A request was rejected by the permission or sandbox policy.
    #[error("permission denied: {0}")]
    PermissionDenied(String),
    /// A project path is outside the allowed boundary.
    #[error("path outside project boundary: {0}")]
    PathBoundary(PathBuf),
    /// A parser could not recover a structured value.
    #[error("parse error: {0}")]
    Parse(String),
    /// Verification failed at a named stage.
    #[error("verification failed at {stage}: {message}")]
    Verification {
        /// Verification stage name.
        stage: String,
        /// Human-readable failure message.
        message: String,
    },
}

/// High-level execution mode selected for the active session.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionMode {
    /// Read-only analysis and planning.
    Plan,
    /// Interactive implementation mode.
    #[default]
    Execute,
    /// Long-running autonomous mode with a durable objective.
    Goal,
}

impl fmt::Display for ExecutionMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let value = match self {
            Self::Plan => "plan",
            Self::Execute => "execute",
            Self::Goal => "goal",
        };
        f.write_str(value)
    }
}

/// Permission posture used when a tool may modify state.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PermissionMode {
    /// Confirm every write, shell, and git operation.
    Safe,
    /// Confirm risky, destructive, and system operations.
    #[default]
    Auto,
    /// Allow all operations except hard security blocks.
    Yolo,
}

impl fmt::Display for PermissionMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let value = match self {
            Self::Safe => "safe",
            Self::Auto => "auto",
            Self::Yolo => "yolo",
        };
        f.write_str(value)
    }
}

/// Core state-machine phase for the harness loop.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum AgentPhase {
    /// The agent is gathering context and building a plan.
    #[default]
    Planning,
    /// The agent is applying changes or running tools.
    Executing,
    /// The agent is verifying the current work.
    Verifying,
    /// The agent is recovering from an error or stuck loop.
    Recovering,
    /// The agent has delegated work to a subagent.
    Delegating,
    /// The task has reached a stopping condition.
    Done,
}

/// Logical group for tools exposed to the model.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolGroup {
    /// Shell command execution.
    Shell,
    /// Filesystem inspection and mutation.
    File,
    /// Git operations.
    Git,
    /// Web search and fetch operations.
    Web,
    /// Planning tools.
    Plan,
    /// Build, lint, test, and grader tools.
    Verify,
    /// Subagent and user-interaction tools.
    Agent,
    /// External MCP tools.
    Mcp,
}

/// Permission category declared by each tool.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PermissionLevel {
    /// Pure read-only operation.
    Read,
    /// Safe write scoped to the workspace.
    Write,
    /// Risky write that can delete, publish, or rewrite history.
    Destructive,
    /// System-level operation such as package installation or service control.
    System,
}

/// A model-requested tool call.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ToolCall {
    /// Tool name.
    pub name: String,
    /// Tool parameters encoded as JSON.
    pub parameters: Value,
}

impl ToolCall {
    /// Creates a new tool call.
    pub fn new(name: impl Into<String>, parameters: Value) -> Self {
        Self {
            name: name.into(),
            parameters,
        }
    }
}

/// A normalized tool execution result.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ToolResult {
    /// Whether the tool completed successfully.
    pub success: bool,
    /// Short human-readable summary for the model and UI.
    pub summary: String,
    /// Structured output for downstream tools.
    pub output: Value,
}

impl ToolResult {
    /// Builds a successful tool result.
    pub fn success(summary: impl Into<String>, output: Value) -> Self {
        Self {
            success: true,
            summary: summary.into(),
            output,
        }
    }

    /// Builds a failed tool result.
    pub fn failure(summary: impl Into<String>) -> Self {
        Self {
            success: false,
            summary: summary.into(),
            output: Value::Null,
        }
    }
}

/// Ask-user interaction shape.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AskUserRequest {
    /// Ask the user to pick one option.
    SingleSelect {
        /// Question to show.
        question: String,
        /// Available choices.
        options: Vec<String>,
        /// Default choice index.
        default_index: Option<usize>,
    },
    /// Ask the user to pick multiple options.
    MultiSelect {
        /// Question to show.
        question: String,
        /// Available choices.
        options: Vec<String>,
        /// Minimum selected count.
        min: usize,
        /// Maximum selected count.
        max: Option<usize>,
    },
    /// Ask the user for free-form text.
    FreeForm {
        /// Question to show.
        question: String,
        /// Optional hint.
        hint: Option<String>,
        /// Default answer.
        default: Option<String>,
    },
}

/// Top-level Peridot configuration.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct PeridotConfig {
    /// Authentication settings.
    #[serde(default)]
    pub auth: AuthConfig,
    /// Model routing settings.
    #[serde(default)]
    pub models: ModelsConfig,
    /// Default runtime settings.
    #[serde(default)]
    pub defaults: DefaultsConfig,
    /// API transport settings.
    #[serde(default)]
    pub api: ApiConfig,
    /// Context-window settings.
    #[serde(default)]
    pub context: ContextConfig,
    /// Security and sandbox settings.
    #[serde(default)]
    pub security: SecurityConfig,
    /// Git automation settings.
    #[serde(default)]
    pub git: GitConfig,
    /// MCP server definitions loaded at session start.
    #[serde(default)]
    pub mcp: Vec<McpServerConfig>,
    /// User hook definitions.
    #[serde(default)]
    pub hooks: HooksConfig,
}

/// Git automation configuration.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct GitConfig {
    /// Whether completed logical units should be committed automatically.
    #[serde(default)]
    pub auto_commit: bool,
    /// Preferred commit cadence.
    #[serde(default = "default_commit_frequency")]
    pub commit_frequency: String,
    /// Branch prefix for agent-created branches.
    #[serde(default = "default_branch_prefix")]
    pub branch_prefix: String,
    /// Whether Peridot may create a branch before committing.
    #[serde(default)]
    pub auto_branch: bool,
    /// Commit message style.
    #[serde(default = "default_commit_message_style")]
    pub commit_message_style: String,
}

impl Default for GitConfig {
    fn default() -> Self {
        Self {
            auto_commit: false,
            commit_frequency: default_commit_frequency(),
            branch_prefix: default_branch_prefix(),
            auto_branch: false,
            commit_message_style: default_commit_message_style(),
        }
    }
}

fn default_commit_frequency() -> String {
    "logical_unit".to_string()
}

fn default_branch_prefix() -> String {
    "peridot/".to_string()
}

fn default_commit_message_style() -> String {
    "conventional".to_string()
}

/// Command sandbox backend.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SandboxMode {
    /// Run commands directly with blocklist and path sandbox only.
    #[default]
    None,
    /// Run shell commands through Docker with the project mounted as /workspace.
    Docker,
    /// Placeholder for Linux firejail isolation.
    Firejail,
}

impl fmt::Display for SandboxMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let value = match self {
            Self::None => "none",
            Self::Docker => "docker",
            Self::Firejail => "firejail",
        };
        f.write_str(value)
    }
}

/// Security and sandbox configuration.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SecurityConfig {
    /// Shell command sandbox backend.
    #[serde(default)]
    pub sandbox: SandboxMode,
    /// Docker image used for command sandboxing.
    #[serde(default = "default_docker_image")]
    pub docker_image: String,
    /// Whether Docker sandboxed commands can access the network.
    #[serde(default)]
    pub docker_network: bool,
    /// Whether dependency installation commands require explicit approval.
    #[serde(default = "default_ask_before_install")]
    pub ask_before_install: bool,
    /// Whether destructive delete/history commands require explicit approval.
    #[serde(default = "default_ask_before_delete")]
    pub ask_before_delete: bool,
}

impl Default for SecurityConfig {
    fn default() -> Self {
        Self {
            sandbox: SandboxMode::None,
            docker_image: default_docker_image(),
            docker_network: false,
            ask_before_install: default_ask_before_install(),
            ask_before_delete: default_ask_before_delete(),
        }
    }
}

fn default_docker_image() -> String {
    "rust:1-bookworm".to_string()
}

fn default_ask_before_install() -> bool {
    true
}

fn default_ask_before_delete() -> bool {
    true
}

/// Authentication configuration.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct AuthConfig {
    /// Primary provider identifier.
    #[serde(default = "default_primary_auth")]
    pub primary: String,
}

impl Default for AuthConfig {
    fn default() -> Self {
        Self {
            primary: default_primary_auth(),
        }
    }
}

fn default_primary_auth() -> String {
    "claude-api".to_string()
}

/// Model routing configuration.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ModelsConfig {
    /// Main agent model.
    #[serde(default = "default_main_model")]
    pub main: String,
    /// Goal checker model.
    #[serde(default = "default_goal_checker_model")]
    pub goal_checker: String,
    /// Compaction model.
    #[serde(default = "default_goal_checker_model")]
    pub compaction: String,
}

impl Default for ModelsConfig {
    fn default() -> Self {
        Self {
            main: default_main_model(),
            goal_checker: default_goal_checker_model(),
            compaction: default_goal_checker_model(),
        }
    }
}

fn default_main_model() -> String {
    "claude-sonnet-4-6".to_string()
}

fn default_goal_checker_model() -> String {
    "claude-haiku-4-5".to_string()
}

/// Runtime default configuration.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DefaultsConfig {
    /// Default execution mode.
    #[serde(default)]
    pub mode: ExecutionMode,
    /// Default permission mode.
    #[serde(default)]
    pub permission: PermissionMode,
    /// Maximum autonomous turns.
    #[serde(default = "default_max_turns")]
    pub max_turns: u32,
    /// Budget cap in USD.
    #[serde(default = "default_budget_usd")]
    pub budget_usd: f64,
    /// Budget warning threshold percentage.
    #[serde(default = "default_budget_warning_pct")]
    pub budget_warning_pct: u8,
}

impl Default for DefaultsConfig {
    fn default() -> Self {
        Self {
            mode: ExecutionMode::default(),
            permission: PermissionMode::default(),
            max_turns: default_max_turns(),
            budget_usd: default_budget_usd(),
            budget_warning_pct: default_budget_warning_pct(),
        }
    }
}

fn default_max_turns() -> u32 {
    100
}

fn default_budget_usd() -> f64 {
    5.0
}

fn default_budget_warning_pct() -> u8 {
    50
}

/// API transport configuration.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ApiConfig {
    /// Provider base URL.
    #[serde(default = "default_api_base_url")]
    pub base_url: String,
    /// Request timeout in seconds.
    #[serde(default = "default_timeout_seconds")]
    pub timeout_seconds: u64,
    /// Maximum retry count.
    #[serde(default = "default_max_retries")]
    pub max_retries: u8,
    /// Prompt cache TTL.
    #[serde(default = "default_cache_ttl")]
    pub cache_ttl: String,
}

impl Default for ApiConfig {
    fn default() -> Self {
        Self {
            base_url: default_api_base_url(),
            timeout_seconds: default_timeout_seconds(),
            max_retries: default_max_retries(),
            cache_ttl: default_cache_ttl(),
        }
    }
}

fn default_api_base_url() -> String {
    "https://api.anthropic.com".to_string()
}

fn default_timeout_seconds() -> u64 {
    120
}

fn default_max_retries() -> u8 {
    3
}

fn default_cache_ttl() -> String {
    "5m".to_string()
}

/// Context-window configuration.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ContextConfig {
    /// Soft token budget.
    #[serde(default = "default_budget_tokens")]
    pub budget_tokens: usize,
    /// Compaction threshold.
    #[serde(default = "default_compaction_threshold")]
    pub compaction_threshold: usize,
    /// Hard token limit.
    #[serde(default = "default_hard_limit")]
    pub hard_limit: usize,
    /// Character threshold for offloading large observations.
    #[serde(default = "default_offload_threshold_chars")]
    pub offload_threshold_chars: usize,
    /// Maximum observation characters injected inline.
    #[serde(default = "default_observation_max_chars")]
    pub observation_max_chars: usize,
    /// Thinking policy.
    #[serde(default = "default_thinking")]
    pub thinking: String,
}

impl Default for ContextConfig {
    fn default() -> Self {
        Self {
            budget_tokens: default_budget_tokens(),
            compaction_threshold: default_compaction_threshold(),
            hard_limit: default_hard_limit(),
            offload_threshold_chars: default_offload_threshold_chars(),
            observation_max_chars: default_observation_max_chars(),
            thinking: default_thinking(),
        }
    }
}

fn default_budget_tokens() -> usize {
    180_000
}

fn default_compaction_threshold() -> usize {
    100_000
}

fn default_hard_limit() -> usize {
    160_000
}

fn default_offload_threshold_chars() -> usize {
    3_000
}

fn default_observation_max_chars() -> usize {
    8_000
}

fn default_thinking() -> String {
    "auto".to_string()
}

/// MCP transport type.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum McpTransport {
    /// Standard input/output transport.
    Stdio,
    /// HTTP/SSE transport.
    Http,
}

impl fmt::Display for McpTransport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let value = match self {
            Self::Stdio => "stdio",
            Self::Http => "http",
        };
        f.write_str(value)
    }
}

/// MCP server configuration.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct McpServerConfig {
    /// Server name.
    pub name: String,
    /// Transport kind.
    pub transport: McpTransport,
    /// Stdio command.
    #[serde(default)]
    pub command: Option<String>,
    /// Stdio command arguments.
    #[serde(default)]
    pub args: Vec<String>,
    /// Stdio environment variables.
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    /// HTTP/SSE endpoint.
    #[serde(default)]
    pub url: Option<String>,
    /// HTTP auth declaration, for example bearer:${TOKEN}.
    #[serde(default)]
    pub auth: Option<String>,
}

/// Hook configuration grouped by hook class.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct HooksConfig {
    /// Tool pre/post hooks.
    #[serde(default)]
    pub tool: Vec<HookConfig>,
    /// System event hooks.
    #[serde(default)]
    pub event: Vec<HookConfig>,
    /// Session lifecycle hooks.
    #[serde(default)]
    pub lifecycle: Vec<HookConfig>,
    /// Default hook timeout in seconds.
    #[serde(default = "default_hook_timeout_seconds")]
    pub timeout_seconds: u64,
}

impl Default for HooksConfig {
    fn default() -> Self {
        Self {
            tool: Vec::new(),
            event: Vec::new(),
            lifecycle: Vec::new(),
            timeout_seconds: default_hook_timeout_seconds(),
        }
    }
}

fn default_hook_timeout_seconds() -> u64 {
    30
}

/// One configured user hook.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct HookConfig {
    /// Hook event name, such as pre:file_write or verification_failed.
    pub event: String,
    /// Command template to execute.
    pub run: String,
    /// Optional human-readable description.
    #[serde(default)]
    pub description: Option<String>,
    /// Failure behavior.
    #[serde(default)]
    pub on_failure: HookFailureMode,
    /// Optional path filters.
    #[serde(default)]
    pub only_paths: Vec<String>,
}

/// Hook failure behavior.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HookFailureMode {
    /// Ignore the failure after recording it.
    Ignore,
    /// Warn and continue.
    #[default]
    Warn,
    /// Block the enclosing operation.
    Block,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_mcp_config() {
        let config = toml::from_str::<PeridotConfig>(
            r#"
            [[mcp]]
            name = "jira"
            transport = "stdio"
            command = "npx"
            args = ["-y", "jira-mcp"]
            "#,
        )
        .unwrap();

        assert_eq!(config.mcp.len(), 1);
        assert_eq!(config.mcp[0].name, "jira");
        assert_eq!(config.mcp[0].transport, McpTransport::Stdio);
        assert_eq!(config.mcp[0].command.as_deref(), Some("npx"));
    }

    #[test]
    fn parses_hook_config() {
        let config = toml::from_str::<PeridotConfig>(
            r#"
            [hooks]
            timeout_seconds = 5

            [[hooks.tool]]
            event = "pre:file_write"
            run = ".peridot/hooks/backup.sh {path}"
            on_failure = "block"
            only_paths = ["src/**"]
            "#,
        )
        .unwrap();

        assert_eq!(config.hooks.timeout_seconds, 5);
        assert_eq!(config.hooks.tool.len(), 1);
        assert_eq!(config.hooks.tool[0].on_failure, HookFailureMode::Block);
        assert_eq!(config.hooks.tool[0].only_paths, vec!["src/**"]);
    }

    #[test]
    fn parses_security_config() {
        let config = toml::from_str::<PeridotConfig>(
            r#"
            [security]
            sandbox = "docker"
            docker_image = "rust:1.95"
            docker_network = true
            ask_before_install = false
            ask_before_delete = false
            "#,
        )
        .unwrap();

        assert_eq!(config.security.sandbox, SandboxMode::Docker);
        assert_eq!(config.security.docker_image, "rust:1.95");
        assert!(config.security.docker_network);
        assert!(!config.security.ask_before_install);
        assert!(!config.security.ask_before_delete);
    }

    #[test]
    fn parses_git_config() {
        let config = toml::from_str::<PeridotConfig>(
            r#"
            [git]
            auto_commit = true
            commit_frequency = "logical_unit"
            branch_prefix = "agent/"
            auto_branch = true
            commit_message_style = "conventional"
            "#,
        )
        .unwrap();

        assert!(config.git.auto_commit);
        assert_eq!(config.git.commit_frequency, "logical_unit");
        assert_eq!(config.git.branch_prefix, "agent/");
        assert!(config.git.auto_branch);
        assert_eq!(config.git.commit_message_style, "conventional");
    }
}
