use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use peridot_agents::SubAgent;
use peridot_common::{
    AgentPhase, ExecutionMode, PeriError, PeriResult, SecurityConfig, ToolCall, ToolResult,
};
use peridot_context::{
    ContextEntry, ContextManager, ContextSource, EvidenceLedger, EvidenceRef,
    estimate_tokens_for_text,
};
use peridot_llm::{
    CompletionRequest, ImageContent, LlmMessage, LlmProvider, ToolChoice, ToolDefinition,
    ToolInvocation, Usage,
};
use peridot_tools::audit::{AuditEvent, append_audit_event};
use peridot_tools::hooks::{HookRunner, tool_hook_variables};
use peridot_tools::{AgentMessageBus, AskUserPort, ToolContext, ToolRegistry};

use crate::agent_helpers::approval_required_error;
use crate::approval::tool_call_has_confirmation_grant;
use crate::checkpoint::write_file_checkpoint;
use crate::permissions::ensure_tool_allowed;
use crate::prompt::{read_plan_reminder, system_prompt_for_role};
use crate::recovery::{
    budget_exceeded_message, run_context_compacted_hook, run_recovery_event_hook,
};
use crate::requests::{
    AgentRunEvent, AgentRunRequest, AgentRunSummary, AgentTurnOutcome, AgentTurnRequest,
    FileDiffPayload, PlanStepUpdate, StopReason,
};
use crate::role::AgentRole;
use crate::state::AgentState;
use crate::usage::{accumulate_usage, stream_completion_with_chunks};
use peridot_common::CancelToken;

/// Function the auto-grade gate consults to produce a worktree diff
/// when the production `collect_git_diff` is unsuitable (e.g. in
/// unit tests that don't bootstrap a real git repo).
pub type GraderDiffProvider = std::sync::Arc<dyn Fn(&std::path::Path) -> String + Send + Sync>;

/// What the driver should do after the turn_error policy chain runs.
/// Flattens `Decision` into the driver's specific control-flow needs
/// (some `Decision` variants don't make sense at this hook).
#[derive(Debug)]
enum TurnErrorAction {
    /// No policy claimed the error → sleep briefly to avoid hot-looping
    /// the same failure and retry.
    Continue,
    /// A policy injected a reminder and asked to retry immediately.
    Retry,
    /// Treat as SkipTurn — re-enter the loop from the top.
    Skip,
    /// Stop the run with this reason; the optional message gets
    /// forwarded as a `Recovery` event and recovery_abort hook.
    Stop {
        reason: StopReason,
        message: Option<String>,
    },
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct RequestContextEstimate {
    total_tokens: u64,
    message_tokens: u64,
    system_tokens: u64,
    tool_schema_tokens: u64,
    overhead_tokens: u64,
}

/// Build a final `AgentRunSummary`, fire the [`AgentRunEvent::Finished`]
/// event, and return the summary so the caller can `return Ok(...)`.
///
/// Used at every run-termination site in `run_until_done_with_events`
/// (Done, Interrupted, Budget, ApprovalRequired, MaxTurns). Centralising
/// the pattern keeps `duration_ms` consistent and ensures every exit
/// path emits a Finished event exactly once.
fn finalize_run<F>(
    turns: Vec<AgentTurnOutcome>,
    usage: Usage,
    stopped_reason: StopReason,
    started_at: std::time::Instant,
    events: &mut F,
) -> AgentRunSummary
where
    F: FnMut(AgentRunEvent),
{
    let summary = AgentRunSummary {
        turns,
        usage,
        stopped_reason,
        duration_ms: started_at.elapsed().as_millis() as u64,
    };
    events(AgentRunEvent::Finished {
        summary: summary.clone(),
    });
    summary
}

// `MAX_ERROR_RECOVERY_ATTEMPTS` moved into `ErrorRecoveryPolicy` (see
// loop_policy/recovery.rs) — that's where the attempt counter lives now.
const ERROR_RECOVERY_RETRY_DELAY: Duration = Duration::from_secs(3);

/// Peridot harness agent shell.
pub struct HarnessAgent {
    state: AgentState,
    context: ContextManager,
    tools: ToolRegistry,
    cancel: Option<CancelToken>,
    context_snapshot_path: Option<PathBuf>,
    agents_md_path: Option<PathBuf>,
    agents_md_signature: Option<(u64, u64)>,
    role: AgentRole,
    subagent_runner: Option<Arc<dyn SubAgent>>,
    ask_user_port: Option<Arc<dyn AskUserPort>>,
    /// Optional inter-session message bus. When set, the harness folds
    /// the bus into every `ToolContext` (so `agent_message` can route to
    /// peers) AND drains its own inbox at the start of every turn so
    /// peer messages reach the model as `PlanReminder` entries.
    message_bus: Option<Arc<dyn AgentMessageBus>>,
    /// Optional session id. Required only when `message_bus` is set —
    /// it identifies which inbox the harness drains. `None` keeps the
    /// legacy single-session behaviour.
    session_id: Option<String>,
    auto_verify_after_mutation: bool,
    auto_grade_on_done: bool,
    auto_fix_cap: u32,
    /// Optional override for the worktree diff fed to the auto-grade
    /// gate. Tests inject a closure here so the empty-diff fast path
    /// can be exercised without a real git repo; production leaves
    /// this `None` and the gate calls `collect_git_diff` directly.
    grader_diff_provider: Option<GraderDiffProvider>,
    /// Optional flag the operator sets via `/compact` to force an LLM
    /// recap on the next turn boundary, even when the buffer is well
    /// below the auto trigger. Atomic so the slash command thread and
    /// the agent loop can share it without locking.
    compact_request: Option<Arc<AtomicBool>>,
    /// Sidecar path used to persist a pending tool call when the
    /// previous run halted on `ApprovalRequired`. On the next session
    /// start the harness reads the file, executes the exact tool call
    /// (under whatever security/permission posture the operator
    /// granted), folds the result into context, and deletes the
    /// sidecar. The model is NOT re-asked; the run picks up from the
    /// point it stopped.
    pending_resume_path: Option<PathBuf>,
    /// Pre-built tool definitions surfaced to the LLM. The registry is
    /// frozen at `new()` time (the field below takes ownership), so the
    /// provider-neutral descriptor list never changes during the session.
    /// Caching it here avoids walking the `BTreeMap` and re-running each
    /// tool's `parameters_schema()` (which often allocates a fresh JSON
    /// tree) on every turn.
    cached_tool_definitions: Vec<ToolDefinition>,
    /// Whether attached images may be sent to vision-capable models
    /// (feature F2, `[vision] enabled`). When `false`, images are always
    /// stripped regardless of model capability. Defaults to `true`.
    vision_enabled: bool,
    /// Optional vision-capable model id (`[vision] model`) to route a turn to
    /// when it carries images but the active model is text-only. Must be served
    /// by the same provider as the active model. `None` leaves the model as-is.
    vision_model: Option<String>,
    /// Optional OCR backend (F2 milestone 5). When set, images bound for a
    /// text-only model are run through it and their text injected as an
    /// `<image-ocr>` block instead of being dropped. `None` keeps the
    /// placeholder-only behavior.
    image_text_extractor: Option<Arc<dyn ImageTextExtractor>>,
}

/// Extracts text from an attached image so a text-only model can still reason
/// about it (F2 milestone 5, OCR fallback). Implementations wrap an OCR engine
/// (e.g. Tesseract) behind a build feature; the core loop holds one only when
/// the operator opted into OCR. Returns `None` when no text could be extracted.
pub trait ImageTextExtractor: Send + Sync {
    /// Returns the recognized text of `image`, or `None` if extraction failed
    /// or yielded nothing.
    fn extract(&self, image: &ImageContent) -> Option<String>;
}

/// The model id to send a turn to once vision routing is considered. When the
/// turn carries images, vision is enabled, the active model is text-only, and a
/// vision-capable override is configured, route to the override so the images
/// reach a capable model. Otherwise the active model is used unchanged.
fn route_vision_model(
    active: &str,
    override_model: Option<&str>,
    vision_enabled: bool,
    has_images: bool,
) -> String {
    if has_images
        && vision_enabled
        && !peridot_llm::model_supports_vision(active)
        && let Some(candidate) = override_model
        && peridot_llm::model_supports_vision(candidate)
    {
        return candidate.to_string();
    }
    active.to_string()
}

/// Reconciles outgoing image blocks with what the active model can accept.
///
/// A no-op when vision is enabled and the model is vision-capable (images pass
/// through). Otherwise the images cannot be sent, so they are stripped and the
/// user turn keeps its text placeholder. When vision is enabled but the model
/// is text-only and an OCR `extractor` is supplied, each image's recognized
/// text is appended to the message as a tagged `<image-ocr>` block first, so
/// the model still sees the image content as (untrusted) text.
fn enforce_vision_capability(
    messages: &mut [LlmMessage],
    model: &str,
    vision_enabled: bool,
    extractor: Option<&dyn ImageTextExtractor>,
) {
    if vision_enabled && peridot_llm::model_supports_vision(model) {
        return;
    }
    for message in messages.iter_mut() {
        if message.images.is_empty() {
            continue;
        }
        // OCR fallback only applies when the vision feature is on (the model
        // just can't see images); a disabled feature drops them silently.
        if vision_enabled && let Some(extractor) = extractor {
            for image in &message.images {
                if let Some(text) = extractor.extract(image) {
                    append_ocr_block(&mut message.content, &text);
                }
            }
        }
        message.images.clear();
    }
}

/// Appends an `<image-ocr>` tagged block carrying OCR-extracted `text` to a
/// message's content. The tag marks the text as external/observed content
/// (the same untrusted-content convention the context layer uses), so the
/// model treats it as evidence rather than instructions.
fn append_ocr_block(content: &mut String, text: &str) {
    if !content.is_empty() {
        content.push_str("\n\n");
    }
    content.push_str("<image-ocr>\n");
    content.push_str(text.trim());
    content.push_str("\n</image-ocr>");
}

impl HarnessAgent {
    /// Creates a harness agent from state and dependencies.
    pub fn new(state: AgentState, context: ContextManager, tools: ToolRegistry) -> Self {
        let cached_tool_definitions = registry_tool_definitions(&tools);
        Self {
            state,
            context,
            tools,
            cancel: None,
            context_snapshot_path: None,
            agents_md_path: None,
            agents_md_signature: None,
            role: AgentRole::default(),
            subagent_runner: None,
            ask_user_port: None,
            message_bus: None,
            session_id: None,
            auto_verify_after_mutation: false,
            auto_grade_on_done: false,
            auto_fix_cap: 3,
            grader_diff_provider: None,
            compact_request: None,
            pending_resume_path: None,
            cached_tool_definitions,
            vision_enabled: true,
            vision_model: None,
            image_text_extractor: None,
        }
    }

    /// Enables or disables sending attached images to vision-capable models
    /// (feature F2, `[vision] enabled`). Defaults to enabled.
    pub fn set_vision_enabled(&mut self, enabled: bool) {
        self.vision_enabled = enabled;
    }

    /// Sets the vision-capable model override (`[vision] model`). When a turn
    /// carries images but the active model is text-only, the request is routed
    /// to this model (which must be served by the same provider). `None`
    /// disables routing.
    pub fn set_vision_model(&mut self, model: Option<String>) {
        self.vision_model = model.filter(|m| !m.is_empty());
    }

    /// Installs an OCR backend for the text-only image fallback (F2 milestone
    /// 5). When set, images that cannot be sent to a vision model have their
    /// recognized text injected as an `<image-ocr>` block instead of being
    /// dropped.
    pub fn set_image_text_extractor(&mut self, extractor: Arc<dyn ImageTextExtractor>) {
        self.image_text_extractor = Some(extractor);
    }

    /// Configures the file Pending tool calls are persisted to when an
    /// approval halt fires, and re-loaded from on next session start.
    /// Typically points at `.peridot/sessions/<id>/pending_resume.bin`.
    pub fn set_pending_resume_path(&mut self, path: PathBuf) {
        self.pending_resume_path = Some(path);
    }

    /// Attaches a shared atomic flag the operator can set (e.g. via
    /// `/compact`) to force an LLM recap on the next turn boundary.
    pub fn set_compact_request(&mut self, flag: Arc<AtomicBool>) {
        self.compact_request = Some(flag);
    }

    /// Installs a subagent runner. The harness injects this into every
    /// `ToolContext` it builds so `agent_delegate` dispatches through it
    /// (typically an `InnerLoopSubAgent` running a bounded child harness)
    /// instead of only preparing a workspace.
    pub fn set_subagent_runner(&mut self, runner: Arc<dyn SubAgent>) {
        self.subagent_runner = Some(runner);
    }

    /// Installs an ask-user port. The harness injects it into every
    /// `ToolContext` it builds so `agent_ask_user` actually awaits a
    /// real user answer through the interactive front-end. Headless and
    /// test harnesses leave it unset and the tool keeps its synthesised
    /// default fallback.
    pub fn set_ask_user_port(&mut self, port: Arc<dyn AskUserPort>) {
        self.ask_user_port = Some(port);
    }

    /// Installs an inter-session message bus. The harness:
    ///
    /// 1. Folds the bus into every `ToolContext`, enabling
    ///    `agent_message` to actually route to peers.
    /// 2. Drains its own inbox at the start of every turn and
    ///    injects received messages as PlanReminder context entries
    ///    so the model sees them on the next call.
    ///
    /// `session_id` must be set for the drain step to know which inbox
    /// to read; see [`Self::set_session_id`].
    pub fn set_message_bus(&mut self, bus: Arc<dyn AgentMessageBus>) {
        self.message_bus = Some(bus);
    }

    /// Identifies this harness's session for inbox routing. Required
    /// when `set_message_bus` is also set; ignored otherwise.
    pub fn set_session_id(&mut self, id: impl Into<String>) {
        self.session_id = Some(id.into());
    }

    /// Enables the "verify after every mutation" auto-loop. When on,
    /// `verify_build` runs automatically after every successful
    /// `file_write` / `file_patch` and its result is
    /// injected into context as a `PlanReminder`, so the next model
    /// turn sees compile errors immediately. Off by default.
    pub fn set_auto_verify_after_mutation(&mut self, enabled: bool) {
        self.auto_verify_after_mutation = enabled;
    }

    /// Enables LLM-based grading on `agent_done`. When the verdict
    /// fails, the recommendations are folded back into context and
    /// the loop continues for another turn instead of finishing.
    /// Off by default.
    pub fn set_auto_grade_on_done(&mut self, enabled: bool) {
        self.auto_grade_on_done = enabled;
    }

    /// Overrides the diff source used by the auto-grade gate. Tests
    /// call this to inject a deterministic diff so the empty-diff
    /// fast path can be skipped without a real git repo.
    pub fn set_grader_diff_provider<F>(&mut self, provider: F)
    where
        F: Fn(&std::path::Path) -> String + Send + Sync + 'static,
    {
        self.grader_diff_provider = Some(std::sync::Arc::new(provider));
    }

    /// Sets the maximum identical-failure attempts before the auto-fix
    /// circuit breaker fires. Sourced from `config.auto_fix.max_attempts`.
    pub fn set_auto_fix_cap(&mut self, cap: u32) {
        self.auto_fix_cap = cap;
    }

    /// Assigns the committee role this agent plays. Defaults to
    /// `AgentRole::Executor`, which keeps the legacy single-agent behaviour.
    pub fn set_role(&mut self, role: AgentRole) {
        self.role = role;
    }

    /// Returns the committee role this agent is configured to play.
    pub fn role(&self) -> AgentRole {
        self.role
    }

    /// Configures the AGENTS.md file the agent loop watches for changes.
    /// On every turn the loop compares the file's `(modified_unix, len)`
    /// fingerprint to the last seen one and, if it changed, re-reads the
    /// file and pushes its contents into context as a `PlanReminder` entry
    /// while emitting `AgentRunEvent::AgentsMdLoaded`.
    pub fn set_agents_md_path(&mut self, path: PathBuf) {
        self.agents_md_path = Some(path);
        self.agents_md_signature = None;
    }

    /// Attaches a cancellation token consulted between turns.
    pub fn with_cancel_token(mut self, token: CancelToken) -> Self {
        self.cancel = Some(token);
        self
    }

    /// Replaces the cancellation token in place.
    pub fn set_cancel_token(&mut self, token: CancelToken) {
        self.cancel = Some(token);
    }

    /// Returns a clone of the attached cancellation token, if any. Used by
    /// the committee loop in `peridot-cli` to check cancellation between
    /// executor turns and reviewer passes.
    pub fn cancel_token(&self) -> Option<CancelToken> {
        self.cancel.clone()
    }

    /// Returns a clone of the attached ask-user port, if any. Used by the
    /// committee loop to surface `Block` verdicts through the interactive
    /// front-end so the operator can override or accept.
    pub fn ask_user_port(&self) -> Option<Arc<dyn AskUserPort>> {
        self.ask_user_port.clone()
    }

    /// Configures the on-disk path the agent loop should snapshot its
    /// [`ContextManager`] entries into after every turn. The write happens
    /// atomically via `tempfile + rename` so concurrent crashes never expose
    /// half-written blobs.
    pub fn set_context_snapshot_path(&mut self, path: PathBuf) {
        self.context_snapshot_path = Some(path);
    }

    /// Returns the current agent state.
    pub fn state(&self) -> &AgentState {
        &self.state
    }

    /// Returns the context manager.
    pub fn context(&self) -> &ContextManager {
        &self.context
    }

    /// Returns a mutable context manager.
    pub fn context_mut(&mut self) -> &mut ContextManager {
        &mut self.context
    }

    /// Returns the tool registry.
    pub fn tools(&self) -> &ToolRegistry {
        &self.tools
    }

    /// Executes one tool call through the registered tool boundary.
    pub async fn execute_tool_call(
        &self,
        call: ToolCall,
        project_root: impl Into<PathBuf>,
    ) -> PeriResult<ToolResult> {
        self.execute_tool_call_with_denied_paths(call, project_root, Vec::new())
            .await
    }

    /// Executes one tool call with explicit project path boundaries.
    pub async fn execute_tool_call_with_denied_paths(
        &self,
        call: ToolCall,
        project_root: impl Into<PathBuf>,
        denied_paths: Vec<PathBuf>,
    ) -> PeriResult<ToolResult> {
        let (result, _diff) = self
            .execute_tool_call_with_runtime(
                call,
                project_root,
                denied_paths,
                peridot_common::HooksConfig::default(),
                SecurityConfig::default(),
            )
            .await?;
        Ok(result)
    }

    /// Executes one tool call with explicit boundaries and hook configuration.
    ///
    /// Returns the tool result paired with an optional [`FileDiffPayload`]
    /// describing the before/after content when the call mutated a file
    /// (`file_write` / `file_patch`). Callers wired into the
    /// [`AgentRunEvent`] stream forward the diff via
    /// [`AgentRunEvent::FileDiff`]; internal callers (auto-verify, the
    /// `agent_done` shortcut) discard it.
    pub async fn execute_tool_call_with_runtime(
        &self,
        call: ToolCall,
        project_root: impl Into<PathBuf>,
        denied_paths: Vec<PathBuf>,
        hooks: peridot_common::HooksConfig,
        security: SecurityConfig,
    ) -> PeriResult<(ToolResult, Option<FileDiffPayload>)> {
        let tool = self
            .tools
            .get(&call.name)
            .ok_or_else(|| PeriError::Tool(format!("unknown tool: {}", call.name)))?;
        ensure_tool_allowed(self.state.mode, self.state.phase, tool.group(), &call.name)?;
        let project_root = project_root.into();
        tool.validate_params(&call.parameters)?;
        if tool.requires_confirmation(self.state.permission)
            && !tool_call_has_confirmation_grant(&call, &security)
        {
            return Err(PeriError::PermissionDenied(format!(
                "{} requires explicit user approval",
                call.name
            )));
        }
        let mut ctx = ToolContext::new(project_root.clone(), self.state.permission)
            .with_denied_paths(denied_paths)
            .with_hooks(hooks)
            .with_security(security);
        if let Some(token) = self.cancel.clone() {
            ctx = ctx.with_cancel(token);
        }
        if let Some(runner) = self.subagent_runner.clone() {
            ctx = ctx.with_subagent_runner(runner);
        }
        if let Some(port) = self.ask_user_port.clone() {
            ctx = ctx.with_ask_user_port(port);
        }
        if let Some(bus) = self.message_bus.clone() {
            ctx = ctx.with_message_bus(bus);
        }
        let parent_packet = self.context.subagent_mission_packet(5_000);
        if !parent_packet.trim().is_empty() {
            ctx = ctx.with_parent_context_packet(parent_packet);
        }
        let runner = HookRunner::new(&project_root, ctx.hooks.clone());
        let mut variables = tool_hook_variables(&call.name, &call.parameters);
        variables.insert(
            "project_root".to_string(),
            project_root.display().to_string(),
        );
        variables.insert("workspace".to_string(), project_root.display().to_string());
        variables.insert("mode".to_string(), self.state.mode.to_string());
        variables.insert("permission".to_string(), self.state.permission.to_string());
        runner.run_tool_hooks(&format!("pre:{}", call.name), &variables)?;
        let tool_name = call.name.clone();
        let params = call.parameters.clone();
        let checkpoint = if tool.modifies_state() {
            write_file_checkpoint(&project_root, &tool_name, &params)
                .ok()
                .flatten()
        } else {
            None
        };
        let checkpoint_id = checkpoint.as_ref().map(|cp| cp.id.clone());
        let result = tool.execute(call.parameters, &ctx).await?;
        // Capture the post-mutation content for the file_diff event so
        // the TUI / future extension clients can render a real before/
        // after diff without re-walking the filesystem. Best-effort:
        // a stat or read failure just suppresses the event — the
        // checkpoint on disk is still the canonical rollback source.
        let file_diff = checkpoint.as_ref().and_then(|cp| {
            if !result.success {
                return None;
            }
            let after = std::fs::read_to_string(&cp.absolute_path).ok()?;
            Some(FileDiffPayload {
                tool_name: tool_name.clone(),
                path: cp.relative_path.clone(),
                before: cp.previous_content.clone(),
                after,
            })
        });
        let _ = append_audit_event(
            &project_root,
            &AuditEvent::tool_call(
                &tool_name,
                result.success,
                &result.summary,
                serde_json::json!({
                    "params": params,
                    "phase": self.state.phase,
                    "mode": self.state.mode,
                    "permission": self.state.permission,
                    "checkpoint_id": checkpoint_id
                }),
            ),
        );
        variables.insert(
            "result_json".to_string(),
            serde_json::to_string(&result).map_err(|err| {
                PeriError::Parse(format!("failed to serialize hook result: {err}"))
            })?,
        );
        runner.run_tool_hooks(&format!("post:{}", call.name), &variables)?;
        Ok((result, file_diff))
    }

    /// Runs one model/tool turn and records the observation in context.
    ///
    /// Takes `&dyn LlmProvider` (rather than the older `&P: LlmProvider
    /// + ?Sized`) so the run loop can stash the provider in `PolicyCx`
    /// without an unsized coercion. Concrete `&MyProvider` auto-coerces
    /// at the call site; `&*arc_dyn` is already `&dyn`. Internal
    /// helpers (`grader::grade_work`, etc.) keep their `?Sized` generic
    /// — `&dyn LlmProvider` flows through them as `P = dyn`.
    pub async fn run_turn(
        &mut self,
        provider: &dyn LlmProvider,
        request: AgentTurnRequest,
    ) -> PeriResult<AgentTurnOutcome> {
        self.run_turn_with_events(provider, request, &mut |_| {})
            .await
    }

    fn can_batch_read_only_tool_calls(&self, invocations: &[ToolInvocation]) -> bool {
        invocations.len() > 1
            && invocations.iter().all(|invocation| {
                invocation.name != "agent_done"
                    && risk_class_label_for(&self.tools, &invocation.name).as_deref()
                        == Some("read_only")
            })
    }

    /// Runs one model/tool turn and emits user-interface events.
    pub async fn run_turn_with_events<F>(
        &mut self,
        provider: &dyn LlmProvider,
        request: AgentTurnRequest,
        events: &mut F,
    ) -> PeriResult<AgentTurnOutcome>
    where
        F: FnMut(AgentRunEvent),
    {
        // Start of a new turn: bump the turn id so every entry
        // appended below shares one id, enabling later `/branch turn`
        // forks at this exact point.
        self.context.bump_turn_id();
        // Plan reminder is co-injected with a real user turn only.
        // Re-injecting on every internal multi-step iteration (after a
        // tool result, etc.) inflates the context by `todo.md`-sized
        // chunks each step — the operator saw 956 tokens for a one-word
        // "hi" because the same plan reminder was appended twice in a
        // two-step run. The model already sees the reminder once via
        // the previous turn's context, so a single injection per real
        // user prompt is sufficient.
        if let Some(user_input) = request.user_input {
            self.context
                .append(ContextEntry::trusted(ContextSource::User, user_input));
            if let Some(plan) = read_plan_reminder(&request.project_root) {
                self.context
                    .append(ContextEntry::trusted(ContextSource::PlanReminder, plan));
            }
        }
        let estimated_tokens = self.context.estimated_tokens();
        // Tier 3 first: ask the model to produce a structured recap.
        // When the operator queued `/compact` we bypass the threshold
        // entirely; otherwise the dynamic threshold (auto_compaction_pct
        // of the active model window) decides. Falls back to Tier 1
        // (deterministic summary) if the LLM call errors or produces
        // no compaction. Provider errors are swallowed so a compaction
        // hiccup never aborts the run.
        let force_compact = self
            .compact_request
            .as_ref()
            .map(|flag| flag.swap(false, Ordering::SeqCst))
            .unwrap_or(false);
        let mut compacted = if force_compact {
            self.context
                .force_compact_with_llm(provider, &request.model)
                .await
                .unwrap_or_default()
        } else {
            self.context
                .compact_with_llm(provider, &request.model)
                .await
                .unwrap_or_default()
        };
        if !compacted {
            compacted = self.context.compact_if_needed();
        }
        if compacted {
            run_context_compacted_hook(
                &request.project_root,
                &request.hooks,
                estimated_tokens,
                self.context.llm_compaction_threshold(),
            )?;
            // Emit the structured snapshot first so editors that wire
            // a `context overview` panel can render `files_read /
            // open_todos / untrusted_inputs / narrative` directly,
            // then a human-readable Thinking line so transcript-only
            // consumers still see "context compacted: X → Y".
            if let Some(snapshot) = self.context.last_compacted() {
                events(AgentRunEvent::ContextCompacted {
                    compacted: snapshot.clone(),
                });
            }
            let threshold = self.context.llm_compaction_threshold();
            let post_tokens = self.context.estimated_tokens();
            events(AgentRunEvent::Thinking {
                text: format!(
                    "context compacted: {estimated_tokens} tok → {post_tokens} tok (threshold {threshold})"
                ),
            });
        }
        events(AgentRunEvent::AssistantStarted {
            label: "assistant".to_string(),
        });
        let tool_definitions = self.cached_tool_definitions.clone();
        let system_prompt = system_prompt_for_role(self.state.mode, self.role).to_string();
        let mut messages = self.context.to_messages();
        // Vision routing (feature F2). When the turn carries images but the
        // active model is text-only, route to the `[vision] model` override if
        // one is configured and capable. Then reconcile image blocks with the
        // effective model: a vision model keeps them; a text-only model gets
        // OCR-extracted text (when an extractor is installed) or drops them to
        // the user turn's text placeholder.
        let has_images = messages.iter().any(|message| !message.images.is_empty());
        let effective_model = route_vision_model(
            &request.model,
            self.vision_model.as_deref(),
            self.vision_enabled,
            has_images,
        );
        enforce_vision_capability(
            &mut messages,
            &effective_model,
            self.vision_enabled,
            self.image_text_extractor.as_deref(),
        );
        // Emit the effective next-request footprint, not just the stored
        // context-manager estimate. Provider latency is driven by the full
        // assembled request: system prompt, messages, tool schemas and the
        // protocol overhead needed to serialize roles/tool-call ids.
        let window = self.context.model_context_window_tokens();
        let context_tokens = self.context.estimated_tokens() as u64;
        let request_context =
            estimate_request_context_tokens(Some(&system_prompt), &messages, &tool_definitions);
        events(AgentRunEvent::ContextUtilizationChanged {
            tokens_used: request_context.total_tokens,
            threshold: window as u64,
            context_tokens,
            message_tokens: request_context.message_tokens,
            system_tokens: request_context.system_tokens,
            tool_schema_tokens: request_context.tool_schema_tokens,
            overhead_tokens: request_context.overhead_tokens,
        });
        // Honour the provider's capability advertisement: a provider that
        // returns `supports_thinking() == false` (e.g. plain OpenAI Chat
        // Completions) must not receive the thinking flag, otherwise the
        // server would either ignore it silently (best case) or reject the
        // request as malformed (some forks). Mode-based intent still gates
        // this, so Execute-mode runs against a thinking-capable provider
        // continue to leave thinking off as before.
        let thinking = self.state.mode == ExecutionMode::Goal && provider.supports_thinking();
        let completion = stream_completion_with_chunks(
            provider,
            CompletionRequest {
                model: effective_model,
                system: Some(system_prompt),
                messages,
                max_tokens: Some(request.max_tokens),
                thinking,
                reasoning_effort: request.reasoning_effort,
                service_tier: request.service_tier,
                tools: tool_definitions,
                tool_choice: ToolChoice::Auto,
            },
            self.cancel.as_ref(),
            |chunk| {
                if !chunk.delta.is_empty() {
                    events(AgentRunEvent::AssistantDelta {
                        delta: chunk.delta.clone(),
                    });
                }
            },
        )
        .await?;
        events(AgentRunEvent::AssistantFinished {
            text: completion.text.clone(),
        });

        let first_tool_call = completion.tool_calls.first().cloned();
        if completion.tool_calls.len() > 1 {
            if self.can_batch_read_only_tool_calls(&completion.tool_calls) {
                self.context.append(ContextEntry::assistant_with_tool_calls(
                    completion.text.clone(),
                    completion.tool_calls.clone(),
                ));
                transition_phase(
                    &mut self.state,
                    AgentPhase::Executing,
                    "tools_started",
                    events,
                );
                let mut outputs = Vec::with_capacity(completion.tool_calls.len());
                let mut all_success = true;
                let names = completion
                    .tool_calls
                    .iter()
                    .map(|call| call.name.as_str())
                    .collect::<Vec<_>>()
                    .join(", ");
                for invocation in &completion.tool_calls {
                    let tool_call_id = invocation.id.clone();
                    let tool_parameters = tool_invocation_parameters(invocation);
                    events(AgentRunEvent::ToolStarted {
                        name: invocation.name.clone(),
                        parameters: tool_parameters.clone(),
                        risk_class: risk_class_label_for(&self.tools, &invocation.name),
                    });
                    let (tool_result, file_diff) = match self
                        .execute_tool_call_with_runtime(
                            ToolCall {
                                name: invocation.name.clone(),
                                parameters: tool_parameters.clone(),
                            },
                            request.project_root.clone(),
                            request.denied_paths.clone(),
                            request.hooks.clone(),
                            request.security.clone(),
                        )
                        .await
                    {
                        Ok(outcome) => outcome,
                        Err(err) => {
                            let failure_result =
                                peridot_common::ToolResult::failure(format!("tool failed: {err}"));
                            events(AgentRunEvent::ToolFinished {
                                name: invocation.name.clone(),
                                result: failure_result.clone(),
                            });
                            append_tool_result_to_context(
                                &mut self.context,
                                &request.project_root,
                                tool_call_id,
                                &invocation.name,
                                &tool_parameters,
                                &failure_result,
                            )?;
                            return Err(err);
                        }
                    };
                    if let Some(diff) = file_diff {
                        events(AgentRunEvent::FileDiff(diff));
                    }
                    events(AgentRunEvent::ToolFinished {
                        name: invocation.name.clone(),
                        result: tool_result.clone(),
                    });
                    emit_plan_updated_after_tool(
                        &invocation.name,
                        &tool_result,
                        &request.project_root,
                        events,
                    );
                    append_tool_result_to_context(
                        &mut self.context,
                        &request.project_root,
                        tool_call_id,
                        &invocation.name,
                        &tool_parameters,
                        &tool_result,
                    )?;
                    all_success &= tool_result.success;
                    outputs.push(serde_json::json!({
                        "name": invocation.name.clone(),
                        "success": tool_result.success,
                        "summary": tool_result.summary.clone(),
                        "output": tool_result.output.clone(),
                    }));
                }
                transition_phase(
                    &mut self.state,
                    AgentPhase::Verifying,
                    "tools_finished",
                    events,
                );
                let summary = format!(
                    "executed {} read-only tool calls: {names}",
                    completion.tool_calls.len()
                );
                return Ok(AgentTurnOutcome {
                    tool_name: "multi_tool".to_string(),
                    tool_result: ToolResult {
                        success: all_success,
                        summary,
                        output: serde_json::Value::Array(outputs),
                    },
                    usage: completion.usage,
                    done: false,
                });
            } else {
                events(AgentRunEvent::Thinking {
                    text: format!(
                        "received {} tool calls; executing the first because the batch contains a non-read-only tool",
                        completion.tool_calls.len()
                    ),
                });
            }
        }

        // No tool call → treat the assistant's text as a chat-style reply and finish
        // the turn synthetically through `agent_done`. Push the assistant text into
        // context so future turns see the reply, then execute `agent_done` for the
        // audit trail and phase transition. We deliberately do NOT emit
        // `ToolStarted`/`ToolFinished` events: the chat text is already on screen as
        // an `Assistant` transcript entry, and re-rendering it as
        // `❯ agent_done running` + `✔ agent_done <text>` produces a duplicated reply
        // in green. The outer `TurnEnded` / `Finished` events still fire from
        // `run_until_done_with_events`, so the loop stays observable without the
        // visual noise.
        let Some(invocation) = first_tool_call else {
            if !completion.text.trim().is_empty() {
                self.context.append(ContextEntry::trusted(
                    ContextSource::Assistant,
                    completion.text.clone(),
                ));
            }
            let summary = if completion.text.trim().is_empty() {
                "no response".to_string()
            } else {
                completion.text.clone()
            };
            let tool_call = ToolCall {
                name: "agent_done".to_string(),
                parameters: serde_json::json!({ "summary": summary }),
            };
            transition_phase(
                &mut self.state,
                AgentPhase::Executing,
                "agent_done_execute",
                events,
            );
            let (tool_result, _file_diff) = self
                .execute_tool_call_with_runtime(
                    tool_call,
                    request.project_root,
                    request.denied_paths,
                    request.hooks,
                    request.security,
                )
                .await?;
            transition_phase(
                &mut self.state,
                AgentPhase::Done,
                "agent_done_complete",
                events,
            );
            return Ok(AgentTurnOutcome {
                tool_name: "agent_done".to_string(),
                tool_result,
                usage: completion.usage,
                done: true,
            });
        };

        let tool_call_id = invocation.id.clone();
        // Native tool-call protocol path. Record only the tool call this harness
        // will execute. The loop deliberately enforces a single-tool-per-turn
        // invariant; replaying ignored parallel calls would make Responses-style
        // providers reject the next turn because those calls have no matching
        // `function_call_output`.
        self.context.append(ContextEntry::assistant_with_tool_calls(
            completion.text.clone(),
            vec![invocation.clone()],
        ));

        let tool_call = ToolCall {
            name: invocation.name.clone(),
            parameters: tool_invocation_parameters(&invocation),
        };
        let tool_name = tool_call.name.clone();
        let tool_parameters = tool_call.parameters.clone();
        transition_phase(
            &mut self.state,
            AgentPhase::Executing,
            "tool_started",
            events,
        );
        // When the model both streams a reply AND closes the turn with
        // `agent_done`, the `agent_done` summary almost always duplicates the
        // text the user just read (qwen does this consistently). Suppress the
        // tool UI events in that case so the transcript shows the reply once.
        // The tool still runs internally for audit / phase transition; we
        // simply don't surface the redundant `❯ agent_done` / `✔ agent_done
        // <summary>` lines. When the model used `agent_done` AS the response
        // channel (no preceding text), the events DO fire so the summary
        // reaches the user — that's the only signal they'd otherwise see.
        let suppress_done_ui = tool_name == "agent_done" && !completion.text.trim().is_empty();
        if !suppress_done_ui {
            events(AgentRunEvent::ToolStarted {
                name: tool_name.clone(),
                parameters: tool_parameters.clone(),
                risk_class: risk_class_label_for(&self.tools, &tool_name),
            });
        }
        let pending_for_resume = ToolCall {
            name: tool_name.clone(),
            parameters: tool_parameters.clone(),
        };
        let (tool_result, file_diff) = match self
            .execute_tool_call_with_runtime(
                ToolCall {
                    name: tool_name.clone(),
                    parameters: tool_parameters.clone(),
                },
                request.project_root.clone(),
                request.denied_paths,
                request.hooks,
                request.security,
            )
            .await
        {
            Ok(outcome) => outcome,
            Err(err) => {
                if let PeriError::PermissionDenied(reason) = &err
                    && approval_required_error(&err)
                {
                    // Persist the exact pending tool call so the next
                    // session can resume from this point instead of
                    // restarting the whole task. Best-effort: a write
                    // failure just degrades to the legacy restart UX.
                    if let Some(path) = self.pending_resume_path.as_ref()
                        && let Ok(bytes) = serde_json::to_vec(&pending_for_resume)
                    {
                        if let Some(parent) = path.parent() {
                            let _ = std::fs::create_dir_all(parent);
                        }
                        let _ = std::fs::write(path, &bytes);
                    }
                    let risk_class = risk_class_label_for(&self.tools, &tool_name);
                    events(AgentRunEvent::ApprovalRequested {
                        tool_name,
                        reason: reason.clone(),
                        parameters: tool_parameters,
                        risk_class,
                    });
                    return Err(err);
                }
                // The tool_call entry was already appended above.
                // Bailing here without a matching
                // function_call_output left the conversation
                // malformed: the very next request to Responses-
                // style providers (OpenAI Codex) was rejected with
                // `400 No tool output found for function call
                // <id>`. Synthesise a failed ToolResult and append
                // it as the paired output BEFORE bubbling the
                // error, so the recovery layer above can still add
                // its plan-reminder while the conversation stays
                // well-formed.
                let failure_result =
                    peridot_common::ToolResult::failure(format!("tool failed: {err}"));
                if !suppress_done_ui {
                    events(AgentRunEvent::ToolFinished {
                        name: tool_name.clone(),
                        result: failure_result.clone(),
                    });
                }
                let (observation_result, evidence_ref) = tool_result_for_context_observation(
                    &request.project_root,
                    &tool_name,
                    &tool_parameters,
                    &failure_result,
                );
                let observation = serde_json::to_string(&observation_result).map_err(|serr| {
                    PeriError::Parse(format!("failed to serialize tool failure: {serr}"))
                })?;
                let mut entry = ContextEntry::trusted(ContextSource::Tool, observation)
                    .with_tool_call_id(tool_call_id);
                if let Some(evidence) = evidence_ref {
                    entry = entry.with_evidence_ref(evidence);
                }
                self.context.append(entry);
                return Err(err);
            }
        };
        // Bind tool_call back for downstream code that reads from it.
        let tool_call = pending_for_resume;
        // Hold a reference so the compiler doesn't complain about the
        // unused binding when only one branch consumes it.
        let _ = &tool_call;
        if !suppress_done_ui {
            if let Some(diff) = file_diff {
                events(AgentRunEvent::FileDiff(diff));
            }
            events(AgentRunEvent::ToolFinished {
                name: tool_name.clone(),
                result: tool_result.clone(),
            });
            emit_plan_updated_after_tool(&tool_name, &tool_result, &request.project_root, events);
        }
        // The tool result is a tool-role message paired with the assistant's
        // `tool_call_id`. We bypass `append_observation` because it stamps every
        // entry as untrusted and offload-eligible; here we want the provider to
        // receive it through the native tool message channel instead, so the
        // model sees its own past action and result without re-running them.
        let (observation_result, evidence_ref) = tool_result_for_context_observation(
            &request.project_root,
            &tool_name,
            &tool_parameters,
            &tool_result,
        );
        let observation = serde_json::to_string(&observation_result)
            .map_err(|err| PeriError::Parse(format!("failed to serialize tool result: {err}")))?;
        let mut entry =
            ContextEntry::trusted(ContextSource::Tool, observation).with_tool_call_id(tool_call_id);
        if let Some(evidence) = evidence_ref {
            entry = entry.with_evidence_ref(evidence);
        }
        self.context.append(entry);

        if tool_name == "agent_done" && tool_result.success {
            transition_phase(
                &mut self.state,
                AgentPhase::Done,
                "agent_done_result",
                events,
            );
        } else {
            transition_phase(
                &mut self.state,
                AgentPhase::Verifying,
                "tool_finished",
                events,
            );
        }

        Ok(AgentTurnOutcome {
            tool_name,
            tool_result,
            usage: completion.usage,
            done: self.state.phase == AgentPhase::Done,
        })
    }

    /// Runs model/tool turns until done or guardrail exhaustion.
    pub async fn run_until_done(
        &mut self,
        provider: &dyn LlmProvider,
        request: AgentRunRequest,
    ) -> PeriResult<AgentRunSummary> {
        self.run_until_done_with_events(provider, request, |_| {})
            .await
    }

    /// Runs model/tool turns until done while emitting user-interface events.
    pub async fn run_until_done_with_events<F>(
        &mut self,
        provider: &dyn LlmProvider,
        request: AgentRunRequest,
        mut events: F,
    ) -> PeriResult<AgentRunSummary>
    where
        F: FnMut(AgentRunEvent) + Send,
    {
        events(AgentRunEvent::RunStarted {
            task: request.task.clone(),
        });
        let initial_grade_diff = self
            .auto_grade_on_done
            .then(|| self.current_grader_diff(&request.project_root));
        // Stamp the run's wall-clock start so the final `AgentRunSummary` can
        // report how long the task took. Using `Instant` instead of
        // `SystemTime` keeps the measurement monotonic across NTP jumps.
        let started_at = std::time::Instant::now();
        // Pending-resume check: when the previous run halted on
        // ApprovalRequired we persisted the pending tool call to a
        // sidecar. If that file is present, execute the call directly
        // against the new (presumably relaxed) security posture so
        // the run picks up exactly where it stopped instead of asking
        // the model to redo everything that led up to the gated step.
        // PR-B.4: pre-loop setup steps extracted to dedicated methods.
        // Their behaviour is unchanged from when they were inline; this
        // is purely an organisational move so `run_until_done_with_events`
        // gets closer to the "30-line orchestrator" goal. A future change
        // will lift each helper into a `LoopPolicy::pre_turn` impl once
        // `ToolDispatcher` lands.
        // PendingResumePolicy now owns pending-tool resume; the inline
        // helper is kept on `HarnessAgent` as a backward-compat alias
        // for in-test direct calls, but the production path runs the
        // policy via `dispatch_pre_turn` inside the loop body.
        // CodebaseSurveyPrefetchPolicy now owns prefetch; the inline
        // helper definition stays as a backward-compat shim until any
        // direct test callers move over.
        let mut outcomes = Vec::new();
        let mut total_usage = Usage::default();
        // Composable run-loop policies, dispatched per phase boundary.
        // Each one owns its own state (e.g. budget warning's one-shot
        // sent flag, stuck detector's history). Inline blocks below
        // will be progressively migrated into this ordered list as the
        // loop_policy/ module grows.
        // The auto-grade policy needs a closure that produces the
        // current worktree diff on demand. Capture the existing
        // grader-diff provider (test injection or production
        // `collect_git_diff`) and the project root by clone so the
        // closure outlives the run.
        let grader_diff_fn: crate::loop_policy::DiffProvider = {
            let grader_diff_provider = self.grader_diff_provider.clone();
            let project_root = request.project_root.clone();
            std::sync::Arc::new(move || match grader_diff_provider.as_ref() {
                Some(custom) => custom(&project_root),
                None => collect_git_diff(&project_root),
            })
        };
        let mut policies: Vec<Box<dyn crate::loop_policy::LoopPolicy>> = vec![
            // Recovery first — it observes on_turn_error and resets its
            // attempt counters on post_turn before any other policy runs.
            Box::new(crate::loop_policy::ErrorRecoveryPolicy::new()),
            // pre_turn: resume any tool that was halted by approval.
            // Idempotent — fires only on turn_index == 0 when a
            // sidecar exists.
            Box::new(crate::loop_policy::PendingResumePolicy::new()),
            // pre_turn: codebase-survey subagent on Goal-mode first
            // turn so the model gets structured orientation.
            Box::new(crate::loop_policy::CodebaseSurveyPrefetchPolicy::new()),
            Box::new(crate::loop_policy::SubAgentReviewPolicy::new()),
            Box::new(crate::loop_policy::BudgetWarningPolicy::new()),
            Box::new(crate::loop_policy::StuckDetectorPolicy::new()),
            // post_turn: run verify_build after every successful
            // mutating tool when the harness opts in.
            Box::new(crate::loop_policy::AutoVerifyAfterMutationPolicy::new(
                self.auto_verify_after_mutation,
            )),
            // post_turn: track verify_* failures, inject a "fix this
            // first" directive on retries, and abort with
            // StopReason::Interrupted when the signature repeats
            // beyond `fix_cap`.
            Box::new(crate::loop_policy::AutoFixLoopPolicy::new(
                self.auto_fix_cap,
            )),
            // on_done policies — ordered so cheaper / deterministic
            // checks run first, then LLM gates last:
            //   Preflight  → mechanical (file lists, todo state)
            //   GoalChecker → LLM call, Goal-mode only
            //   AutoGrade  → LLM call, when enabled, with diff
            //
            // Preflight stays OFF by default. The "verify after
            // mutation" gate is context-aware (it accepts a successful
            // `[auto-verify] verify_build passed` PlanReminder as
            // satisfying the check), but auto-coupling it to
            // `auto_verify_after_mutation` would block runs whose
            // verify_build *failed* — the existing helper still appends
            // a marker, just with FAILED, and that's a separate decision
            // from "should the loop refuse to terminate."
            //
            // Operators opt into the gate explicitly via a future flag;
            // the test suite uses `with_verify_after_mutation()` directly
            // when it wants to exercise the path.
            Box::new(crate::loop_policy::PreflightPolicy::new()),
            Box::new(crate::loop_policy::GoalCheckerPolicy::new()),
            Box::new(crate::loop_policy::AutoGradePolicy::new(
                self.auto_grade_on_done,
                grader_diff_fn,
                initial_grade_diff.clone(),
            )),
        ];
        // Auto-fix loop state. Tracks the current failing verifier by
        // a compact signature so the model can tell "same failure,
        // same attempted fix" apart from a new failure uncovered by
        // progress.
        // verify_failure_state + fix_cap moved into AutoFixLoopPolicy.
        for turn_index in 0..request.max_turns {
            // Drain any inter-session messages parked for this session.
            // Each entry becomes a trusted `[peer message from <id>]`
            // PlanReminder so the model sees coordination signals (e.g.
            // a parent's "stop", a child's "tests passed") on the next
            // call without an explicit tool round-trip.
            if let (Some(bus), Some(session_id)) =
                (self.message_bus.clone(), self.session_id.clone())
            {
                let messages = bus.drain_inbox(&session_id).await;
                for entry in messages {
                    self.context.append(ContextEntry::trusted(
                        ContextSource::PlanReminder,
                        format!("[peer message from {}] {}", entry.from, entry.body),
                    ));
                }
            }
            if self
                .cancel
                .as_ref()
                .map(|token| token.is_cancelled())
                .unwrap_or(false)
            {
                events(AgentRunEvent::Interrupted {
                    stage: "turn_start".to_string(),
                });
                return Ok(finalize_run(
                    outcomes,
                    total_usage,
                    StopReason::Interrupted,
                    started_at,
                    &mut events,
                ));
            }
            self.refresh_agents_md(&mut events);
            // pre_turn policy dispatch. PendingResumePolicy fires here
            // on turn_index == 0 to replay any sidecar-recorded tool
            // call. Other pre_turn policies (codebase-survey prefetch)
            // will land here as they're extracted.
            {
                let pre_decision = {
                    let tool_dispatcher = self.build_tool_dispatcher();
                    let subagent_runner = self.subagent_runner.clone();
                    let mut cx = crate::loop_policy::PolicyCx {
                        state: &mut self.state,
                        context: &mut self.context,
                        events: &mut events,
                        usage: &mut total_usage,
                        request: &request,
                        turn_index: turn_index as usize,
                        project_root: &request.project_root,
                        hooks: &request.hooks,
                        security: &request.security,
                        provider,
                        tool_dispatcher: &tool_dispatcher,
                        subagent_runner,
                        pending_resume_path: self.pending_resume_path.as_deref(),
                    };
                    crate::loop_policy::dispatch_pre_turn(&mut policies, &mut cx).await?
                };
                match pre_decision {
                    crate::loop_policy::Decision::Continue => {}
                    crate::loop_policy::Decision::SkipTurn => continue,
                    crate::loop_policy::Decision::Retry => continue,
                    crate::loop_policy::Decision::Stop(reason, message) => {
                        if let Some(msg) = message {
                            events(AgentRunEvent::Recovery { message: msg });
                        }
                        return Ok(finalize_run(
                            outcomes,
                            total_usage,
                            reason,
                            started_at,
                            &mut events,
                        ));
                    }
                }
            }
            events(AgentRunEvent::TurnStarted { turn_index });
            let outcome = match self
                .run_turn_with_events(
                    provider,
                    AgentTurnRequest {
                        user_input: (turn_index == 0).then(|| request.task.clone()),
                        model: request.model.clone(),
                        max_tokens: request.max_tokens,
                        reasoning_effort: request.reasoning_effort,
                        service_tier: request.service_tier.clone(),
                        project_root: request.project_root.clone(),
                        denied_paths: request.denied_paths.clone(),
                        hooks: request.hooks.clone(),
                        security: request.security.clone(),
                    },
                    &mut events,
                )
                .await
            {
                Ok(outcome) => outcome,
                Err(err) => {
                    // Two early-exit conditions the policy chain can't handle
                    // (they need to terminate the run immediately, not recover):
                    //   - approval-required: a tool is gated, paused for user.
                    //   - cancellation token tripped: external interrupt.
                    if approval_required_error(&err) {
                        return Ok(finalize_run(
                            outcomes,
                            total_usage,
                            StopReason::ApprovalRequired,
                            started_at,
                            &mut events,
                        ));
                    }
                    if self
                        .cancel
                        .as_ref()
                        .map(|token| token.is_cancelled())
                        .unwrap_or(false)
                    {
                        events(AgentRunEvent::Interrupted {
                            stage: "turn_error".to_string(),
                        });
                        return Ok(finalize_run(
                            outcomes,
                            total_usage,
                            StopReason::Interrupted,
                            started_at,
                            &mut events,
                        ));
                    }
                    // Hand the error to the policy chain. ErrorRecoveryPolicy
                    // owns: phase transition, hook firing, classify_error
                    // branching, attempt counter, signature tracking,
                    // category-specific backoff, plan-reminder injection,
                    // and the abort-after-N-attempts decision.
                    let action = self
                        .dispatch_turn_error(
                            &err,
                            &request,
                            &mut total_usage,
                            turn_index,
                            provider,
                            &mut policies,
                            &mut events,
                        )
                        .await?;
                    match action {
                        TurnErrorAction::Continue => {
                            // No policy claimed the error → sleep
                            // briefly to avoid hammering the same
                            // failure, then retry.
                            sleep_before_error_recovery_retry().await;
                            continue;
                        }
                        TurnErrorAction::Retry | TurnErrorAction::Skip => continue,
                        TurnErrorAction::Stop { reason, message } => {
                            if let Some(msg) = message {
                                run_recovery_event_hook(
                                    &request.project_root,
                                    &request.hooks,
                                    "recovery_abort",
                                    &msg,
                                )?;
                                events(AgentRunEvent::Recovery { message: msg });
                            }
                            return Ok(finalize_run(
                                outcomes,
                                total_usage,
                                reason,
                                started_at,
                                &mut events,
                            ));
                        }
                    }
                }
            };
            let turn_success = outcome.tool_result.success;
            // AutoFixLoopPolicy and AutoVerifyAfterMutationPolicy now
            // own this — they run in the post_turn dispatch block
            // below. AutoFix's circuit-breaker abort flows through
            // Decision::Stop in the same dispatch.
            accumulate_usage(&mut total_usage, outcome.usage);
            // AutoVerifyAfterMutationPolicy now owns this — it runs in
            // the post_turn dispatch above.
            events(AgentRunEvent::UsageUpdated { usage: total_usage });
            events(AgentRunEvent::TurnEnded {
                turn_index,
                success: turn_success,
            });
            if let Some(path) = self.context_snapshot_path.as_ref() {
                snapshot_context_to_disk(path, &self.context, &mut events);
            }
            // Post-turn policy dispatch. Sub-agent review + budget warning
            // (and, in later PRs, stuck detector, auto-verify, etc.) live
            // here. PolicyCx is constructed in a tight scope so its
            // mutable borrows release before the rest of the iteration
            // resumes its own use of self.state/self.context/events.
            {
                let policy_decision = {
                    let tool_dispatcher = self.build_tool_dispatcher();
                    let subagent_runner = self.subagent_runner.clone();
                    let mut cx = crate::loop_policy::PolicyCx {
                        state: &mut self.state,
                        context: &mut self.context,
                        events: &mut events,
                        usage: &mut total_usage,
                        request: &request,
                        turn_index: turn_index as usize,
                        project_root: &request.project_root,
                        hooks: &request.hooks,
                        security: &request.security,
                        provider,
                        tool_dispatcher: &tool_dispatcher,
                        subagent_runner,
                        pending_resume_path: self.pending_resume_path.as_deref(),
                    };
                    crate::loop_policy::dispatch_post_turn(&mut policies, &mut cx, &outcome).await?
                };
                match policy_decision {
                    crate::loop_policy::Decision::Continue => {}
                    crate::loop_policy::Decision::SkipTurn => {
                        outcomes.push(outcome);
                        continue;
                    }
                    crate::loop_policy::Decision::Retry => {
                        // post_turn cannot meaningfully request a retry —
                        // the turn already produced an outcome. Treat
                        // identically to Continue.
                    }
                    crate::loop_policy::Decision::Stop(reason, message) => {
                        outcomes.push(outcome);
                        if let Some(msg) = message {
                            events(AgentRunEvent::Recovery { message: msg });
                        }
                        return Ok(finalize_run(
                            outcomes,
                            total_usage,
                            reason,
                            started_at,
                            &mut events,
                        ));
                    }
                }
            }
            let done = outcome.done;
            outcomes.push(outcome);
            if done {
                // Deterministic preflight gate. Runs BEFORE the LLM
                // goal checker / auto-grader so cheap, mechanical
                // failure modes (mutation without verify, lingering
                // pending tools) are caught without an LLM round-trip.
                // PreflightPolicy returns SkipTurn + appends a plan
                // reminder when the model declared done prematurely.
                {
                    let preflight_decision = {
                        let tool_dispatcher = self.build_tool_dispatcher();
                        let subagent_runner = self.subagent_runner.clone();
                        let mut cx = crate::loop_policy::PolicyCx {
                            state: &mut self.state,
                            context: &mut self.context,
                            events: &mut events,
                            usage: &mut total_usage,
                            request: &request,
                            turn_index: turn_index as usize,
                            project_root: &request.project_root,
                            hooks: &request.hooks,
                            security: &request.security,
                            provider,
                            tool_dispatcher: &tool_dispatcher,
                            subagent_runner,
                            pending_resume_path: self.pending_resume_path.as_deref(),
                        };
                        crate::loop_policy::dispatch_on_done(&mut policies, &mut cx, &outcomes)
                            .await?
                    };
                    match preflight_decision {
                        crate::loop_policy::Decision::Continue => {}
                        crate::loop_policy::Decision::SkipTurn => continue,
                        crate::loop_policy::Decision::Retry => continue,
                        crate::loop_policy::Decision::Stop(reason, message) => {
                            if let Some(msg) = message {
                                events(AgentRunEvent::Recovery { message: msg });
                            }
                            return Ok(finalize_run(
                                outcomes,
                                total_usage,
                                reason,
                                started_at,
                                &mut events,
                            ));
                        }
                    }
                }
                // GoalCheckerPolicy and AutoGradePolicy ran above in
                // `dispatch_on_done`. If they wanted to veto the done
                // state they returned SkipTurn → already handled.
                // Reaching this point means done is final.
                return Ok(finalize_run(
                    outcomes,
                    total_usage,
                    StopReason::Done,
                    started_at,
                    &mut events,
                ));
            }
            if request.budget_usd > 0.0 && total_usage.estimated_cost_usd >= request.budget_usd {
                transition_phase(
                    &mut self.state,
                    AgentPhase::Recovering,
                    "budget_exceeded",
                    &mut events,
                );
                self.context.append(ContextEntry::trusted(
                    ContextSource::PlanReminder,
                    budget_exceeded_message(total_usage.estimated_cost_usd, request.budget_usd),
                ));
                return Ok(finalize_run(
                    outcomes,
                    total_usage,
                    StopReason::Budget,
                    started_at,
                    &mut events,
                ));
            }
        }

        Ok(finalize_run(
            outcomes,
            total_usage,
            StopReason::MaxTurns,
            started_at,
            &mut events,
        ))
    }
}

#[cfg(not(test))]
async fn sleep_before_error_recovery_retry() {
    tokio::time::sleep(ERROR_RECOVERY_RETRY_DELAY).await;
}

#[cfg(test)]
async fn sleep_before_error_recovery_retry() {
    let _ = ERROR_RECOVERY_RETRY_DELAY;
}

/// Look up a tool's risk-class label by name, suitable for the
/// `AgentRunEvent::ToolStarted::risk_class` field. Returns `None` when
/// the registry has no such tool (e.g. for synthetic delegate events
/// emitted before a real tool dispatch).
pub(crate) fn risk_class_label_for(tools: &ToolRegistry, name: &str) -> Option<String> {
    tools
        .get(name)
        .map(|tool| tool.risk_class().label().to_string())
}

/// Snapshot of [`HarnessAgent`]'s tool-execution dependencies, cloned
/// fresh at the start of each loop iteration so policies can dispatch
/// tool calls without re-borrowing `&mut HarnessAgent`.
///
/// All fields are owned (or `Arc`-shared) so the dispatcher can be
/// passed by `&` into `PolicyCx` while `&mut self.state` /
/// `&mut self.context` borrows are live. Construction takes a single
/// `&self` snapshot — see [`HarnessAgent::build_tool_dispatcher`].
pub struct HarnessToolDispatcher {
    tools: ToolRegistry,
    cancel: Option<peridot_common::CancelToken>,
    subagent_runner: Option<std::sync::Arc<dyn peridot_agents::SubAgent>>,
    ask_user_port: Option<std::sync::Arc<dyn AskUserPort>>,
    message_bus: Option<std::sync::Arc<dyn AgentMessageBus>>,
    /// Parent-context mission packet captured at construction time.
    /// Slightly stale across the loop iteration but acceptable — the
    /// packet is a high-level summary, not a per-call diff.
    mission_packet: String,
}

#[async_trait::async_trait]
impl crate::loop_policy::ToolDispatcher for HarnessToolDispatcher {
    async fn execute(
        &self,
        call: ToolCall,
        mode: ExecutionMode,
        phase: peridot_common::AgentPhase,
        permission: peridot_common::PermissionMode,
        project_root: PathBuf,
        denied_paths: Vec<PathBuf>,
        hooks: peridot_common::HooksConfig,
        security: SecurityConfig,
    ) -> PeriResult<(ToolResult, Option<FileDiffPayload>)> {
        // Mirrors `HarnessAgent::execute_tool_call_with_runtime`. State
        // values are parameters (not `&self.state` access) so this can
        // run while the harness's state is mutably borrowed elsewhere.
        let tool = self
            .tools
            .get(&call.name)
            .ok_or_else(|| PeriError::Tool(format!("unknown tool: {}", call.name)))?;
        crate::permissions::ensure_tool_allowed(mode, phase, tool.group(), &call.name)?;
        tool.validate_params(&call.parameters)?;
        if tool.requires_confirmation(permission)
            && !tool_call_has_confirmation_grant(&call, &security)
        {
            return Err(PeriError::PermissionDenied(format!(
                "{} requires explicit user approval",
                call.name
            )));
        }
        let mut ctx = ToolContext::new(project_root.clone(), permission)
            .with_denied_paths(denied_paths)
            .with_hooks(hooks)
            .with_security(security);
        if let Some(token) = self.cancel.clone() {
            ctx = ctx.with_cancel(token);
        }
        if let Some(runner) = self.subagent_runner.clone() {
            ctx = ctx.with_subagent_runner(runner);
        }
        if let Some(port) = self.ask_user_port.clone() {
            ctx = ctx.with_ask_user_port(port);
        }
        if let Some(bus) = self.message_bus.clone() {
            ctx = ctx.with_message_bus(bus);
        }
        if !self.mission_packet.trim().is_empty() {
            ctx = ctx.with_parent_context_packet(self.mission_packet.clone());
        }
        let runner = HookRunner::new(&project_root, ctx.hooks.clone());
        let mut variables = tool_hook_variables(&call.name, &call.parameters);
        variables.insert(
            "project_root".to_string(),
            project_root.display().to_string(),
        );
        variables.insert("workspace".to_string(), project_root.display().to_string());
        variables.insert("mode".to_string(), mode.to_string());
        variables.insert("permission".to_string(), permission.to_string());
        runner.run_tool_hooks(&format!("pre:{}", call.name), &variables)?;
        let tool_name = call.name.clone();
        let params = call.parameters.clone();
        let checkpoint = if tool.modifies_state() {
            write_file_checkpoint(&project_root, &tool_name, &params)
                .ok()
                .flatten()
        } else {
            None
        };
        let checkpoint_id = checkpoint.as_ref().map(|cp| cp.id.clone());
        let result = tool.execute(call.parameters, &ctx).await?;
        let file_diff = checkpoint.as_ref().and_then(|cp| {
            if !result.success {
                return None;
            }
            let after = std::fs::read_to_string(&cp.absolute_path).ok()?;
            Some(FileDiffPayload {
                tool_name: tool_name.clone(),
                path: cp.relative_path.clone(),
                before: cp.previous_content.clone(),
                after,
            })
        });
        let _ = append_audit_event(
            &project_root,
            &AuditEvent::tool_call(
                &tool_name,
                result.success,
                &result.summary,
                serde_json::json!({
                    "params": params,
                    "phase": phase,
                    "mode": mode,
                    "permission": permission,
                    "checkpoint_id": checkpoint_id
                }),
            ),
        );
        variables.insert(
            "result_json".to_string(),
            serde_json::to_string(&result).map_err(|err| {
                PeriError::Parse(format!("failed to serialize hook result: {err}"))
            })?,
        );
        runner.run_tool_hooks(&format!("post:{}", call.name), &variables)?;
        Ok((result, file_diff))
    }
}

/// Centralized `AgentState::phase` transition.
///
/// Every change of `state.phase` flows through here so observers
/// (TUI, daemon, VS Code) receive a single structured event per
/// transition instead of inferring phase from other signals. The
/// `reason` is a short, stable label (e.g. `"tool_started"`,
/// `"recovery_abort"`) — not user-facing prose. No-op when the
/// requested phase equals the current one, which keeps the event
/// stream free of spurious self-transitions.
///
/// This is a free function (not a `&mut self` method on
/// `HarnessAgent`) so it can be called from inside methods that
/// already hold `&mut self` plus a separately-borrowed event sink
/// without tripping the borrow checker.
pub(crate) fn transition_phase<F: FnMut(AgentRunEvent) + ?Sized>(
    state: &mut AgentState,
    next: AgentPhase,
    reason: &'static str,
    events: &mut F,
) {
    let from = state.phase;
    if from == next {
        return;
    }
    state.phase = next;
    events(AgentRunEvent::PhaseChanged {
        from,
        to: next,
        reason: reason.to_string(),
    });
}

impl HarnessAgent {
    /// Run the `on_turn_error` policy chain for `err` and flatten the
    /// resulting [`Decision`] into a [`TurnErrorAction`] the driver can
    /// match on. Centralises the `PolicyCx` construction so the
    /// driver's error branch stays short — see `run_until_done_with_events`.
    ///
    /// The argument list mirrors `PolicyCx`'s fields; collapsing them
    /// into a struct would just shuffle the count from here into the
    /// caller, so the lint is silenced rather than masked.
    #[allow(clippy::too_many_arguments)]
    async fn dispatch_turn_error<F>(
        &mut self,
        err: &peridot_common::PeriError,
        request: &AgentRunRequest,
        total_usage: &mut Usage,
        turn_index: u32,
        provider: &dyn LlmProvider,
        policies: &mut [Box<dyn crate::loop_policy::LoopPolicy>],
        events: &mut F,
    ) -> PeriResult<TurnErrorAction>
    where
        F: FnMut(AgentRunEvent) + Send,
    {
        let tool_dispatcher = self.build_tool_dispatcher();
        let subagent_runner = self.subagent_runner.clone();
        let pending_resume_path = self.pending_resume_path.as_deref();
        let decision = {
            let mut cx = crate::loop_policy::PolicyCx {
                state: &mut self.state,
                context: &mut self.context,
                events,
                usage: total_usage,
                request,
                turn_index: turn_index as usize,
                project_root: &request.project_root,
                hooks: &request.hooks,
                security: &request.security,
                provider,
                tool_dispatcher: &tool_dispatcher,
                subagent_runner,
                pending_resume_path,
            };
            crate::loop_policy::dispatch_on_turn_error(policies, &mut cx, err).await?
        };
        Ok(match decision {
            crate::loop_policy::Decision::Continue => TurnErrorAction::Continue,
            crate::loop_policy::Decision::Retry => TurnErrorAction::Retry,
            crate::loop_policy::Decision::SkipTurn => TurnErrorAction::Skip,
            crate::loop_policy::Decision::Stop(reason, message) => {
                TurnErrorAction::Stop { reason, message }
            }
        })
    }

    /// Snapshot the dependencies needed for tool execution into an
    /// owned [`HarnessToolDispatcher`]. Called once per loop iteration
    /// just before constructing `PolicyCx`, so the dispatcher's Arcs +
    /// mission packet reflect the current turn's view of the world
    /// without holding a `&self` borrow across the policy chain.
    pub(crate) fn build_tool_dispatcher(&self) -> HarnessToolDispatcher {
        HarnessToolDispatcher {
            tools: self.tools.clone(),
            cancel: self.cancel.clone(),
            subagent_runner: self.subagent_runner.clone(),
            ask_user_port: self.ask_user_port.clone(),
            message_bus: self.message_bus.clone(),
            mission_packet: self.context.subagent_mission_packet(5_000),
        }
    }

    fn current_grader_diff(&self, project_root: &std::path::Path) -> String {
        self.grader_diff_provider
            .as_ref()
            .map(|f| f(project_root))
            .unwrap_or_else(|| collect_git_diff(project_root))
    }

    /// Compares the configured AGENTS.md path's `(modified_unix, len)`
    /// fingerprint against the last seen one and, when it changed, re-reads
    /// the file, injects its contents into context as a `PlanReminder`
    /// entry, and emits `AgentRunEvent::AgentsMdLoaded` with the rule count
    /// and origin path. The first call after `set_agents_md_path` always
    /// fires the inject because `agents_md_signature` starts as `None`.
    fn refresh_agents_md<F>(&mut self, events: &mut F)
    where
        F: FnMut(AgentRunEvent),
    {
        let Some(path) = self.agents_md_path.clone() else {
            return;
        };
        let Ok(meta) = std::fs::metadata(&path) else {
            return;
        };
        let modified = meta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let len = meta.len();
        let signature = (modified, len);
        if self.agents_md_signature == Some(signature) {
            return;
        }
        let Ok(content) = std::fs::read_to_string(&path) else {
            return;
        };
        let rule_count = content
            .lines()
            .filter(|line| !line.trim().is_empty())
            .count() as u32;
        self.context.append(ContextEntry::trusted(
            ContextSource::PlanReminder,
            format!(
                "AGENTS.md reloaded ({rule_count} lines) from {}:\n{content}",
                path.display()
            ),
        ));
        events(AgentRunEvent::AgentsMdLoaded {
            rule_count,
            paths: vec![path.display().to_string()],
        });
        self.agents_md_signature = Some(signature);
    }
}

/// Reads + deletes the pending-resume sidecar. Returns `Some` only
/// when the file exists, parses as a `ToolCall`, and was successfully
/// removed afterward. Any I/O or parse failure returns `None` so the
/// caller silently falls back to the legacy restart-from-scratch flow.
pub(crate) fn take_pending_resume(path: Option<&PathBuf>) -> Option<ToolCall> {
    let path = path?;
    if !path.exists() {
        return None;
    }
    let bytes = std::fs::read(path).ok()?;
    let call: ToolCall = serde_json::from_slice(&bytes).ok()?;
    // Best-effort delete: if the remove fails we still proceed —
    // worst case the next session re-applies the same tool call.
    let _ = std::fs::remove_file(path);
    Some(call)
}

#[derive(serde::Deserialize)]
struct TodoPlanFile {
    steps: Vec<TodoPlanStep>,
}

#[derive(serde::Deserialize)]
struct TodoPlanStep {
    text: String,
    status: String,
}

pub(crate) fn emit_plan_updated_after_tool<F>(
    tool_name: &str,
    result: &ToolResult,
    project_root: &Path,
    events: &mut F,
) where
    F: FnMut(AgentRunEvent) + ?Sized,
{
    if !result.success || !matches!(tool_name, "plan_create" | "plan_update") {
        return;
    }
    let Some((steps, current)) = read_todo_plan_updates(project_root) else {
        return;
    };
    events(AgentRunEvent::PlanUpdated { steps, current });
}

fn read_todo_plan_updates(project_root: &Path) -> Option<(Vec<PlanStepUpdate>, Option<u32>)> {
    let content = std::fs::read_to_string(project_root.join("todo.json")).ok()?;
    let plan: TodoPlanFile = serde_json::from_str(&content).ok()?;
    let mut current = None;
    let steps = plan
        .steps
        .into_iter()
        .enumerate()
        .map(|(index, step)| {
            let done = matches!(step.status.as_str(), "done" | "completed");
            if !done && current.is_none() {
                current = Some(index as u32);
            }
            PlanStepUpdate {
                label: step.text,
                done,
            }
        })
        .collect();
    Some((steps, current))
}

pub(crate) fn append_pending_resume_observation(
    context: &mut ContextManager,
    tool_name: &str,
    result: &ToolResult,
) -> PeriResult<()> {
    let observation = serde_json::to_string(result)
        .map_err(|err| PeriError::Parse(format!("failed to serialize tool result: {err}")))?;
    if let Some(tool_call_id) = latest_unanswered_tool_call_id(context.entries()) {
        context.append(
            ContextEntry::trusted(ContextSource::Tool, observation).with_tool_call_id(tool_call_id),
        );
        return Ok(());
    }
    context.append(ContextEntry::trusted(
        ContextSource::PlanReminder,
        format!(
            "[resume] Operator approved {tool_name}. Result: {}",
            result.summary
        ),
    ));
    Ok(())
}

fn latest_unanswered_tool_call_id(entries: &[ContextEntry]) -> Option<String> {
    let mut answered = std::collections::HashSet::new();
    for entry in entries.iter().rev() {
        if let Some(tool_call_id) = entry.tool_call_id.as_ref() {
            answered.insert(tool_call_id.clone());
            continue;
        }
        if entry.source == ContextSource::Assistant && !entry.tool_calls.is_empty() {
            return entry
                .tool_calls
                .iter()
                .find(|call| !answered.contains(&call.id))
                .map(|call| call.id.clone());
        }
    }
    None
}

const TOOL_OBSERVATION_INLINE_LIMIT: usize = 12_000;

fn estimate_request_context_tokens(
    system: Option<&str>,
    messages: &[LlmMessage],
    tools: &[ToolDefinition],
) -> RequestContextEstimate {
    let system_tokens = system.map(estimate_tokens_for_text).unwrap_or_default() as u64;
    let mut message_tokens = 0_u64;
    let mut overhead_tokens = 8_u64;
    for message in messages {
        message_tokens += estimate_tokens_for_text(&message.content) as u64;
        overhead_tokens += 6;
        overhead_tokens += message
            .tool_call_id
            .as_ref()
            .map(|id| id.len().div_ceil(4) + 4)
            .unwrap_or(0) as u64;
        if !message.tool_calls.is_empty() {
            overhead_tokens += 8 * message.tool_calls.len() as u64;
            for call in &message.tool_calls {
                message_tokens += estimate_tokens_for_text(&call.name) as u64;
                message_tokens += estimate_tokens_for_text(&call.arguments.to_string()) as u64;
                overhead_tokens += estimate_tokens_for_text(&call.id) as u64 + 4;
            }
        }
    }
    let tool_schema_tokens = if tools.is_empty() {
        0
    } else {
        serde_json::to_string(tools)
            .map(|json| estimate_tokens_for_text(&json) as u64)
            .unwrap_or_default()
    };
    if !tools.is_empty() {
        overhead_tokens += 10 * tools.len() as u64;
    }
    RequestContextEstimate {
        total_tokens: system_tokens + message_tokens + tool_schema_tokens + overhead_tokens,
        message_tokens,
        system_tokens,
        tool_schema_tokens,
        overhead_tokens,
    }
}

fn append_tool_result_to_context(
    context: &mut ContextManager,
    project_root: &Path,
    tool_call_id: String,
    tool_name: &str,
    parameters: &serde_json::Value,
    result: &ToolResult,
) -> PeriResult<()> {
    let (observation_result, evidence_ref) =
        tool_result_for_context_observation(project_root, tool_name, parameters, result);
    let observation = serde_json::to_string(&observation_result)
        .map_err(|err| PeriError::Parse(format!("failed to serialize tool result: {err}")))?;
    let mut entry =
        ContextEntry::trusted(ContextSource::Tool, observation).with_tool_call_id(tool_call_id);
    if let Some(evidence) = evidence_ref {
        entry = entry.with_evidence_ref(evidence);
    }
    context.append(entry);
    Ok(())
}

fn tool_result_for_context_observation(
    project_root: &Path,
    tool_name: &str,
    parameters: &serde_json::Value,
    result: &ToolResult,
) -> (ToolResult, Option<EvidenceRef>) {
    let raw_result_value = serde_json::to_value(result).unwrap_or_else(|_| {
        serde_json::json!({
            "success": result.success,
            "summary": result.summary,
            "output": result.output,
        })
    });
    let evidence_ref = EvidenceLedger::new(project_root)
        .record_tool_result(tool_name, parameters, &raw_result_value, &result.summary)
        .ok();
    let Some(evidence) = evidence_ref.clone() else {
        return (result.clone(), None);
    };
    let raw_len = serde_json::to_string(&raw_result_value)
        .map(|text| text.len())
        .unwrap_or_default();
    let evidence_json = serde_json::json!({
        "id": evidence.id.clone(),
        "kind": evidence.kind.clone(),
        "path": evidence.path.clone(),
        "bytes": evidence.bytes,
        "digest": evidence.digest.clone(),
        "summary": evidence.summary.clone(),
    });
    if raw_len <= TOOL_OBSERVATION_INLINE_LIMIT {
        let original_output = result.output.clone();
        let mut output = result.output.clone();
        match &mut output {
            serde_json::Value::Object(map) => {
                map.insert("_peridot_evidence".to_string(), evidence_json);
            }
            _ => {
                output = serde_json::json!({
                    "value": original_output,
                    "_peridot_evidence": evidence_json,
                });
            }
        }
        return (
            ToolResult {
                success: result.success,
                summary: result.summary.clone(),
                output,
            },
            evidence_ref,
        );
    }
    (
        ToolResult {
            success: result.success,
            summary: format!(
                "{} (raw output stored as evidence {}; call evidence_read to inspect exact content)",
                result.summary, evidence.id
            ),
            output: serde_json::json!({
                "compressed": true,
                "reason": "tool output exceeded live context inline limit",
                "inline_limit_chars": TOOL_OBSERVATION_INLINE_LIMIT,
                "evidence": evidence_json,
            }),
        },
        evidence_ref,
    )
}

pub(crate) fn should_prefetch_codebase_survey(mode: ExecutionMode, task: &str) -> bool {
    if mode == ExecutionMode::Plan {
        return false;
    }
    let lower = task.to_ascii_lowercase();
    let broad_english = [
        "read the project",
        "analyze the project",
        "understand the codebase",
        "codebase",
        "entire repo",
        "whole repo",
        "find bugs",
        "bug audit",
        "systematically",
        "architecture",
    ];
    let broad_korean = [
        "프로젝트를 읽",
        "프로젝트를 분석",
        "코드베이스",
        "전체 구조",
        "체계적으로",
        "버그가 존재",
        "어떤 프로젝트",
        "기능을 추가",
    ];
    broad_english.iter().any(|needle| lower.contains(needle))
        || broad_korean.iter().any(|needle| task.contains(needle))
}

pub(crate) fn codebase_survey_prompt(task: &str, context: &ContextManager) -> String {
    format!(
        "{}\n\n[codebase survey task]\n\
The parent received this broad request:\n{task}\n\n\
Survey the repository with cheap read-only tools. Do not edit files. Do not read every document in full. \
Build a compact map of likely relevant crates/modules, current UX surfaces, and risk points. \
Return only:\n\
1. Project summary in 5 bullets.\n\
2. Relevant files/symbols with path:line evidence where you directly inspected them.\n\
3. Potential bug/risk list with evidence or explicit uncertainty.\n\
4. Next reads the parent should perform before answering.\n\n\
Treat summaries and filenames as hypotheses unless you inspected exact source.",
        context.subagent_mission_packet(4_000)
    )
}

/// Builds the `[sub-agent review]` directive injected after a
/// successful `agent_delegate` call. Extracts the workspace diff from
/// the serialized `SubAgentResult` (under `output.diff`) and wraps it
/// in an explicit "verify this" instruction. Empty diff → empty
/// directive (the helper returns "" and the caller skips the append).
pub(crate) fn build_subagent_review(output: &serde_json::Value) -> String {
    let diff = output.get("diff").and_then(|v| v.as_str()).unwrap_or("");
    let summary = output.get("summary").and_then(|v| v.as_str()).unwrap_or("");
    let workspace = output
        .get("workspace")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    if diff.trim().is_empty() {
        // No diff captured — happens for prepare-only LocalSubAgentRunner
        // (no inner execution) or when `git diff` failed. Still surface
        // a soft review reminder so the parent doesn't blindly trust
        // the text summary.
        return format!(
            "[sub-agent review] Sub-agent reported: \"{summary}\". No diff captured. Inspect the workspace at {workspace} before declaring done."
        );
    }
    // Cap the diff at 4000 chars so a giant refactor doesn't blow the
    // parent's context window. The parent can always read individual
    // files for detail.
    let trimmed = if diff.chars().count() > 4000 {
        let head: String = diff.chars().take(4000).collect();
        format!("{head}\n…(diff truncated; read individual files for detail)")
    } else {
        diff.to_string()
    };
    format!(
        "[sub-agent review] Sub-agent finished with summary: \"{summary}\".\n\
         Workspace: {workspace}\n\
         Captured diff:\n\
         ```\n{trimmed}\n```\n\
         Inspect this diff before declaring done. If the change looks wrong, fix it; do not trust the summary text alone."
    )
}

/// Runs `git diff` in `project_root` and returns its stdout. Returns
/// an empty string when git isn't available or the directory isn't a
/// repo — auto-grade is best-effort, never blocks the run.
pub(crate) fn collect_git_diff(project_root: &std::path::Path) -> String {
    std::process::Command::new("git")
        .args(["diff", "HEAD"])
        .current_dir(project_root)
        .output()
        .ok()
        .map(|output| String::from_utf8_lossy(&output.stdout).to_string())
        .unwrap_or_default()
}

fn snapshot_context_to_disk<F>(path: &std::path::Path, context: &ContextManager, events: &mut F)
where
    F: FnMut(AgentRunEvent),
{
    let entries = context.snapshot_entries();
    let bytes = match serde_json::to_vec(&entries) {
        Ok(bytes) => bytes,
        Err(err) => {
            events(AgentRunEvent::Recovery {
                message: format!("context snapshot serialize failed: {err}"),
            });
            return;
        }
    };
    if let Some(parent) = path.parent()
        && let Err(err) = std::fs::create_dir_all(parent)
    {
        events(AgentRunEvent::Recovery {
            message: format!(
                "context snapshot create_dir_all {} failed: {err}",
                parent.display()
            ),
        });
        return;
    }
    let temp = path.with_extension("tmp");
    if let Err(err) = std::fs::write(&temp, &bytes) {
        events(AgentRunEvent::Recovery {
            message: format!("context snapshot write {} failed: {err}", temp.display()),
        });
        return;
    }
    if let Err(err) = std::fs::rename(&temp, path) {
        events(AgentRunEvent::Recovery {
            message: format!(
                "context snapshot rename {} -> {} failed: {err}",
                temp.display(),
                path.display()
            ),
        });
    }
}

/// Builds the provider-neutral tool definitions surfaced through native tool calling
/// from every tool registered on the harness. Order is deterministic because the
/// registry stores tools in a `BTreeMap`.
fn registry_tool_definitions(registry: &ToolRegistry) -> Vec<ToolDefinition> {
    registry
        .descriptors()
        .into_iter()
        .map(|descriptor| ToolDefinition {
            name: descriptor.name,
            description: descriptor.description,
            parameters: descriptor.parameters,
        })
        .collect()
}

/// Decodes the model's tool call arguments. Providers normalise them to a
/// `serde_json::Value`, but Chat Completions returns the JSON as a raw string that
/// may already have been parsed into `Value::Null` on the wire — coerce both shapes
/// into an object so downstream `validate_params` checks keep working.
fn tool_invocation_parameters(invocation: &ToolInvocation) -> serde_json::Value {
    match &invocation.arguments {
        serde_json::Value::Null => serde_json::json!({}),
        serde_json::Value::String(raw) => {
            serde_json::from_str(raw).unwrap_or(serde_json::json!({}))
        }
        other => other.clone(),
    }
}

#[cfg(test)]
mod helpers_tests {
    use super::*;

    #[test]
    fn enforce_vision_capability_strips_images_for_text_only_models() {
        use peridot_llm::{ImageContent, MessageRole};
        let image = ImageContent {
            media_type: "image/png".to_string(),
            data: "QUJD".to_string(),
        };
        let mut messages = vec![LlmMessage::user_with_images("look", vec![image.clone()])];

        // Vision-capable model with vision enabled: images preserved.
        enforce_vision_capability(&mut messages, "claude-opus-4-8", true, None);
        assert_eq!(messages[0].images.len(), 1);

        // Vision disabled by config: images stripped even on a vision model.
        enforce_vision_capability(&mut messages, "claude-opus-4-8", false, None);
        assert!(messages[0].images.is_empty());

        // Text-only model: images stripped, text kept.
        let mut messages = vec![LlmMessage::user_with_images("look", vec![image])];
        enforce_vision_capability(&mut messages, "gpt-3.5-turbo", true, None);
        assert!(messages[0].images.is_empty());
        assert_eq!(messages[0].content, "look");
        assert_eq!(messages[0].role, MessageRole::User);
    }

    /// Test OCR backend that returns a fixed string for any image.
    struct FixedOcr(&'static str);
    impl ImageTextExtractor for FixedOcr {
        fn extract(&self, _image: &ImageContent) -> Option<String> {
            Some(self.0.to_string())
        }
    }

    #[test]
    fn ocr_fallback_injects_tagged_text_for_text_only_models() {
        let image = ImageContent {
            media_type: "image/png".to_string(),
            data: "QUJD".to_string(),
        };
        let extractor = FixedOcr("hello from the image");

        // Text-only model + OCR backend: images dropped, OCR text injected.
        let mut messages = vec![LlmMessage::user_with_images("look", vec![image.clone()])];
        enforce_vision_capability(&mut messages, "gpt-3.5-turbo", true, Some(&extractor));
        assert!(messages[0].images.is_empty());
        assert!(messages[0].content.contains("look"));
        assert!(
            messages[0]
                .content
                .contains("<image-ocr>\nhello from the image\n</image-ocr>")
        );

        // Vision-capable model: OCR is not invoked and images are preserved.
        let mut messages = vec![LlmMessage::user_with_images("look", vec![image.clone()])];
        enforce_vision_capability(&mut messages, "claude-opus-4-8", true, Some(&extractor));
        assert_eq!(messages[0].images.len(), 1);
        assert_eq!(messages[0].content, "look");

        // Vision disabled: images dropped without OCR even with a backend.
        let mut messages = vec![LlmMessage::user_with_images("look", vec![image])];
        enforce_vision_capability(&mut messages, "gpt-3.5-turbo", false, Some(&extractor));
        assert!(messages[0].images.is_empty());
        assert_eq!(messages[0].content, "look");
    }

    #[test]
    fn route_vision_model_prefers_capable_override_only_when_needed() {
        // Text-only active model + capable override + images → route.
        assert_eq!(
            route_vision_model("gpt-3.5-turbo", Some("gpt-4o"), true, true),
            "gpt-4o"
        );
        // No images → keep the active model.
        assert_eq!(
            route_vision_model("gpt-3.5-turbo", Some("gpt-4o"), true, false),
            "gpt-3.5-turbo"
        );
        // Active model already vision-capable → keep it.
        assert_eq!(
            route_vision_model("claude-opus-4-8", Some("gpt-4o"), true, true),
            "claude-opus-4-8"
        );
        // Override is itself text-only → don't route to it.
        assert_eq!(
            route_vision_model("gpt-3.5-turbo", Some("gpt-3.5-turbo"), true, true),
            "gpt-3.5-turbo"
        );
        // Vision disabled → never route.
        assert_eq!(
            route_vision_model("gpt-3.5-turbo", Some("gpt-4o"), false, true),
            "gpt-3.5-turbo"
        );
        // No override configured → keep the active model.
        assert_eq!(
            route_vision_model("gpt-3.5-turbo", None, true, true),
            "gpt-3.5-turbo"
        );
    }

    #[test]
    fn transition_phase_emits_event_on_change() {
        use peridot_common::{ExecutionMode, PermissionMode};
        let mut state = AgentState::new(ExecutionMode::Plan, PermissionMode::Safe);
        let mut emitted = Vec::new();
        let mut sink = |event: AgentRunEvent| emitted.push(event);

        transition_phase(&mut state, AgentPhase::Executing, "tool_started", &mut sink);

        assert_eq!(state.phase, AgentPhase::Executing);
        assert_eq!(emitted.len(), 1);
        match &emitted[0] {
            AgentRunEvent::PhaseChanged { from, to, reason } => {
                assert_eq!(*from, AgentPhase::Planning);
                assert_eq!(*to, AgentPhase::Executing);
                assert_eq!(reason, "tool_started");
            }
            other => panic!("expected PhaseChanged, got {other:?}"),
        }
    }

    #[test]
    fn transition_phase_skips_event_when_phase_unchanged() {
        use peridot_common::{ExecutionMode, PermissionMode};
        let mut state = AgentState::new(ExecutionMode::Plan, PermissionMode::Safe);
        state.phase = AgentPhase::Recovering;
        let mut emitted = Vec::new();
        let mut sink = |event: AgentRunEvent| emitted.push(event);

        transition_phase(&mut state, AgentPhase::Recovering, "turn_error", &mut sink);

        assert_eq!(state.phase, AgentPhase::Recovering);
        assert!(
            emitted.is_empty(),
            "no event should fire for a self-transition: got {emitted:?}"
        );
    }

    #[test]
    fn build_subagent_review_with_diff_carries_workspace_and_diff() {
        let payload = serde_json::json!({
            "summary": "added function foo",
            "workspace": "/tmp/sub-1",
            "diff": "+++ src/lib.rs\n+fn foo() {}\n",
        });
        let review = build_subagent_review(&payload);
        assert!(review.contains("[sub-agent review]"));
        assert!(review.contains("added function foo"));
        assert!(review.contains("/tmp/sub-1"));
        assert!(review.contains("fn foo()"));
        assert!(review.contains("do not trust the summary"));
    }

    #[test]
    fn build_subagent_review_without_diff_still_warns_to_inspect() {
        let payload = serde_json::json!({
            "summary": "task complete",
            "workspace": "/tmp/sub-2",
            "diff": "",
        });
        let review = build_subagent_review(&payload);
        assert!(review.contains("[sub-agent review]"));
        assert!(review.contains("No diff captured"));
        assert!(review.contains("/tmp/sub-2"));
    }

    #[test]
    fn take_pending_resume_returns_none_when_file_missing() {
        let path = std::env::temp_dir().join(format!(
            "peridot-pending-missing-{}.bin",
            std::process::id()
        ));
        // Ensure the file does not exist.
        let _ = std::fs::remove_file(&path);
        assert!(take_pending_resume(Some(&path)).is_none());
    }

    #[test]
    fn take_pending_resume_consumes_valid_sidecar() {
        let path =
            std::env::temp_dir().join(format!("peridot-pending-valid-{}.bin", std::process::id()));
        let call = ToolCall::new(
            "shell_exec",
            serde_json::json!({ "command": "npm install left-pad" }),
        );
        std::fs::write(&path, serde_json::to_vec(&call).unwrap()).unwrap();
        let recovered = take_pending_resume(Some(&path)).expect("recovered");
        assert_eq!(recovered.name, "shell_exec");
        assert_eq!(
            recovered.parameters.get("command").and_then(|v| v.as_str()),
            Some("npm install left-pad")
        );
        assert!(
            !path.exists(),
            "sidecar should be deleted after consumption"
        );
    }

    #[test]
    fn take_pending_resume_handles_unparseable_payload() {
        let path = std::env::temp_dir().join(format!(
            "peridot-pending-garbage-{}.bin",
            std::process::id()
        ));
        std::fs::write(&path, b"not json at all").unwrap();
        assert!(take_pending_resume(Some(&path)).is_none());
        // garbage file is left on disk for the operator to inspect
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn pending_resume_observation_pairs_dangling_tool_call() {
        let mut context = ContextManager::new();
        context.append(ContextEntry::assistant_with_tool_calls(
            "",
            vec![ToolInvocation {
                id: "call_pending".to_string(),
                name: "shell_exec".to_string(),
                arguments: serde_json::json!({ "command": "rm -rf tmp" }),
            }],
        ));
        let result = ToolResult::success("removed tmp", serde_json::Value::Null);

        append_pending_resume_observation(&mut context, "shell_exec", &result).unwrap();

        let last = context.entries().last().expect("tool result appended");
        assert_eq!(last.source, ContextSource::Tool);
        assert_eq!(last.tool_call_id.as_deref(), Some("call_pending"));
        assert!(last.content.contains("removed tmp"));
    }

    #[test]
    fn pending_resume_observation_uses_plan_reminder_without_dangling_call() {
        let mut context = ContextManager::new();
        let result = ToolResult::success("done", serde_json::Value::Null);

        append_pending_resume_observation(&mut context, "shell_exec", &result).unwrap();

        let last = context.entries().last().expect("resume note appended");
        assert_eq!(last.source, ContextSource::PlanReminder);
        assert!(last.tool_call_id.is_none());
    }

    #[test]
    fn build_subagent_review_truncates_giant_diffs() {
        let big_diff = "a".repeat(8000);
        let payload = serde_json::json!({
            "summary": "",
            "workspace": "/tmp/sub-3",
            "diff": big_diff,
        });
        let review = build_subagent_review(&payload);
        assert!(review.contains("diff truncated"));
        assert!(review.chars().count() < 6000);
    }
}
