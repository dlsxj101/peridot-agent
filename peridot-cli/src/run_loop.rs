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
    compact_request: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
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
    // Drive dynamic compaction off the active model's context window
    // when known. Unknown models keep the static thresholds from
    // ContextLimits. peridot-llm::context_window_tokens covers the
    // common Anthropic / OpenAI / Gemini / DeepSeek / Qwen families.
    context.set_model_window_tokens(peridot_llm::context_window_tokens(&options.model));
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
        .with_reasoning_effort(config.models.reasoning_effort),
    ));
    agent.set_auto_verify_after_mutation(config.defaults.auto_verify_after_mutation);
    agent.set_auto_grade_on_done(config.defaults.auto_grade_on_done);
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
    // Length gate: chat-style inputs below the threshold skip planner
    // preflight — planning overhead dwarfs the value for trivial asks.
    let task_chars = task.trim().chars().count();
    if task_chars < config.committee.min_task_chars {
        return Ok(());
    }
    // LLM complexity gate (opt-in): when on, classify the task with a
    // small model call and skip preflight unless it's complex or
    // architectural. On classifier failure the helper returns Complex
    // so the planner still fires — a missed planner is worse than an
    // extra one.
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
                reasoning_effort: options.config.models.reasoning_effort,
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

pub(super) async fn run_committee_loop_with_events<P, F>(
    executor: &mut HarnessAgent,
    provider: &P,
    options: RunLoopOptions<'_>,
    mut events: F,
) -> Result<peridot_core::AgentRunSummary>
where
    P: LlmProvider + ?Sized,
    F: FnMut(AgentRunEvent),
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
            reasoning_effort: options.config.models.reasoning_effort,
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
    ) {
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

async fn run_reviewer_pass<P, F>(
    provider: &P,
    reviewer_model: &str,
    task: &str,
    diff: &str,
    _security: &peridot_common::SecurityConfig,
    events: &mut F,
) -> Result<peridot_core::ReviewerVerdict>
where
    P: LlmProvider + ?Sized,
    F: FnMut(AgentRunEvent),
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
