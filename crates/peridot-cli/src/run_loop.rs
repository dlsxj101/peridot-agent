use super::*;

pub(super) async fn run_task(
    task: String,
    mode: ExecutionMode,
    cli: &Cli,
    config: &PeridotConfig,
    project_root: &Path,
) -> Result<()> {
    maybe_print_update_notice(config, cli.effective_headless(), cli.output).await;
    let task = apply_resume(task, cli.resume.as_deref(), project_root)?;
    let permission = cli
        .permission
        .map(PermissionMode::from)
        .unwrap_or(config.defaults.permission);
    let model = cli
        .model
        .clone()
        .unwrap_or_else(|| config.models.main.clone());
    let state = if mode == ExecutionMode::Goal {
        AgentState::new(mode, permission).with_goal(task.clone())
    } else {
        AgentState::new(mode, permission)
    };
    let max_turns = cli.max_turns.unwrap_or(config.defaults.max_turns);
    let budget_usd = cli.budget.unwrap_or(config.defaults.budget_usd);
    let mut registry = ToolRegistry::new();
    register_builtin_tools(&mut registry)?;
    register_configured_mcp_tools(&mut registry, config).await?;
    let mut context = ContextManager::with_limits(project_context_limits_from_config(
        project_root,
        &config.context,
    ));
    let window = resolve_model_window(&model, &config.auth.primary).await;
    context.set_model_window_tokens(Some(window));
    let mut agent = HarnessAgent::new(state, context, registry);

    let ndjson_events = cli.effective_ndjson_events();
    if let Some(mock_response_file) = &cli.mock_response_file {
        let profile = ProjectScanner::new().scan(project_root)?;
        let denied_paths = profile.boundaries.into_iter().map(PathBuf::from).collect();
        let provider = FileMockProvider::from_file(mock_response_file)?;
        let summary = run_agent_loop_with_default_observability(
            &mut agent,
            &provider,
            RunLoopOptions {
                task,
                model: model.clone(),
                reasoning_effort: config.models.reasoning_effort,
                service_tier: normalize_model_service_tier(&model, &config.models.service_tier),
                max_turns,
                budget_usd,
                config,
                project_root,
                denied_paths,
            },
            ndjson_events,
        )
        .await?;
        if cli.output == OutputFormat::Json {
            println!(
                "{}",
                serde_json::to_string_pretty(&run_summary_output(&summary, mode))?
            );
        } else {
            print_run_summary_text(&summary, mode);
        }
        exit_for_summary(&summary, cli.effective_headless());
        return Ok(());
    }

    let _live_flag_kept_for_compatibility = cli.live;
    let profile = ProjectScanner::new().scan(project_root)?;
    let denied_paths = profile.boundaries.into_iter().map(PathBuf::from).collect();
    let provider_box = live_provider(config, &model, project_root).await?;
    let provider: std::sync::Arc<dyn peridot_llm::LlmProvider> = std::sync::Arc::from(provider_box);
    agent.set_subagent_runner(std::sync::Arc::new(
        peridot_core::InnerLoopSubAgent::new(
            provider.clone(),
            project_root.to_path_buf(),
            model.clone(),
        )
        .with_max_turns(max_turns.clamp(1, 8))
        .with_max_tokens(4096)
        .with_permission(permission)
        .with_security(config.security.clone())
        .with_reasoning_effort(config.models.reasoning_effort),
    ));
    agent.set_auto_verify_after_mutation(config.defaults.auto_verify_after_mutation);
    agent.set_auto_grade_on_done(config.defaults.auto_grade_on_done);
    agent.set_auto_fix_cap(config.auto_fix.max_attempts);
    let summary = run_agent_loop_with_default_observability(
        &mut agent,
        provider.as_ref(),
        RunLoopOptions {
            task,
            model: model.clone(),
            reasoning_effort: config.models.reasoning_effort,
            service_tier: normalize_model_service_tier(&model, &config.models.service_tier),
            max_turns,
            budget_usd,
            config,
            project_root,
            denied_paths,
        },
        ndjson_events,
    )
    .await?;
    if cli.output == OutputFormat::Json {
        println!(
            "{}",
            serde_json::to_string_pretty(&run_summary_output(&summary, mode))?
        );
    } else {
        print_run_summary_text(&summary, mode);
    }
    exit_for_summary(&summary, cli.effective_headless());
    Ok(())
}

#[derive(Clone, Debug)]
pub(crate) struct AgentTaskOptions {
    pub(crate) permission: PermissionMode,
    pub(crate) model: String,
    pub(crate) reasoning_effort: peridot_common::ReasoningEffort,
    pub(crate) service_tier: Option<String>,
    pub(crate) max_turns: u32,
    pub(crate) budget_usd: f64,
    pub(crate) resume: Option<String>,
    pub(crate) mock_response_file: Option<PathBuf>,
    pub(crate) live: bool,
}

pub(crate) fn agent_task_options(cli: &Cli, config: &PeridotConfig) -> AgentTaskOptions {
    let model = cli
        .model
        .clone()
        .unwrap_or_else(|| config.models.main.clone());
    AgentTaskOptions {
        permission: cli
            .permission
            .map(PermissionMode::from)
            .unwrap_or(config.defaults.permission),
        model: model.clone(),
        reasoning_effort: config.models.reasoning_effort,
        service_tier: normalize_model_service_tier(&model, &config.models.service_tier),
        max_turns: cli.max_turns.unwrap_or(config.defaults.max_turns),
        budget_usd: cli.budget.unwrap_or(config.defaults.budget_usd),
        resume: cli.resume.clone(),
        mock_response_file: cli.mock_response_file.clone(),
        live: cli.live,
    }
}

pub(super) fn normalize_model_service_tier(
    model: &str,
    configured_tier: &Option<String>,
) -> Option<String> {
    let model = model.trim().to_ascii_lowercase();
    if model.ends_with("-fast") {
        return Some("fast".to_string());
    }
    configured_tier
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .and_then(|value| match value.to_ascii_lowercase().as_str() {
            "fast" | "priority" => Some("fast".to_string()),
            "off" | "none" | "standard" | "default" => None,
            other => Some(other.to_string()),
        })
}

/// Picks the active model's max input-token window. When
/// `auth_primary == "openrouter-api"` we
/// consult the cached OpenRouter catalog first (24h disk cache, no auth
/// required) so slugs the static heuristic table doesn't recognise — or
/// new releases whose context window grew — get the exact value. For
/// OpenAI OAuth we consult the ChatGPT/Codex model catalog because it is
/// account/plan aware. For every other path we fall straight through to the
/// heuristic table and finally to a 200K floor.
pub(super) async fn resolve_model_window(model: &str, auth_primary: &str) -> usize {
    let lookup_model = model.strip_suffix("-fast").unwrap_or(model);
    if auth_primary == "openrouter-api"
        && let Some(cache_dir) = peridot_llm::catalog::default_cache_dir()
        && let Some(catalog) = peridot_llm::catalog::openrouter_context_lengths(&cache_dir).await
        && let Some(window) = catalog.get(lookup_model).copied()
    {
        return window;
    }
    if auth_primary == "openai-oauth"
        && let Some(cache_dir) = peridot_llm::catalog::default_cache_dir()
        && let Some(credentials) = read_stored_openai_oauth_credentials().await.ok().flatten()
        && let Some(account_id) = credentials.account_id.as_deref()
        && let Some(catalog) = peridot_llm::catalog::openai_codex_context_lengths(
            &cache_dir,
            &credentials.access_token,
            account_id,
            "peridot",
        )
        .await
        && let Some(window) = catalog.get(lookup_model).copied()
    {
        return window;
    }
    if auth_primary == "openai-oauth" && lookup_model.to_ascii_lowercase().starts_with("gpt-5") {
        // ChatGPT/Codex OAuth is plan-aware and currently much smaller than
        // the public Platform API window for the same model aliases. If the
        // live catalog is unavailable, prefer the conservative Codex value
        // over the generic OpenAI API heuristic.
        return 272_000;
    }
    peridot_llm::context_window_tokens(lookup_model).unwrap_or(200_000)
}

#[allow(clippy::too_many_arguments)]
/// Optional inter-session messaging hookup. `(bus, session_id)` are
/// installed on the harness so it drains its inbox every turn and
/// `agent_message` calls route through `bus`. Pass `None` for headless
/// or single-session runs.
pub(crate) type MessageBusHookup =
    Option<(std::sync::Arc<dyn peridot_tools::AgentMessageBus>, String)>;

#[allow(clippy::too_many_arguments)]
pub(crate) async fn run_task_with_events<F>(
    task: String,
    mode: ExecutionMode,
    options: AgentTaskOptions,
    config: PeridotConfig,
    project_root: PathBuf,
    cancel: Option<peridot_core::CancelToken>,
    compact_request: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
    context_snapshot_path: Option<PathBuf>,
    ask_user_port: Option<std::sync::Arc<dyn peridot_tools::AskUserPort>>,
    message_bus: MessageBusHookup,
    events: F,
) -> Result<peridot_core::AgentRunSummary>
where
    F: FnMut(AgentRunEvent) + Send,
{
    let task = apply_resume(task, options.resume.as_deref(), &project_root)?;
    let state = if mode == ExecutionMode::Goal {
        AgentState::new(mode, options.permission).with_goal(task.clone())
    } else {
        AgentState::new(mode, options.permission)
    };
    let mut registry = ToolRegistry::new();
    register_builtin_tools(&mut registry)?;
    register_configured_mcp_tools(&mut registry, &config).await?;
    let mut context = ContextManager::with_limits(project_context_limits_from_config(
        &project_root,
        &config.context,
    ));
    // Resolve the active model's full context window. The TUI displays this
    // exact value; auto-compaction triggers from `window * auto_compaction_pct`.
    //
    // For OpenRouter we fetch the live catalog from `/v1/models` (no auth
    // required), which returns each slug's exact `context_length`. The
    // result is disk-cached for 24h under `~/.peridot/cache/`, so the
    // first run pays one HTTP round-trip and every subsequent run reads
    // from disk. Stale cache is preferred over a network failure so the
    // operator stays online when OpenRouter is briefly unreachable.
    //
    // For OpenAI OAuth, the ChatGPT/Codex catalog is account/plan aware and
    // can differ from public API model windows for the same slug. For other
    // providers (and as a fallback when live catalog lookup misses) we use the
    // static heuristic table in `peridot_llm::context_window_tokens`.
    let window = resolve_model_window(&options.model, &config.auth.primary).await;
    context.set_model_window_tokens(Some(window));
    if let Some(path) = context_snapshot_path.as_ref()
        && let Ok(bytes) = std::fs::read(path)
        && let Ok(entries) = serde_json::from_slice::<Vec<peridot_context::ContextEntry>>(&bytes)
    {
        context.restore_entries(entries);
    }
    let mut agent = HarnessAgent::new(state, context, registry);
    if let Some(token) = cancel {
        agent.set_cancel_token(token);
    }
    if let Some(flag) = compact_request {
        agent.set_compact_request(flag);
    }
    if let Some(port) = ask_user_port {
        agent.set_ask_user_port(port);
    }
    if let Some((bus, session_id)) = message_bus {
        agent.set_message_bus(bus);
        agent.set_session_id(session_id);
    }
    if let Some(path) = context_snapshot_path.as_ref() {
        // Resume-after-approval sidecar lives next to context.bin.
        // When the previous halt persisted a pending tool call here,
        // the next session executes it under the new (presumably
        // relaxed) security posture instead of restarting the task.
        if let Some(parent) = path.parent() {
            agent.set_pending_resume_path(parent.join("pending_resume.bin"));
        }
    }
    if let Some(path) = context_snapshot_path {
        agent.set_context_snapshot_path(path);
    }
    if let Some(path) = peridot_project::locate_agents_md(&project_root) {
        agent.set_agents_md_path(path);
    }
    let profile = ProjectScanner::new().scan(&project_root)?;
    let denied_paths: Vec<PathBuf> = profile.boundaries.into_iter().map(PathBuf::from).collect();
    let mut events = events;

    if let Some(mock_response_file) = options.mock_response_file.clone() {
        let provider = FileMockProvider::from_file(&mock_response_file)?;
        run_planner_preflight_if_enabled(
            &mut agent,
            &provider,
            &task,
            &config,
            &options,
            &project_root,
            &denied_paths,
            &mut events,
        )
        .await?;
        if config.committee.mode == peridot_common::CommitteeMode::Full {
            return run_committee_loop_with_events(
                &mut agent,
                &provider,
                RunLoopOptions {
                    task,
                    model: options.model,
                    reasoning_effort: options.reasoning_effort,
                    service_tier: options.service_tier.clone(),
                    max_turns: options.max_turns,
                    budget_usd: options.budget_usd,
                    config: &config,
                    project_root: &project_root,
                    denied_paths,
                },
                events,
            )
            .await;
        }
        return run_agent_loop_with_events(
            &mut agent,
            &provider,
            RunLoopOptions {
                task,
                model: options.model,
                reasoning_effort: options.reasoning_effort,
                service_tier: options.service_tier.clone(),
                max_turns: options.max_turns,
                budget_usd: options.budget_usd,
                config: &config,
                project_root: &project_root,
                denied_paths,
            },
            events,
        )
        .await;
    }

    let _live_flag_kept_for_compatibility = options.live;
    let provider_box = live_provider(&config, &options.model, &project_root).await?;
    let provider: std::sync::Arc<dyn peridot_llm::LlmProvider> = std::sync::Arc::from(provider_box);
    agent.set_subagent_runner(std::sync::Arc::new(
        peridot_core::InnerLoopSubAgent::new(
            provider.clone(),
            project_root.clone(),
            options.model.clone(),
        )
        .with_max_turns(options.max_turns.clamp(1, 8))
        .with_max_tokens(4096)
        .with_permission(options.permission)
        .with_security(config.security.clone())
        .with_reasoning_effort(options.reasoning_effort),
    ));
    agent.set_auto_verify_after_mutation(config.defaults.auto_verify_after_mutation);
    agent.set_auto_grade_on_done(config.defaults.auto_grade_on_done);
    agent.set_auto_fix_cap(config.auto_fix.max_attempts);
    run_planner_preflight_if_enabled(
        &mut agent,
        provider.as_ref(),
        &task,
        &config,
        &options,
        &project_root,
        &denied_paths,
        &mut events,
    )
    .await?;
    if config.committee.mode == peridot_common::CommitteeMode::Full {
        return run_committee_loop_with_events(
            &mut agent,
            provider.as_ref(),
            RunLoopOptions {
                task,
                model: options.model,
                reasoning_effort: options.reasoning_effort,
                service_tier: options.service_tier.clone(),
                max_turns: options.max_turns,
                budget_usd: options.budget_usd,
                config: &config,
                project_root: &project_root,
                denied_paths,
            },
            events,
        )
        .await;
    }
    run_agent_loop_with_events(
        &mut agent,
        provider.as_ref(),
        RunLoopOptions {
            task,
            model: options.model,
            reasoning_effort: options.reasoning_effort,
            service_tier: options.service_tier.clone(),
            max_turns: options.max_turns,
            budget_usd: options.budget_usd,
            config: &config,
            project_root: &project_root,
            denied_paths,
        },
        events,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
pub(super) async fn run_planner_preflight_if_enabled<F>(
    executor: &mut HarnessAgent,
    provider: &dyn peridot_llm::LlmProvider,
    task: &str,
    config: &PeridotConfig,
    options: &AgentTaskOptions,
    project_root: &Path,
    denied_paths: &[PathBuf],
    events: &mut F,
) -> Result<()>
where
    F: FnMut(AgentRunEvent) + Send,
{
    use peridot_common::CommitteeMode;
    if config.committee.mode == CommitteeMode::Off {
        return Ok(());
    }
    // Length gate: chat-style inputs below the threshold skip planner
    // preflight — planning overhead dwarfs the value for trivial asks.
    let task_chars = task.trim().chars().count();
    if task_chars < config.committee.min_task_chars {
        return Ok(());
    }
    // LLM complexity gate (opt-in): when on, classify the task with a
    // single capped-output call to the main model and skip preflight
    // unless it is complex or architectural. On classifier failure
    // the helper returns Complex so the planner still fires — a
    // missed planner is worse than an extra one.
    if config.committee.use_llm_complexity_gate {
        let verdict =
            peridot_core::classify_task_complexity(provider, &options.model, task).await?;
        if !verdict.warrants_planner() {
            return Ok(());
        }
    }
    let planner_model = if config.committee.planner_model.is_empty() {
        options.model.clone()
    } else {
        config.committee.planner_model.clone()
    };

    let mut planner_registry = ToolRegistry::new();
    register_builtin_tools(&mut planner_registry)?;
    let planner_context = ContextManager::with_limits(project_context_limits_from_config(
        project_root,
        &config.context,
    ));
    let planner_state = peridot_core::AgentState::new(ExecutionMode::Plan, options.permission);
    let mut planner_agent = HarnessAgent::new(planner_state, planner_context, planner_registry);
    planner_agent.set_role(peridot_core::AgentRole::Planner);

    // Planner gets at most a few turns to produce its plan; budget is capped
    // at a fraction of the executor budget so a runaway planner can't burn
    // the whole session's headroom.
    let planner_max_turns = options.max_turns.clamp(1, 3);
    let planner_budget = if options.budget_usd > 0.0 {
        (options.budget_usd * 0.25).max(0.001)
    } else {
        0.0
    };

    let summary = run_agent_loop_with_events(
        &mut planner_agent,
        provider,
        RunLoopOptions {
            task: task.to_string(),
            model: planner_model,
            reasoning_effort: options.reasoning_effort,
            service_tier: options.service_tier.clone(),
            max_turns: planner_max_turns,
            budget_usd: planner_budget,
            config,
            project_root,
            denied_paths: denied_paths.to_vec(),
        },
        &mut *events,
    )
    .await?;

    let planner_tokens = summary.usage.input_tokens
        + summary.usage.output_tokens
        + summary.usage.cache_read_tokens
        + summary.usage.cache_creation_tokens
        + summary.usage.reasoning_output_tokens;
    events(AgentRunEvent::CommitteeRoleUsage {
        role: "planner".to_string(),
        cost_usd: summary.usage.estimated_cost_usd,
        tokens: planner_tokens,
    });
    if let Some(plan_text) = extract_planner_plan(&planner_agent, &summary) {
        executor
            .context_mut()
            .append(peridot_context::ContextEntry::trusted(
                peridot_context::ContextSource::PlanReminder,
                format!("Committee plan from planner:\n\n{plan_text}"),
            ));
        events(AgentRunEvent::PlannerPlanReady { plan_text });
    }
    Ok(())
}

fn extract_planner_plan(
    agent: &HarnessAgent,
    summary: &peridot_core::AgentRunSummary,
) -> Option<String> {
    if let Some(content) = agent
        .context()
        .entries()
        .iter()
        .rev()
        .find(|entry| matches!(entry.source, peridot_context::ContextSource::Assistant))
        .map(|entry| entry.content.trim().to_string())
        && !content.is_empty()
    {
        return Some(content);
    }
    summary.turns.iter().rev().find_map(|turn| {
        let summary = turn.tool_result.summary.trim();
        if summary.is_empty() {
            None
        } else {
            Some(summary.to_string())
        }
    })
}

pub(super) async fn register_configured_mcp_tools(
    registry: &mut ToolRegistry,
    config: &PeridotConfig,
) -> Result<()> {
    for server in &config.mcp {
        let tools = McpClient::new(server.clone()).list_tools().await?;
        register_mcp_tools(registry, server.clone(), tools)?;
    }
    Ok(())
}

pub(super) struct RunLoopOptions<'a> {
    task: String,
    model: String,
    reasoning_effort: peridot_common::ReasoningEffort,
    service_tier: Option<String>,
    max_turns: u32,
    budget_usd: f64,
    config: &'a PeridotConfig,
    project_root: &'a Path,
    denied_paths: Vec<PathBuf>,
}

/// Backwards-compatible silent variant. `run_task` now goes through
/// `run_agent_loop_with_default_observability` directly because every
/// caller wants the `ndjson_events` knob; this wrapper stays for
/// non-CLI callers (e.g. integration test harnesses) that didn't yet
/// migrate.
#[allow(dead_code)]
pub(super) async fn run_agent_loop(
    agent: &mut HarnessAgent,
    provider: &dyn LlmProvider,
    options: RunLoopOptions<'_>,
) -> Result<peridot_core::AgentRunSummary> {
    run_agent_loop_with_default_observability(agent, provider, options, false).await
}

/// `run_agent_loop` with an explicit knob to also stream every event
/// as JSON-lines to stderr. Used by `run_task` when the user passed
/// `--ndjson-events` (or `--headless`, which implicitly opts in).
/// Stderr is intentional: stdout is reserved for the final summary so
/// shell pipes like `peridot run --headless "…" | jq` keep parsing
/// just the summary.
pub(super) async fn run_agent_loop_with_default_observability(
    agent: &mut HarnessAgent,
    provider: &dyn LlmProvider,
    options: RunLoopOptions<'_>,
    ndjson_events: bool,
) -> Result<peridot_core::AgentRunSummary> {
    run_agent_loop_with_events(agent, provider, options, |event| {
        if ndjson_events {
            emit_ndjson_event(&event);
        }
        if let AgentRunEvent::Recovery { message } = &event
            && !ndjson_events
        {
            // Already serialised above; avoid duplicating the
            // recovery line when we're in JSONL mode.
            eprintln!("recovery: {message}");
        }
    })
    .await
}

/// Serialise an [`AgentRunEvent`] to a single line on stderr. Failure
/// to serialise is logged as a structured fallback line so the consumer
/// still sees *something* in their event stream — silent drops would
/// defeat the whole point of the flag.
fn emit_ndjson_event(event: &AgentRunEvent) {
    match serde_json::to_string(event) {
        Ok(line) => eprintln!("{line}"),
        Err(err) => eprintln!(
            r#"{{"kind":"_serialization_error","error":{:?},"context":"ndjson event"}}"#,
            err.to_string()
        ),
    }
}

pub(super) async fn run_agent_loop_with_events<F>(
    agent: &mut HarnessAgent,
    provider: &dyn LlmProvider,
    options: RunLoopOptions<'_>,
    mut events: F,
) -> Result<peridot_core::AgentRunSummary>
where
    F: FnMut(AgentRunEvent) + Send,
{
    let session_id = format!("session-{}-{}", std::process::id(), unix_timestamp());
    run_lifecycle_hook(agent, &options, &session_id, "session_start", "running", "")?;
    let summary = agent
        .run_until_done_with_events(
            provider,
            AgentRunRequest {
                task: options.task.clone(),
                model: options.model.clone(),
                goal_checker_model: Some(options.config.models.goal_checker().to_string()),
                max_turns: options.max_turns,
                max_tokens: 4096,
                reasoning_effort: options.reasoning_effort,
                service_tier: options.service_tier.clone(),
                budget_usd: options.budget_usd,
                budget_warning_pct: options.config.defaults.budget_warning_pct,
                project_root: options.project_root.to_path_buf(),
                denied_paths: options.denied_paths.clone(),
                hooks: options.config.hooks.clone(),
                security: options.config.security.clone(),
            },
            &mut events,
        )
        .await?;
    run_lifecycle_hook(
        agent,
        &options,
        &session_id,
        "session_end",
        &format!("{:?}", summary.stopped_reason),
        &format!("turns={}", summary.turns.len()),
    )?;
    run_completion_lifecycle_hooks(agent, &options, &session_id, &summary)?;
    // Pass the live provider through so `save_auto_skill` can route
    // the SKILL.md rewrite through the same model the session was
    // running on. The fallback template is used when this branch
    // didn't supply a provider (e.g. mock sessions).
    if let Err(err) = save_run_session(
        options.project_root,
        &session_id,
        &summary,
        &options.task,
        &options.config.memory,
        Some(provider),
        &options.model,
    )
    .await
    {
        events(AgentRunEvent::SessionSaveFailed {
            session_id: session_id.clone(),
            message: err.to_string(),
        });
        eprintln!("warning: failed to save session {session_id}: {err}");
    } else {
        events(AgentRunEvent::SessionSaved {
            session_id: session_id.clone(),
        });
    }
    if let Err(err) = auto_commit_run(
        options.project_root,
        options.config,
        &summary,
        &options.task,
    ) {
        eprintln!("warning: failed to auto-commit session {session_id}: {err}");
    }
    Ok(summary)
}

pub(super) async fn run_committee_loop_with_events<F>(
    executor: &mut HarnessAgent,
    provider: &dyn LlmProvider,
    options: RunLoopOptions<'_>,
    mut events: F,
) -> Result<peridot_core::AgentRunSummary>
where
    F: FnMut(AgentRunEvent) + Send,
{
    use peridot_context::ContextEntry as Entry;
    use peridot_context::ContextSource as Source;
    use peridot_core::{AgentRunSummary, AgentTurnRequest, ReviewerVerdict as Verdict, StopReason};

    let session_id = format!("session-{}-{}", std::process::id(), unix_timestamp());
    run_lifecycle_hook(
        executor,
        &options,
        &session_id,
        "session_start",
        "running",
        "",
    )?;
    events(AgentRunEvent::RunStarted {
        task: options.task.clone(),
    });
    let started_at = std::time::Instant::now();

    let reviewer_model = if options.config.committee.reviewer_model.is_empty() {
        options.model.clone()
    } else {
        options.config.committee.reviewer_model.clone()
    };
    let max_review_passes = options.config.committee.max_review_passes.max(1);

    let mut total_usage = peridot_llm::Usage::default();
    let accumulate_usage = peridot_core::accumulate_usage;
    let mut outcomes: Vec<peridot_core::AgentTurnOutcome> = Vec::new();
    let mut review_pass_counter: u32 = 0;
    let mut user_input_for_next: Option<String> = Some(options.task.clone());
    let mut stop_reason = StopReason::MaxTurns;
    let cancel_token = executor.cancel_token();

    for turn_index in 0..options.max_turns {
        if let Some(token) = cancel_token.as_ref()
            && token.is_cancelled()
        {
            stop_reason = StopReason::Interrupted;
            events(AgentRunEvent::Interrupted {
                stage: "committee_turn_start".to_string(),
            });
            break;
        }
        events(AgentRunEvent::TurnStarted { turn_index });
        let turn_request = AgentTurnRequest {
            user_input: user_input_for_next.take(),
            model: options.model.clone(),
            max_tokens: 4096,
            reasoning_effort: options.reasoning_effort,
            service_tier: options.service_tier.clone(),
            project_root: options.project_root.to_path_buf(),
            denied_paths: options.denied_paths.clone(),
            hooks: options.config.hooks.clone(),
            security: options.config.security.clone(),
        };
        let outcome = match executor
            .run_turn_with_events(provider, turn_request, &mut events)
            .await
        {
            Ok(outcome) => outcome,
            Err(err) => {
                events(AgentRunEvent::Recovery {
                    message: format!("committee turn {turn_index} failed: {err}"),
                });
                stop_reason = StopReason::ApprovalRequired;
                break;
            }
        };
        let turn_success = outcome.tool_result.success;
        accumulate_usage(&mut total_usage, outcome.usage);
        events(AgentRunEvent::UsageUpdated { usage: total_usage });
        events(AgentRunEvent::TurnEnded {
            turn_index,
            success: turn_success,
        });

        let mutating = turn_success && is_mutating_tool(&outcome.tool_name);
        let done = outcome.tool_name == "agent_done" && turn_success;
        outcomes.push(outcome);

        if mutating {
            let diff = collect_diff_for_review(options.project_root);
            match run_reviewer_pass(
                provider,
                &reviewer_model,
                &options.task,
                &diff,
                &options.config.security,
                &mut events,
            )
            .await
            {
                Ok(verdict) => {
                    events(AgentRunEvent::ReviewerVerdict {
                        turn_index,
                        verdict: verdict.clone(),
                    });
                    match verdict {
                        Verdict::Approve => {
                            review_pass_counter = 0;
                        }
                        Verdict::RequestChanges { comments } => {
                            executor.context_mut().append(Entry::trusted(
                                Source::ReviewerComment,
                                format!(
                                    "Reviewer feedback for turn {turn_index}:\n{comments}\n\nAddress the feedback before declaring agent_done."
                                ),
                            ));
                            review_pass_counter += 1;
                            if review_pass_counter >= max_review_passes {
                                events(AgentRunEvent::Recovery {
                                    message: format!(
                                        "committee: reviewer requested changes {max_review_passes} consecutive turns; auto-blocking"
                                    ),
                                });
                                if let Some(token) = cancel_token.as_ref() {
                                    token.cancel();
                                }
                                stop_reason = StopReason::Interrupted;
                                events(AgentRunEvent::Interrupted {
                                    stage: "committee_review_loop".to_string(),
                                });
                                break;
                            }
                        }
                        Verdict::Block { reason } => {
                            events(AgentRunEvent::Recovery {
                                message: format!("committee reviewer blocked: {reason}"),
                            });
                            let overridden = if let Some(port) = executor.ask_user_port() {
                                let answer = port
                                    .ask(peridot_common::AskUserRequest::SingleSelect {
                                        question: format!(
                                            "Reviewer blocked this change:\n{reason}\n\nOverride and continue, or accept the block?"
                                        ),
                                        options: vec![
                                            "Override and continue".to_string(),
                                            "Accept block and stop".to_string(),
                                        ],
                                        default_index: Some(1),
                                    })
                                    .await;
                                matches!(
                                    answer,
                                    peridot_common::AskUserAnswer::Selected { index: 0, .. }
                                )
                            } else {
                                false
                            };
                            if overridden {
                                executor.context_mut().append(Entry::trusted(
                                    Source::PlanReminder,
                                    format!(
                                        "Operator overrode reviewer block (reason: {reason}). Proceed with caution."
                                    ),
                                ));
                            } else {
                                if let Some(token) = cancel_token.as_ref() {
                                    token.cancel();
                                }
                                stop_reason = StopReason::Interrupted;
                                events(AgentRunEvent::Interrupted {
                                    stage: "committee_review_block".to_string(),
                                });
                                break;
                            }
                        }
                    }
                }
                Err(err) => {
                    events(AgentRunEvent::Recovery {
                        message: format!(
                            "committee reviewer pass failed: {err}; continuing without verdict"
                        ),
                    });
                }
            }
        } else {
            review_pass_counter = 0;
        }

        if done {
            stop_reason = StopReason::Done;
            break;
        }
        if options.budget_usd > 0.0 && total_usage.estimated_cost_usd >= options.budget_usd {
            stop_reason = StopReason::Budget;
            break;
        }
    }

    let summary = AgentRunSummary {
        turns: outcomes,
        usage: total_usage,
        stopped_reason: stop_reason,
        duration_ms: started_at.elapsed().as_millis() as u64,
    };
    events(AgentRunEvent::Finished {
        summary: summary.clone(),
    });
    run_lifecycle_hook(
        executor,
        &options,
        &session_id,
        "session_end",
        &format!("{:?}", summary.stopped_reason),
        &format!("turns={}", summary.turns.len()),
    )?;
    run_completion_lifecycle_hooks(executor, &options, &session_id, &summary)?;
    if let Err(err) = save_run_session(
        options.project_root,
        &session_id,
        &summary,
        &options.task,
        &options.config.memory,
        Some(provider),
        &options.model,
    )
    .await
    {
        events(AgentRunEvent::SessionSaveFailed {
            session_id: session_id.clone(),
            message: err.to_string(),
        });
    } else {
        events(AgentRunEvent::SessionSaved {
            session_id: session_id.clone(),
        });
    }
    if let Err(err) = auto_commit_run(
        options.project_root,
        options.config,
        &summary,
        &options.task,
    ) {
        eprintln!("warning: committee auto-commit failed for session {session_id}: {err}");
    }
    Ok(summary)
}

fn is_mutating_tool(name: &str) -> bool {
    matches!(name, "file_write" | "file_patch" | "shell_exec")
}

fn collect_diff_for_review(project_root: &Path) -> String {
    let manager = peridot_git::GitManager::new(project_root);
    if !manager.is_repository() {
        return "(workspace is not a git repository; reviewer received no diff)".to_string();
    }
    let raw = manager
        .diff()
        .unwrap_or_else(|err| format!("(git diff unavailable: {err})"));
    if raw.trim().is_empty() {
        "(no uncommitted changes detected)".to_string()
    } else {
        truncate_diff(&raw, 8_000)
    }
}

fn truncate_diff(raw: &str, max_chars: usize) -> String {
    if raw.chars().count() <= max_chars {
        return raw.to_string();
    }
    let mut out: String = raw.chars().take(max_chars.saturating_sub(80)).collect();
    out.push_str("\n... <diff truncated for reviewer context>");
    out
}

async fn run_reviewer_pass<F>(
    provider: &dyn LlmProvider,
    reviewer_model: &str,
    task: &str,
    diff: &str,
    _security: &peridot_common::SecurityConfig,
    events: &mut F,
) -> Result<peridot_core::ReviewerVerdict>
where
    F: FnMut(AgentRunEvent) + Send,
{
    use peridot_core::AgentRole;
    use peridot_llm::{CompletionRequest, LlmMessage, MessageRole, ToolChoice};
    let system = AgentRole::Reviewer
        .system_prompt_suffix()
        .trim_start_matches('\n')
        .to_string();
    let user_prompt = format!(
        "Original task:\n{task}\n\nDiff produced by Executor:\n```\n{diff}\n```\n\nReturn the verdict JSON now."
    );
    let request = CompletionRequest {
        model: reviewer_model.to_string(),
        system: Some(system),
        messages: vec![LlmMessage::new(MessageRole::User, user_prompt)],
        max_tokens: Some(512),
        thinking: false,
        reasoning_effort: peridot_common::ReasoningEffort::Off,
        service_tier: None,
        tools: Vec::new(),
        tool_choice: ToolChoice::Auto,
    };
    let completion = provider.complete(request).await?;
    let reviewer_tokens = completion.usage.input_tokens
        + completion.usage.output_tokens
        + completion.usage.cache_read_tokens
        + completion.usage.cache_creation_tokens
        + completion.usage.reasoning_output_tokens;
    events(AgentRunEvent::CommitteeRoleUsage {
        role: "reviewer".to_string(),
        cost_usd: completion.usage.estimated_cost_usd,
        tokens: reviewer_tokens,
    });
    parse_reviewer_verdict(&completion.text).ok_or_else(|| {
        anyhow::anyhow!(
            "reviewer returned a non-verdict response (could not parse JSON): {}",
            completion.text.trim()
        )
    })
}

pub(super) fn parse_reviewer_verdict(raw: &str) -> Option<peridot_core::ReviewerVerdict> {
    let trimmed = strip_code_fence(raw.trim());
    let json: serde_json::Value = serde_json::from_str(trimmed).ok()?;
    let verdict_kind = json.get("verdict")?.as_str()?;
    let comments = json
        .get("comments")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    match verdict_kind {
        "approve" => Some(peridot_core::ReviewerVerdict::Approve),
        "request_changes" => Some(peridot_core::ReviewerVerdict::RequestChanges { comments }),
        "block" => Some(peridot_core::ReviewerVerdict::Block { reason: comments }),
        _ => None,
    }
}

fn strip_code_fence(raw: &str) -> &str {
    let trimmed = raw.trim();
    if let Some(body) = trimmed
        .strip_prefix("```json")
        .or_else(|| trimmed.strip_prefix("```"))
    {
        let body = body.trim_start_matches('\n');
        if let Some(end) = body.rfind("```") {
            return body[..end].trim();
        }
    }
    trimmed
}

pub(super) fn run_lifecycle_hook(
    agent: &HarnessAgent,
    options: &RunLoopOptions<'_>,
    session_id: &str,
    event: &str,
    status: &str,
    summary: &str,
) -> Result<()> {
    let variables = lifecycle_hook_variables(
        session_id,
        &agent.state().mode.to_string(),
        &agent.state().permission.to_string(),
        options.project_root,
        status,
        summary,
    );
    HookRunner::new(options.project_root, options.config.hooks.clone())
        .run_lifecycle_hooks(event, &variables)?;
    Ok(())
}

pub(super) fn run_completion_lifecycle_hooks(
    agent: &HarnessAgent,
    options: &RunLoopOptions<'_>,
    session_id: &str,
    summary: &AgentRunSummary,
) -> Result<()> {
    if summary.stopped_reason != StopReason::Done {
        return Ok(());
    }
    match agent.state().mode {
        ExecutionMode::Plan => run_lifecycle_hook(
            agent,
            options,
            session_id,
            "plan_completed",
            "done",
            "plan_file=todo.md",
        ),
        ExecutionMode::Goal => run_lifecycle_hook(
            agent,
            options,
            session_id,
            "goal_achieved",
            "done",
            agent.state().goal.as_deref().unwrap_or(&options.task),
        ),
        ExecutionMode::Execute => Ok(()),
    }
}
