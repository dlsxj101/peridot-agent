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
    let session_id = format!("session-{}-{}", std::process::id(), unix_timestamp());
    run_lifecycle_hook(agent, &options, &session_id, "session_start", "running", "")?;
    let summary = agent
        .run_until_done(
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
        eprintln!("warning: failed to save session {session_id}: {err}");
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
