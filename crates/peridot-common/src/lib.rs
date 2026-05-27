//! Shared domain types for Peridot crates.

mod cancel;

pub use cancel::CancelToken;

use std::collections::BTreeMap;
use std::fmt;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;

/// Result alias used by Peridot domain crates.
pub type PeriResult<T> = Result<T, PeriError>;

/// Returns the user's home directory using cross-platform environment
/// conventions.
///
/// Unix-like shells usually provide `HOME`; Windows-native extension hosts
/// commonly provide `USERPROFILE` instead. The `HOMEDRIVE` + `HOMEPATH`
/// fallback covers older Windows process environments.
pub fn user_home_dir() -> Option<PathBuf> {
    if let Some(home) = std::env::var_os("HOME").filter(|value| !value.is_empty()) {
        return Some(PathBuf::from(home));
    }
    if let Some(profile) = std::env::var_os("USERPROFILE").filter(|value| !value.is_empty()) {
        return Some(PathBuf::from(profile));
    }
    let drive = std::env::var_os("HOMEDRIVE").filter(|value| !value.is_empty());
    let path = std::env::var_os("HOMEPATH").filter(|value| !value.is_empty());
    match (drive, path) {
        (Some(drive), Some(path)) => {
            let mut home = PathBuf::from(drive);
            home.push(path);
            Some(home)
        }
        _ => None,
    }
}

/// Returns Peridot's global state directory.
///
/// `PERIDOT_HOME` remains the explicit override. When it is unset, the
/// directory is `$HOME/.peridot` on Unix-like hosts and
/// `%USERPROFILE%\.peridot` on Windows hosts.
pub fn peridot_home_dir() -> Option<PathBuf> {
    if let Some(home) = std::env::var_os("PERIDOT_HOME").filter(|value| !value.is_empty()) {
        return Some(PathBuf::from(home));
    }
    user_home_dir().map(|home| home.join(".peridot"))
}

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
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Hash, Serialize, Deserialize)]
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

/// Finer-grained risk classification used to surface "why does this need
/// approval?" reasoning in the UI and to drive class-based approval
/// policies in [`SecurityConfig`].
///
/// [`PermissionLevel`] (above) is coarse and was designed for tool-allowlist
/// checks. `RiskClass` is the orthogonal axis: what kind of *harm* the
/// tool could cause if mis-invoked. The UI uses this to colour-code tool
/// chips; the daemon uses it to decide whether a given class auto-approves
/// in the current security mode.
///
/// Ordering is meaningful: higher discriminants are riskier, and policies
/// can compare with `>=` to opt every class above a threshold into a
/// stricter rule (e.g., "always prompt for >= Destructive").
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RiskClass {
    /// Read-only file or repository access. Cannot mutate state.
    ReadOnly,
    /// Writes scoped to the workspace (file_patch, file_write).
    LocalWrite,
    /// Build / test / lint commands. Read code, run compilers, no
    /// network. Side effects limited to caches and build artifacts.
    BuildOrTest,
    /// External network access (web_fetch, MCP HTTP servers, package
    /// install). Risk: data exfiltration, supply chain.
    ExternalNetwork,
    /// Workspace operations that can permanently destroy local state
    /// (rm -rf, git reset --hard, git push --force).
    Destructive,
    /// Touches secrets, environment, or auth surfaces (env vars,
    /// credential files, config edits). Risk: credential leak.
    SecretAdjacent,
}

impl RiskClass {
    /// Stable short string used in event payloads / UI labels.
    pub const fn label(self) -> &'static str {
        match self {
            Self::ReadOnly => "read_only",
            Self::LocalWrite => "local_write",
            Self::BuildOrTest => "build_or_test",
            Self::ExternalNetwork => "external_network",
            Self::Destructive => "destructive",
            Self::SecretAdjacent => "secret_adjacent",
        }
    }
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

/// Answer returned by an `AskUserPort` after the user resolves the
/// request — or after the harness synthesises a fallback in
/// non-interactive contexts.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AskUserAnswer {
    /// User picked a single option from a `SingleSelect` request.
    Selected {
        /// Index into the original options list.
        index: usize,
        /// Text of the selected option.
        text: String,
    },
    /// User picked multiple options from a `MultiSelect` request.
    MultiSelected {
        /// Indices into the original options list (sorted ascending).
        indices: Vec<usize>,
    },
    /// User typed free-form text for a `FreeForm` request.
    Text(String),
    /// The prompt was cancelled or timed out without a real selection.
    /// Callers should treat the request's default as the resolved value
    /// when one is available.
    Cancelled,
}

impl AskUserAnswer {
    /// Returns the answer rendered as a plain string for inclusion in
    /// tool results and conversation history. `MultiSelected` joins the
    /// indices with commas; `Cancelled` becomes the empty string.
    pub fn to_display_string(&self, request: &AskUserRequest) -> String {
        match self {
            AskUserAnswer::Selected { text, .. } => text.clone(),
            AskUserAnswer::MultiSelected { indices } => {
                let options = match request {
                    AskUserRequest::MultiSelect { options, .. } => options.as_slice(),
                    _ => &[][..],
                };
                indices
                    .iter()
                    .filter_map(|index| options.get(*index).cloned())
                    .collect::<Vec<_>>()
                    .join(", ")
            }
            AskUserAnswer::Text(text) => text.clone(),
            AskUserAnswer::Cancelled => String::new(),
        }
    }
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
    /// Learned memory and self-improvement settings.
    #[serde(default)]
    pub memory: MemoryConfig,
    /// Terminal UI settings.
    #[serde(default)]
    pub tui: TuiConfig,
    /// Security and sandbox settings.
    #[serde(default)]
    pub security: SecurityConfig,
    /// Git automation settings.
    #[serde(default)]
    pub git: GitConfig,
    /// Self-update notification and installation settings.
    #[serde(default)]
    pub updates: UpdatesConfig,
    /// MCP server definitions loaded at session start.
    #[serde(default)]
    pub mcp: Vec<McpServerConfig>,
    /// User hook definitions.
    #[serde(default)]
    pub hooks: HooksConfig,
    /// Multi-LLM committee settings (Planner / Reviewer / Executor roles).
    #[serde(default)]
    pub committee: CommitteeConfig,
    /// Sub-agent (fork / worktree / teammate) spawn defaults.
    #[serde(default)]
    pub subagents: SubAgentsConfig,
    /// Auto-fix loop settings (verify-after-mutation behaviour).
    #[serde(default)]
    pub auto_fix: AutoFixConfig,
    /// Cross-surface UI preferences (currently just locale). Decoupled
    /// from `[tui]` so the value can drive both the terminal UI and the
    /// VS Code extension without implying TUI semantics. `language` is
    /// `Option` because `None` means "fall back to `tui.language`",
    /// which preserves backwards compatibility with configs written
    /// before this section existed.
    #[serde(default)]
    pub ui: UiConfig,
}

impl PeridotConfig {
    /// Resolve the user's chosen interface locale, preferring the new
    /// `[ui].language` knob and falling back to the legacy
    /// `[tui].language` value. Centralised so every surface (TUI
    /// settings screen, VS Code webview, future API) reads the same
    /// effective value without each having to re-implement the
    /// migration logic.
    pub fn effective_language(&self) -> Locale {
        self.ui.language.unwrap_or(self.tui.language)
    }
}

/// Returns model names worth suggesting in interactive `/model` pickers.
///
/// The list is intentionally config-derived instead of provider-catalog
/// derived: Peridot supports several provider backends and custom compatible
/// models, so the safest cross-surface completion source is the operator's
/// own configured main/subagent/committee role model set plus the currently
/// active runtime model when provided.
pub fn configured_model_suggestions(
    config: &PeridotConfig,
    active_model: Option<&str>,
) -> Vec<String> {
    let mut models = vec![config.models.main.clone()];
    if let Some(model) = active_model {
        models.push(model.to_string());
    }
    if let Some(model) = config.subagents.default_model.as_deref() {
        models.push(model.to_string());
    }
    models.extend(
        [
            config.committee.planner_model.as_str(),
            config.committee.reviewer_model.as_str(),
            config.committee.executor_model.as_str(),
        ]
        .into_iter()
        .filter(|model| !model.trim().is_empty())
        .map(str::to_string),
    );
    dedupe_sorted_nonempty(models)
}

fn dedupe_sorted_nonempty(values: Vec<String>) -> Vec<String> {
    let mut values: Vec<String> = values
        .into_iter()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .collect();
    values.sort();
    values.dedup_by(|left, right| left.eq_ignore_ascii_case(right));
    values
}

/// Defaults applied when the main agent spawns a sub-agent via
/// `agent_delegate` / `/fork` / `/teammate` / `/worktree`. When
/// `default_model` is `None` the spawn reuses the caller's main model name
/// so sub-agents stay on the user's chosen tier by default; setting it lets
/// the operator route every sub-agent to a cheaper / faster / stronger
/// model regardless of the main loop's selection. Overrideable at runtime
/// through the `/subagent model <name|reset>` slash command, which writes
/// to the running TUI state — config.toml carries the persistent default.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct SubAgentsConfig {
    /// Model name passed to every sub-agent spawn. `None` (the default)
    /// means "inherit from the caller" — sub-agents run on the same model
    /// as the main agent unless an explicit value is set here.
    #[serde(default)]
    pub default_model: Option<String>,
}

/// Auto-fix loop configuration. Controls the verify-after-mutation
/// behaviour in the agent harness.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AutoFixConfig {
    /// Maximum identical-failure attempts before the circuit breaker fires.
    #[serde(default = "default_auto_fix_max_attempts")]
    pub max_attempts: u32,
    /// Verification commands to run after each mutation, in order. An empty
    /// list means "use the built-in `verify_build` tool only" (the default).
    #[serde(default)]
    pub commands: Vec<String>,
    /// Whether the auto-fix loop is enabled when a session starts.
    /// On by default so a failed `verify_*` doesn't immediately stop
    /// the agent — instead, the policy injects the failure as a
    /// recovery reminder and the loop retries up to `max_attempts`
    /// times. Operators can disable it explicitly to enforce hard
    /// failures (CI-style runs, etc).
    ///
    /// Pairs with the serde-default helper so partial TOMLs that
    /// don't mention this knob still get the autonomous behaviour.
    #[serde(default = "default_true")]
    pub enabled: bool,
}

fn default_auto_fix_max_attempts() -> u32 {
    3
}

impl Default for AutoFixConfig {
    fn default() -> Self {
        Self {
            max_attempts: default_auto_fix_max_attempts(),
            commands: Vec::new(),
            enabled: true,
        }
    }
}

/// Provider-neutral reasoning intensity dial. Maps to: Anthropic
/// `thinking: { type: enabled, budget_tokens }` with budget scaled by tier
/// (Low ≈ 1k, Medium ≈ 4k, High ≈ 16k, XHigh ≈ 32k tokens); OpenAI
/// `reasoning: { effort: "low|medium|high|xhigh" }` (gpt-5, o-series).
/// Models without a reasoning channel simply ignore the field.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReasoningEffort {
    /// No reasoning channel; cheap chat-style behaviour.
    #[default]
    Off,
    /// Light reasoning — small token budget, fast.
    Low,
    /// Default reasoning depth when the user opts in without specifying.
    Medium,
    /// Maximum reasoning budget; expensive but most thorough.
    High,
    /// Extra-high reasoning budget for models that expose a deeper tier.
    XHigh,
}

impl ReasoningEffort {
    /// Parses a case-insensitive string (`off|low|medium|high|xhigh`) used by the
    /// `/reasoning` slash command and toml deserialisation when the user
    /// types a value by hand. Returns `None` for unrecognised input so
    /// callers can surface a helpful error.
    pub fn parse(input: &str) -> Option<Self> {
        match input.trim().to_ascii_lowercase().as_str() {
            "off" | "none" | "false" | "0" => Some(Self::Off),
            "low" | "min" | "minimal" => Some(Self::Low),
            "medium" | "med" | "default" | "true" => Some(Self::Medium),
            "high" | "max" | "maximum" => Some(Self::High),
            "xhigh" | "x-high" | "extra-high" | "extra_high" => Some(Self::XHigh),
            _ => None,
        }
    }

    /// Approximate Anthropic `budget_tokens` value for the tier. Empirically
    /// chosen to give a clear cost/quality separation while staying well
    /// inside published limits.
    pub fn anthropic_budget_tokens(self) -> Option<u32> {
        match self {
            Self::Off => None,
            Self::Low => Some(1_024),
            Self::Medium => Some(4_096),
            Self::High => Some(16_384),
            Self::XHigh => Some(32_768),
        }
    }

    /// String value passed to OpenAI's `reasoning.effort` field. `None`
    /// when reasoning is disabled (the field is omitted from the request).
    pub fn openai_effort_label(self) -> Option<&'static str> {
        match self {
            Self::Off => None,
            Self::Low => Some("low"),
            Self::Medium => Some("medium"),
            Self::High => Some("high"),
            Self::XHigh => Some("xhigh"),
        }
    }
}

impl std::fmt::Display for ReasoningEffort {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Off => "off",
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
            Self::XHigh => "xhigh",
        })
    }
}

/// Activation mode for the multi-role committee. `Off` keeps the legacy
/// single-agent loop; `Planner` runs a one-shot planner pre-flight before the
/// executor loop; `Full` adds an in-loop reviewer after every mutating turn.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CommitteeMode {
    /// Legacy single-agent behaviour (default).
    #[default]
    Off,
    /// Planner pre-flight + single executor loop (no per-turn reviewer).
    Planner,
    /// Planner pre-flight + executor loop with reviewer-after-each-turn.
    Full,
}

impl std::fmt::Display for CommitteeMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let label = match self {
            CommitteeMode::Off => "off",
            CommitteeMode::Planner => "planner",
            CommitteeMode::Full => "full",
        };
        write!(f, "{label}")
    }
}

impl std::str::FromStr for CommitteeMode {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value.trim().to_ascii_lowercase().as_str() {
            "off" => Ok(CommitteeMode::Off),
            "planner" => Ok(CommitteeMode::Planner),
            "full" => Ok(CommitteeMode::Full),
            other => Err(format!(
                "unknown committee mode '{other}' (expected off|planner|full)"
            )),
        }
    }
}

/// Configuration for the multi-LLM committee. When `mode == Off` (default),
/// the harness behaves exactly like the single-agent baseline. When enabled,
/// each role can use an independent model — empty strings fall back to
/// `models.main`, so a project can opt in by setting only `mode = "full"`.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct CommitteeConfig {
    /// Activation mode.
    #[serde(default)]
    pub mode: CommitteeMode,
    /// Model used by the Planner role. Empty falls back to `models.main`.
    #[serde(default)]
    pub planner_model: String,
    /// Model used by the Reviewer role. Empty falls back to `models.main`.
    #[serde(default)]
    pub reviewer_model: String,
    /// Model used by the Executor role. Empty falls back to `models.main`.
    #[serde(default)]
    pub executor_model: String,
    /// Maximum number of reviewer re-passes before auto-`Block` fires.
    #[serde(default = "default_max_review_passes")]
    pub max_review_passes: u32,
    /// Minimum task length (UTF-8 chars) that triggers the planner
    /// preflight pass. Short chat-style inputs ("hi", "what does this
    /// do") below this threshold skip planning even when the committee
    /// is enabled, since planning overhead would dominate. `0` (default)
    /// keeps the legacy always-on behaviour.
    #[serde(default)]
    pub min_task_chars: usize,
    /// When `true`, the harness runs a single capped-output
    /// classification round-trip to the main model before the planner
    /// preflight and skips planning unless the task is judged
    /// `complex` or `architectural`. Replaces the brittle char-count
    /// heuristic with a model verdict. Off by default to avoid the
    /// extra round trip; turn on when the operator wants the planner
    /// to fire selectively without manual tuning. On by default — a
    /// fast complexity classification with the main model is cheap
    /// (cap-output prompt) and prevents the planner from firing on
    /// trivial chat-y tasks. Operators with a free-tier subscription
    /// can flip this off to skip even the gate call.
    #[serde(default = "default_true")]
    pub use_llm_complexity_gate: bool,
}

impl Default for CommitteeConfig {
    fn default() -> Self {
        Self {
            mode: CommitteeMode::Off,
            planner_model: String::new(),
            reviewer_model: String::new(),
            executor_model: String::new(),
            max_review_passes: default_max_review_passes(),
            min_task_chars: 0,
            use_llm_complexity_gate: true,
        }
    }
}

fn default_max_review_passes() -> u32 {
    3
}

/// Display locale for user-facing strings in the TUI.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Locale {
    /// English.
    #[default]
    En,
    /// Korean.
    Ko,
}

impl fmt::Display for Locale {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Locale::En => formatter.write_str("en"),
            Locale::Ko => formatter.write_str("ko"),
        }
    }
}

impl std::str::FromStr for Locale {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value.trim().to_ascii_lowercase().as_str() {
            "en" | "english" => Ok(Locale::En),
            "ko" | "korean" | "kr" => Ok(Locale::Ko),
            other => Err(format!("unsupported locale: {other}")),
        }
    }
}

/// Terminal UI configuration.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct TuiConfig {
    /// Theme identifier.
    #[serde(default = "default_tui_theme")]
    pub theme: String,
    /// Display locale for status text, queue messages, and other free-form UI strings.
    #[serde(default)]
    pub language: Locale,
    /// Whether model thinking should be shown when available.
    #[serde(default = "default_true")]
    pub show_thinking: bool,
    /// Whether token totals should be shown.
    #[serde(default = "default_true")]
    pub show_token_count: bool,
    /// Whether estimated cost should be shown.
    #[serde(default = "default_true")]
    pub show_cost: bool,
    /// Whether prompt cache hit rate should be shown.
    #[serde(default = "default_true")]
    pub show_cache_rate: bool,
    /// Whether the right-side status panel should be shown. Default is
    /// OFF — the transcript runs full-width so drag-selection grabs only
    /// chat content (no status chrome). Operator can summon it on demand
    /// with `Ctrl+]`; the same key toggles it back.
    #[serde(default = "default_false")]
    pub show_subagent_panel: bool,
    /// Streaming presentation speed: realtime, fast, or instant.
    #[serde(default = "default_stream_speed")]
    pub stream_speed: String,
    /// Whether the Peridot deer mascot should be rendered in the side panel.
    #[serde(default = "default_true")]
    pub show_mascot: bool,
}

impl Default for TuiConfig {
    fn default() -> Self {
        Self {
            theme: default_tui_theme(),
            language: Locale::default(),
            show_thinking: true,
            show_token_count: true,
            show_cost: true,
            show_cache_rate: true,
            show_subagent_panel: false,
            stream_speed: default_stream_speed(),
            show_mascot: true,
        }
    }
}

/// Cross-surface UI preferences. Today this only holds the locale that
/// drives label translation in both the TUI settings screen and the VS
/// Code settings webview, but it's its own struct so adding e.g. a
/// theme override or a font size later doesn't force a TOML schema
/// change. `language: Option<Locale>` (rather than a defaulted enum)
/// lets `None` mean "use the legacy `[tui].language` value" — the
/// `PeridotConfig::effective_language` helper performs the lookup.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct UiConfig {
    /// Preferred display locale for setting labels, help text, and
    /// future UI chrome. `None` defers to `tui.language` for backward
    /// compatibility; once a user saves through the settings webview
    /// or `peridot setting`, this gets populated with `Some(...)` and
    /// becomes the source of truth.
    #[serde(default)]
    pub language: Option<Locale>,
}

fn default_tui_theme() -> String {
    "peridot-night".to_string()
}

fn default_stream_speed() -> String {
    "realtime".to_string()
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

/// Self-update notification and installation settings.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct UpdatesConfig {
    /// Whether interactive sessions check for a newer release.
    #[serde(default = "default_true")]
    pub auto_check: bool,
    /// Minimum interval between update checks.
    #[serde(default = "default_auto_check_interval")]
    pub auto_check_interval: String,
    /// Whether an available update may be installed without prompting.
    #[serde(default)]
    pub auto_install: bool,
}

impl Default for UpdatesConfig {
    fn default() -> Self {
        Self {
            auto_check: true,
            auto_check_interval: default_auto_check_interval(),
            auto_install: false,
        }
    }
}

fn default_auto_check_interval() -> String {
    "24h".to_string()
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
    /// Whether the docker filesystem should be read-only outside the
    /// `/workspace` mount. When `true`, the container is invoked with
    /// `--read-only --tmpfs /tmp:rw,size=64m`, so the only writable
    /// surface is the project mount. Defaults to `false` for backward
    /// compatibility — existing installs that depend on transient
    /// writes outside the workspace (rustup, cargo target outside the
    /// project, npm global cache) keep working without surprise.
    #[serde(default)]
    pub docker_read_only_rootfs: bool,
    /// Soft per-command timeout in seconds. `0` (default) disables the
    /// cap; otherwise a `shell_exec` invocation that runs longer than
    /// this is killed (via the same path as Esc cancel) and reported as
    /// a timeout error. Acts as a guard against runaway loops in
    /// long-running goal-mode runs.
    #[serde(default = "default_shell_command_timeout_seconds")]
    pub shell_command_timeout_seconds: u64,
    /// Optional Docker memory limit (e.g. `"512m"`, `"2g"`). Empty
    /// string disables it. Forwarded as `--memory` so the kernel OOM
    /// killer terminates a runaway container instead of pinning the
    /// host.
    #[serde(default)]
    pub docker_memory_limit: String,
    /// When `true`, `shell_exec` does not actually execute commands.
    /// Returns a synthetic `ToolResult` describing the would-be
    /// invocation (the resolved program + args + cwd) and leaves the
    /// workspace untouched. Used for safety drills and CI smoke tests.
    #[serde(default)]
    pub shell_dry_run: bool,
    /// Whether dependency installation commands require explicit approval.
    #[serde(default = "default_ask_before_install")]
    pub ask_before_install: bool,
    /// Whether destructive delete/history commands require explicit approval.
    #[serde(default = "default_ask_before_delete")]
    pub ask_before_delete: bool,
    /// Exact shell commands approved by the operator for the current config scope.
    #[serde(default)]
    pub approved_shell_commands: Vec<String>,
    /// Shell path substrings approved by the operator for destructive commands.
    #[serde(default)]
    pub approved_shell_path_scopes: Vec<String>,
    /// Exact tool calls approved by the operator for the current config scope.
    #[serde(default)]
    pub approved_tool_calls: Vec<String>,
    /// Tool names approved for the rest of the current session.
    #[serde(default)]
    pub approved_session_tools: Vec<String>,
    /// Tool/path pairs approved by the operator for the current config scope.
    #[serde(default)]
    pub approved_tool_path_scopes: Vec<String>,
}

impl Default for SecurityConfig {
    fn default() -> Self {
        Self {
            sandbox: SandboxMode::None,
            docker_image: default_docker_image(),
            docker_network: false,
            docker_read_only_rootfs: false,
            shell_command_timeout_seconds: default_shell_command_timeout_seconds(),
            docker_memory_limit: String::new(),
            shell_dry_run: false,
            ask_before_install: default_ask_before_install(),
            ask_before_delete: default_ask_before_delete(),
            approved_shell_commands: Vec::new(),
            approved_shell_path_scopes: Vec::new(),
            approved_tool_calls: Vec::new(),
            approved_session_tools: Vec::new(),
            approved_tool_path_scopes: Vec::new(),
        }
    }
}

fn default_docker_image() -> String {
    "rust:1-bookworm".to_string()
}

fn default_shell_command_timeout_seconds() -> u64 {
    0
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
///
/// Only `main` is operator-configurable. The goal-checker and compaction
/// roles deliberately track `main` 1:1 so a single `models.main = "..."`
/// switch reroutes every internal call site — no chance of a forgotten
/// `models.goal_checker` quietly hitting an unrelated model after the
/// operator thought they migrated. Accessors are exposed via
/// [`ModelsConfig::goal_checker`] / [`ModelsConfig::compaction`] which
/// always return `main`.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ModelsConfig {
    /// Main agent model. Also the implicit goal-checker and compaction
    /// model (see struct docs).
    #[serde(default = "default_main_model")]
    pub main: String,
    /// Reasoning intensity applied to every request the main agent sends.
    /// Cheap chat models ignore this; o-series / gpt-5 / Anthropic extended
    /// thinking models translate it to their native reasoning controls.
    /// Defaults to `Off` so cost stays predictable; opt in via toml or the
    /// `/reasoning <off|low|medium|high|xhigh>` slash command.
    #[serde(default)]
    pub reasoning_effort: ReasoningEffort,
    /// Optional provider service tier. `fast` maps to the provider's
    /// priority/fast path where supported; `None` leaves provider defaults.
    #[serde(default)]
    pub service_tier: Option<String>,
}

impl ModelsConfig {
    /// Model used by the goal-checker / grader role. Always tracks `main`.
    pub fn goal_checker(&self) -> &str {
        &self.main
    }

    /// Model used by the context compaction step. Always tracks `main`.
    pub fn compaction(&self) -> &str {
        &self.main
    }
}

impl Default for ModelsConfig {
    fn default() -> Self {
        Self {
            main: default_main_model(),
            reasoning_effort: ReasoningEffort::default(),
            service_tier: None,
        }
    }
}

fn default_main_model() -> String {
    "claude-sonnet-4-6".to_string()
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
    /// Automatically run `verify_build` after direct file edits
    /// (`file_write` / `file_patch`). On by default — the harness
    /// runs more reliably when compile errors surface in the same
    /// turn that caused them, so the model can fix them while the
    /// change is still fresh in context. Projects with a slow build
    /// can opt out via `.peridot/config.toml`.
    ///
    /// Note: the serde default mirrors the struct default so a partial
    /// TOML that omits this field still gets the harness-optimised
    /// behaviour, not silently `false`.
    #[serde(default = "default_true")]
    pub auto_verify_after_mutation: bool,
    /// Automatically call the LLM-based grader (`grade_work`) after
    /// `agent_done` to decide whether the task is actually shippable.
    /// When the verdict is `passed: false`, the recommendations are
    /// injected as a `PlanReminder` and the loop continues for another
    /// turn instead of stopping. On by default so "first agent_done"
    /// doesn't ship half-finished work.
    #[serde(default = "default_true")]
    pub auto_grade_on_done: bool,
}

impl Default for DefaultsConfig {
    fn default() -> Self {
        Self {
            mode: ExecutionMode::default(),
            permission: PermissionMode::default(),
            max_turns: default_max_turns(),
            budget_usd: default_budget_usd(),
            budget_warning_pct: default_budget_warning_pct(),
            // Defaults flipped to ON in v0.7.3. The harness now verifies
            // every mutation and grades every agent_done by default so
            // an agent that "looks finished" really is — the operator
            // doesn't need to know the toggle exists to get the safe
            // behaviour. Opt out by setting these to false in
            // `.peridot/config.toml`.
            auto_verify_after_mutation: true,
            auto_grade_on_done: true,
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
    // Effectively disabled. See peridot_context::ContextLimits::default() for the
    // rationale: modern models support large enough contexts that disk offload causes
    // more harm (recursive re-reads on smaller models) than memory benefit.
    //
    // `i64::MAX` (≈9.2 × 10¹⁸) is chosen instead of `usize::MAX` so the value
    // round-trips through TOML, whose integer spec is signed 64-bit. Still
    // larger than any realistic observation size.
    i64::MAX as usize
}

fn default_observation_max_chars() -> usize {
    8_000
}

fn default_thinking() -> String {
    "auto".to_string()
}

/// Learned memory and self-improvement configuration.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct MemoryConfig {
    /// Whether completed sessions are stored in memory.
    #[serde(default = "default_true")]
    pub session_history: bool,
    /// Whether successful sessions can create auto skills.
    #[serde(default = "default_true")]
    pub auto_skills: bool,
    /// Whether generated skills should be marked for human review.
    #[serde(default = "default_true")]
    pub skills_review: bool,
    /// Maximum session summaries to keep.
    #[serde(default = "default_max_sessions_stored")]
    pub max_sessions_stored: usize,
    /// Model the Curator uses for its LLM reflection pass. When unset,
    /// the Curator falls back to `models.main`. A cheaper/faster model
    /// here is usually right: the Curator's job is bookkeeping, not
    /// novel generation.
    #[serde(default)]
    pub curator_model: Option<String>,
    /// Whether the cross-session reflection pass promotes repeated
    /// tool-call patterns into auto-skills. Off by default — the
    /// single-session capture (`auto_skills`) is conservative and
    /// always-on; the cross-session pass costs one LLM call per
    /// promoted pattern, so operators opt in.
    #[serde(default)]
    pub auto_skill_reflection: bool,
    /// Minimum occurrence count an n-gram must reach before the
    /// reflection pass considers it for promotion. Default 5 — high
    /// enough to suppress one-off pairings, low enough that a
    /// daily-shipping routine reaches it inside a week.
    #[serde(default = "default_ngram_min_count")]
    pub ngram_min_count: u32,
    /// Maximum n-gram length recorded by `save_tool_sequence`. Default
    /// 3 — bigrams catch "A→B always together" patterns, trigrams
    /// catch end-to-end mini-workflows. Wider n-grams explode the
    /// table without paying for themselves on most workspaces.
    #[serde(default = "default_ngram_max_length")]
    pub ngram_max_length: u32,
    /// Maximum number of n-grams the reflection pass promotes per run.
    /// Caps cost on the first run against an aged DB that has lots of
    /// pending candidates.
    #[serde(default = "default_ngram_batch_cap")]
    pub ngram_batch_cap: usize,
}

fn default_ngram_min_count() -> u32 {
    5
}

fn default_ngram_max_length() -> u32 {
    3
}

fn default_ngram_batch_cap() -> usize {
    8
}

impl Default for MemoryConfig {
    fn default() -> Self {
        Self {
            session_history: true,
            auto_skills: true,
            skills_review: true,
            max_sessions_stored: default_max_sessions_stored(),
            curator_model: None,
            // Default flipped to ON in v0.7.3. Cross-session reflection
            // only runs on the 7-day idle trigger, so a normal session
            // pays nothing for it; the cost only materialises after
            // the project has been idle for a week, at which point a
            // single batched LLM call promotes any pattern the operator
            // has actually used 5+ times.
            auto_skill_reflection: true,
            ngram_min_count: default_ngram_min_count(),
            ngram_max_length: default_ngram_max_length(),
            ngram_batch_cap: default_ngram_batch_cap(),
        }
    }
}

fn default_true() -> bool {
    true
}

fn default_false() -> bool {
    false
}

fn default_max_sessions_stored() -> usize {
    100
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
    /// Per-request timeout for initialize, list, and tool calls.
    #[serde(default = "default_mcp_timeout_seconds")]
    pub timeout_seconds: u64,
    /// Default permission level applied to every tool exposed by this
    /// server. The legacy behaviour ("everything is System") corresponds
    /// to `system`; servers that only expose read-only operations
    /// (e.g. a read-only Postgres MCP) can drop their gate to `read` or
    /// `write` so the harness does not gratuitously ask for approval.
    #[serde(default = "default_mcp_permission_level")]
    pub default_permission: String,
    /// Per-tool permission overrides keyed by the raw tool name as
    /// reported by the MCP server (NOT the `mcp_<server>_<tool>` adapter
    /// name). Lets the operator promote one dangerous tool to
    /// `destructive` while keeping the rest of the server at the
    /// server-wide default.
    #[serde(default)]
    pub tool_permission_overrides: BTreeMap<String, String>,
    /// Cache TTL in seconds for `tools/list` responses. Default is 300
    /// (5 min) so a server that exposes a stable catalogue is not
    /// re-listed on every session start within the same process. `0`
    /// disables caching.
    #[serde(default = "default_mcp_schema_cache_seconds")]
    pub schema_cache_seconds: u64,
}

fn default_mcp_timeout_seconds() -> u64 {
    30
}

fn default_mcp_permission_level() -> String {
    "system".to_string()
}

fn default_mcp_schema_cache_seconds() -> u64 {
    300
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
        assert_eq!(config.mcp[0].timeout_seconds, 30);
    }

    #[test]
    fn parses_mcp_timeout_config() {
        let config = toml::from_str::<PeridotConfig>(
            r#"
            [[mcp]]
            name = "jira"
            transport = "stdio"
            command = "npx"
            timeout_seconds = 5
            "#,
        )
        .unwrap();

        assert_eq!(config.mcp[0].timeout_seconds, 5);
    }

    #[test]
    fn xhigh_reasoning_maps_to_provider_labels() {
        assert_eq!(
            ReasoningEffort::parse("xhigh"),
            Some(ReasoningEffort::XHigh)
        );
        assert_eq!(
            ReasoningEffort::parse("x-high"),
            Some(ReasoningEffort::XHigh)
        );
        assert_eq!(ReasoningEffort::XHigh.openai_effort_label(), Some("xhigh"));
        assert_eq!(
            ReasoningEffort::XHigh.anthropic_budget_tokens(),
            Some(32_768)
        );
        assert_eq!(ReasoningEffort::XHigh.to_string(), "xhigh");
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
    fn parses_tui_config() {
        let config = toml::from_str::<PeridotConfig>(
            r#"
            [tui]
            theme = "light"
            show_token_count = false
            show_cost = false
            show_cache_rate = false
            show_subagent_panel = false
            stream_speed = "instant"
            "#,
        )
        .unwrap();

        assert_eq!(config.tui.theme, "light");
        assert!(!config.tui.show_token_count);
        assert!(!config.tui.show_cost);
        assert!(!config.tui.show_cache_rate);
        assert!(!config.tui.show_subagent_panel);
        assert_eq!(config.tui.stream_speed, "instant");
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

    #[test]
    fn parses_updates_config() {
        let config = toml::from_str::<PeridotConfig>(
            r#"
            [updates]
            auto_check = false
            auto_check_interval = "12h"
            auto_install = true
            "#,
        )
        .unwrap();

        assert!(!config.updates.auto_check);
        assert_eq!(config.updates.auto_check_interval, "12h");
        assert!(config.updates.auto_install);
    }

    #[test]
    fn parses_memory_config() {
        let config = toml::from_str::<PeridotConfig>(
            r#"
            [memory]
            session_history = false
            auto_skills = false
            skills_review = false
            max_sessions_stored = 12
            "#,
        )
        .unwrap();

        assert!(!config.memory.session_history);
        assert!(!config.memory.auto_skills);
        assert!(!config.memory.skills_review);
        assert_eq!(config.memory.max_sessions_stored, 12);
    }

    #[test]
    fn committee_defaults_to_off_with_empty_role_models() {
        let config = PeridotConfig::default();
        assert_eq!(config.committee.mode, CommitteeMode::Off);
        assert!(config.committee.planner_model.is_empty());
        assert!(config.committee.reviewer_model.is_empty());
        assert!(config.committee.executor_model.is_empty());
        assert_eq!(config.committee.max_review_passes, 3);
    }

    #[test]
    fn configured_model_suggestions_collects_runtime_and_role_models() {
        let mut config = PeridotConfig::default();
        config.models.main = "main-model".to_string();
        config.subagents.default_model = Some("subagent-model".to_string());
        config.committee.planner_model = "planner-model".to_string();
        config.committee.reviewer_model = "reviewer-model".to_string();
        config.committee.executor_model = "main-model".to_string();

        assert_eq!(
            configured_model_suggestions(&config, Some("runtime-model")),
            vec![
                "main-model",
                "planner-model",
                "reviewer-model",
                "runtime-model",
                "subagent-model"
            ]
        );
    }

    #[test]
    fn parses_committee_config_from_toml() {
        let config = toml::from_str::<PeridotConfig>(
            r#"
            [committee]
            mode = "full"
            planner_model = "claude-haiku-4-5"
            reviewer_model = "openai-gpt-4o-mini"
            executor_model = "claude-opus-4-7"
            max_review_passes = 5
            "#,
        )
        .unwrap();

        assert_eq!(config.committee.mode, CommitteeMode::Full);
        assert_eq!(config.committee.planner_model, "claude-haiku-4-5");
        assert_eq!(config.committee.reviewer_model, "openai-gpt-4o-mini");
        assert_eq!(config.committee.executor_model, "claude-opus-4-7");
        assert_eq!(config.committee.max_review_passes, 5);
    }

    #[test]
    fn committee_mode_round_trips_through_from_str_and_display() {
        use std::str::FromStr;
        for mode in [
            CommitteeMode::Off,
            CommitteeMode::Planner,
            CommitteeMode::Full,
        ] {
            let rendered = mode.to_string();
            let parsed = CommitteeMode::from_str(&rendered).unwrap();
            assert_eq!(parsed, mode);
        }
        assert!(CommitteeMode::from_str("nope").is_err());
    }
}
