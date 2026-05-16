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
    let context = ContextManager::with_limits(project_context_limits_from_config(
        project_root,
        &config.context,
    ));
    let mut agent = HarnessAgent::new(state, context, registry);

    if let Some(mock_response_file) = &cli.mock_response_file {
        let profile = ProjectScanner::new().scan(project_root)?;
        let denied_paths = profile.boundaries.into_iter().map(PathBuf::from).collect();
        let provider = FileMockProvider::from_file(mock_response_file)?;
        let summary = run_agent_loop(
            &mut agent,
            &provider,
            RunLoopOptions {
                task,
                model,
                max_turns,
                budget_usd,
                config,
                project_root,
                denied_paths,
            },
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
    let provider = live_provider(config, &model, project_root).await?;
    let summary = run_agent_loop(
        &mut agent,
        provider.as_ref(),
        RunLoopOptions {
            task,
            model,
            max_turns,
            budget_usd,
            config,
            project_root,
            denied_paths,
        },
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
pub(super) struct AgentTaskOptions {
    pub(super) permission: PermissionMode,
    pub(super) model: String,
    pub(super) max_turns: u32,
    pub(super) budget_usd: f64,
    pub(super) resume: Option<String>,
    pub(super) mock_response_file: Option<PathBuf>,
    pub(super) live: bool,
}

pub(super) fn agent_task_options(cli: &Cli, config: &PeridotConfig) -> AgentTaskOptions {
    AgentTaskOptions {
        permission: cli
            .permission
            .map(PermissionMode::from)
            .unwrap_or(config.defaults.permission),
        model: cli
            .model
            .clone()
            .unwrap_or_else(|| config.models.main.clone()),
        max_turns: cli.max_turns.unwrap_or(config.defaults.max_turns),
        budget_usd: cli.budget.unwrap_or(config.defaults.budget_usd),
        resume: cli.resume.clone(),
        mock_response_file: cli.mock_response_file.clone(),
        live: cli.live,
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) async fn run_task_with_events<F>(
    task: String,
    mode: ExecutionMode,
    options: AgentTaskOptions,
    config: PeridotConfig,
    project_root: PathBuf,
    cancel: Option<peridot_core::CancelToken>,
    context_snapshot_path: Option<PathBuf>,
    events: F,
) -> Result<peridot_core::AgentRunSummary>
where
    F: FnMut(AgentRunEvent),
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
        return run_agent_loop_with_events(
            &mut agent,
            &provider,
            RunLoopOptions {
                task,
                model: options.model,
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
    let provider = live_provider(&config, &options.model, &project_root).await?;
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
    run_agent_loop_with_events(
        &mut agent,
        provider.as_ref(),
        RunLoopOptions {
            task,
            model: options.model,
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
pub(super) async fn run_planner_preflight_if_enabled<P, F>(
    executor: &mut HarnessAgent,
    provider: &P,
    task: &str,
    config: &PeridotConfig,
    options: &AgentTaskOptions,
    project_root: &Path,
    denied_paths: &[PathBuf],
    events: &mut F,
) -> Result<()>
where
    P: peridot_llm::LlmProvider + ?Sized,
    F: FnMut(AgentRunEvent),
{
    use peridot_common::CommitteeMode;
    if config.committee.mode == CommitteeMode::Off {
        return Ok(());
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
            max_turns: planner_max_turns,
            budget_usd: planner_budget,
            config,
            project_root,
            denied_paths: denied_paths.to_vec(),
        },
        &mut *events,
    )
    .await?;

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
    max_turns: u32,
    budget_usd: f64,
    config: &'a PeridotConfig,
    project_root: &'a Path,
    denied_paths: Vec<PathBuf>,
}

pub(super) async fn run_agent_loop<P>(
    agent: &mut HarnessAgent,
    provider: &P,
    options: RunLoopOptions<'_>,
) -> Result<peridot_core::AgentRunSummary>
where
    P: LlmProvider + ?Sized,
{
    run_agent_loop_with_events(agent, provider, options, |_| {}).await
}

pub(super) async fn run_agent_loop_with_events<P, F>(
    agent: &mut HarnessAgent,
    provider: &P,
    options: RunLoopOptions<'_>,
    mut events: F,
) -> Result<peridot_core::AgentRunSummary>
where
    P: LlmProvider + ?Sized,
    F: FnMut(AgentRunEvent),
{
    let session_id = format!("session-{}-{}", std::process::id(), unix_timestamp());
    run_lifecycle_hook(agent, &options, &session_id, "session_start", "running", "")?;
    let summary = agent
        .run_until_done_with_events(
            provider,
            AgentRunRequest {
                task: options.task.clone(),
                model: options.model.clone(),
                goal_checker_model: Some(options.config.models.goal_checker.clone()),
                max_turns: options.max_turns,
                max_tokens: 4096,
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
    if let Err(err) = save_run_session(
        options.project_root,
        &session_id,
        &summary,
        &options.task,
        &options.config.memory,
    ) {
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
