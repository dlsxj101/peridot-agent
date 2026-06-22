//! Goal-control and read-only inspection slash command handlers
//! (`/goal pause|resume|clear|status`, `/plan show`, `/cost`, `/info`)
//! plus their result-rendering helpers, split out of the daemon module.
//! Parent (private) items are reached via `use super::*`.

use peridot_core::GoalStatus;
use serde_json::Value;

use super::*;

pub(super) async fn handle_command_goal_control(
    state: &DaemonState,
    session_id: Option<&str>,
    raw_command: &str,
    command: &SlashCommand,
) -> Result<Value, String> {
    let Some(session_id) = session_id else {
        return Ok(goal_command_result(
            raw_command,
            None,
            "none",
            None,
            0,
            0,
            "goal: no active session",
        ));
    };
    let Some((goal, plan)) = ({
        let sessions = state.sessions.lock().await;
        sessions.get(session_id).map(|entry| {
            let mut goal = entry.goal.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            if goal.objective.is_none() && entry.spec.mode == ExecutionMode::Goal {
                *goal = initial_live_goal(&entry.spec);
            }
            match command {
                SlashCommand::GoalPause if goal.objective.is_some() => {
                    goal.status = Some(GoalStatus::Paused);
                }
                SlashCommand::GoalResume if goal.objective.is_some() => {
                    goal.status = Some(GoalStatus::Running);
                }
                SlashCommand::GoalClear => {
                    *goal = LiveSessionGoal {
                        objective: None,
                        status: Some(GoalStatus::Cleared),
                        started_at_unix: None,
                    };
                    *entry.plan.lock().unwrap_or_else(std::sync::PoisonError::into_inner) =
                        LiveSessionPlan::default();
                }
                _ => {}
            }
            (
                goal.clone(),
                entry
                    .plan
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .clone(),
            )
        })
    }) else {
        return Ok(goal_command_result(
            raw_command,
            Some(session_id),
            "missing",
            None,
            0,
            0,
            &format!("goal: session not found: {session_id}"),
        ));
    };
    let done = plan.steps.iter().filter(|step| step.done).count();
    let total = plan.steps.len();
    let status = goal
        .status
        .as_ref()
        .map(goal_status_label)
        .unwrap_or("none");
    let message = match command {
        SlashCommand::GoalPause if goal.objective.is_some() => "goal: paused".to_string(),
        SlashCommand::GoalResume if goal.objective.is_some() => "goal: resumed".to_string(),
        SlashCommand::GoalClear => "goal: cleared".to_string(),
        _ => format!("goal: {status} {done}/{total} steps done"),
    };
    Ok(goal_command_result(
        raw_command,
        Some(session_id),
        status,
        goal.objective.as_deref(),
        done,
        total,
        &message,
    )
    .with_object("started_at_unix", goal.started_at_unix))
}

trait JsonObjectExt {
    fn with_object(self, key: &str, value: Option<u64>) -> Value;
}

impl JsonObjectExt for Value {
    fn with_object(mut self, key: &str, value: Option<u64>) -> Value {
        if let Some(object) = self.as_object_mut() {
            object.insert(
                key.to_string(),
                serde_json::to_value(value).unwrap_or(Value::Null),
            );
        }
        self
    }
}

fn goal_command_result(
    raw_command: &str,
    session_id: Option<&str>,
    status: &str,
    objective: Option<&str>,
    done: usize,
    total: usize,
    message: &str,
) -> Value {
    serde_json::json!({
        "kind": "goal",
        "title": "Goal",
        "message": message,
        "severity": "info",
        "command": raw_command,
        "session_id": session_id,
        "status": status,
        "objective": objective,
        "done": done,
        "total": total,
        "items": [
            { "label": "status", "detail": status },
            { "label": "objective", "detail": objective.unwrap_or("<none>") },
            { "label": "steps", "detail": format!("{done}/{total}") },
        ],
    })
}

fn goal_status_label(status: &GoalStatus) -> &'static str {
    match status {
        GoalStatus::Running => "running",
        GoalStatus::Paused => "paused",
        GoalStatus::Done => "done",
        GoalStatus::Cleared => "cleared",
    }
}

pub(super) async fn handle_command_plan_show(
    state: &DaemonState,
    session_id: Option<&str>,
    raw_command: &str,
) -> Result<Value, String> {
    let plan = if let Some(session_id) = session_id {
        state.sessions.lock().await.get(session_id).map(|entry| {
            entry
                .plan
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone()
        })
    } else {
        None
    }
    .unwrap_or_default();
    let done = plan.steps.iter().filter(|step| step.done).count();
    let total = plan.steps.len();
    let items: Vec<Value> = plan
        .steps
        .iter()
        .enumerate()
        .map(|(index, step)| {
            let current = plan.current == Some(index as u32);
            let status = if step.done {
                "done"
            } else if current {
                "in_progress"
            } else {
                "pending"
            };
            serde_json::json!({
                "label": format!("{}. {}", index + 1, step.label),
                "detail": status,
                "source": status,
                "step_index": index,
                "done": step.done,
                "current": current,
            })
        })
        .collect();
    Ok(serde_json::json!({
        "kind": "plan",
        "title": "Plan",
        "message": if total == 0 {
            "plan: <empty>".to_string()
        } else {
            format!("plan: {done}/{total} steps")
        },
        "severity": "info",
        "command": raw_command,
        "items": items,
        "session_id": session_id,
        "steps": plan
            .steps
            .iter()
            .enumerate()
            .map(|(index, step)| {
                let current = plan.current == Some(index as u32);
                let status = if step.done {
                    "done"
                } else if current {
                    "in_progress"
                } else {
                    "pending"
                };
                serde_json::json!({
                    "text": step.label,
                    "label": step.label,
                    "done": step.done,
                    "status": status,
                    "current": current,
                })
            })
            .collect::<Vec<_>>(),
        "current": plan.current,
        "done": done,
        "total": total,
    }))
}

#[derive(Clone, Debug)]
struct CostSessionRow {
    id: String,
    title: String,
    status: String,
    task: Option<String>,
    executor_tokens: u64,
    executor_cost_usd: f64,
    committee_tokens: u64,
    committee_cost_usd: f64,
    turns_used: u32,
}

impl CostSessionRow {
    fn all_in_tokens(&self) -> u64 {
        self.executor_tokens + self.committee_tokens
    }

    fn all_in_cost_usd(&self) -> f64 {
        self.executor_cost_usd + self.committee_cost_usd
    }
}

pub(super) async fn handle_command_cost(
    state: &DaemonState,
    session_id: Option<&str>,
    raw_command: &str,
) -> Result<Value, String> {
    let rows = cost_session_rows(state).await?;
    let current = session_id.and_then(|id| rows.iter().find(|row| row.id == id).cloned());
    let executor_tokens: u64 = rows.iter().map(|row| row.executor_tokens).sum();
    let executor_cost_usd: f64 = rows.iter().map(|row| row.executor_cost_usd).sum();
    let committee_tokens: u64 = rows.iter().map(|row| row.committee_tokens).sum();
    let committee_cost_usd: f64 = rows.iter().map(|row| row.committee_cost_usd).sum();
    let total_tokens = executor_tokens + committee_tokens;
    let total_cost_usd = executor_cost_usd + committee_cost_usd;
    let budget_limit = current_budget_limit(state, session_id).await;
    let budget_pct = budget_limit
        .filter(|limit| *limit > 0.0)
        .map(|limit| total_cost_usd / limit * 100.0);
    let current_cost = current
        .as_ref()
        .map(CostSessionRow::all_in_cost_usd)
        .unwrap_or(total_cost_usd);
    let current_tokens = current
        .as_ref()
        .map(CostSessionRow::all_in_tokens)
        .unwrap_or(total_tokens);
    let items: Vec<Value> = rows
        .iter()
        .map(|row| {
            serde_json::json!({
                "label": row.title,
                "detail": format!(
                    "${:.4} · {} tok · {} turn(s)",
                    row.all_in_cost_usd(),
                    row.all_in_tokens(),
                    row.turns_used
                ),
                "source": row.status,
                "session_id": row.id,
                "tokens": row.all_in_tokens(),
                "total_cost_usd": row.all_in_cost_usd(),
                "executor_tokens": row.executor_tokens,
                "executor_cost_usd": row.executor_cost_usd,
                "committee_tokens": row.committee_tokens,
                "committee_cost_usd": row.committee_cost_usd,
                "turns_used": row.turns_used,
                "last_task": row.task,
            })
        })
        .collect();
    let message = if rows.is_empty() {
        "cost: $0.0000 · tokens: 0 · sessions: 0".to_string()
    } else if let Some(limit) = budget_limit.filter(|limit| *limit > 0.0) {
        format!(
            "cost: ${current_cost:.4} · tokens: {current_tokens} · aggregate: ${total_cost_usd:.4} / ${limit:.4} ({:.0}%) across {} session(s)",
            budget_pct.unwrap_or_default(),
            rows.len()
        )
    } else {
        format!(
            "cost: ${current_cost:.4} · tokens: {current_tokens} · aggregate: ${total_cost_usd:.4} across {} session(s)",
            rows.len()
        )
    };
    Ok(serde_json::json!({
        "kind": "cost",
        "title": "Cost",
        "message": message,
        "severity": "info",
        "command": raw_command,
        "items": items,
        "session_id": session_id,
        "session_count": rows.len(),
        "current_cost_usd": current_cost,
        "current_tokens": current_tokens,
        "total_tokens": total_tokens,
        "total_cost_usd": total_cost_usd,
        "executor_tokens": executor_tokens,
        "executor_cost_usd": executor_cost_usd,
        "committee_tokens": committee_tokens,
        "committee_cost_usd": committee_cost_usd,
        "budget_limit_usd": budget_limit,
        "budget_pct": budget_pct,
    }))
}

async fn cost_session_rows(state: &DaemonState) -> Result<Vec<CostSessionRow>, String> {
    let store = MemoryStore::new(state.project_root.join(".peridot/memory.db"));
    let records = store
        .list_session_records()
        .map_err(|err| format!("failed to read session records: {err}"))?;
    let mut rows = BTreeMap::<String, CostSessionRow>::new();
    for record in records {
        rows.insert(
            record.id.clone(),
            CostSessionRow {
                id: record.id.clone(),
                title: record_title(&record),
                status: format!("{:?}", record.status).to_ascii_lowercase(),
                task: record.last_task.clone(),
                executor_tokens: record.total_tokens,
                executor_cost_usd: record.total_cost_usd,
                committee_tokens: 0,
                committee_cost_usd: 0.0,
                turns_used: record.turns_used,
            },
        );
    }
    for (id, entry) in state.sessions.lock().await.iter() {
        let usage = entry
            .usage
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        let row = rows.entry(id.clone()).or_insert_with(|| CostSessionRow {
            id: id.clone(),
            title: session_title_from_task(&entry.spec.task),
            status: "running".to_string(),
            task: Some(entry.spec.task.clone()),
            executor_tokens: 0,
            executor_cost_usd: 0.0,
            committee_tokens: 0,
            committee_cost_usd: 0.0,
            turns_used: 0,
        });
        row.status = "running".to_string();
        row.task = Some(entry.spec.task.clone());
        row.executor_tokens = row.executor_tokens.max(usage.total_tokens);
        row.executor_cost_usd = row.executor_cost_usd.max(usage.cost_usd);
        row.turns_used = row.turns_used.max(usage.turns_used);
        row.committee_tokens = usage.committee_planner_tokens + usage.committee_reviewer_tokens;
        row.committee_cost_usd =
            usage.committee_planner_cost_usd + usage.committee_reviewer_cost_usd;
    }
    Ok(rows.into_values().collect())
}

async fn current_budget_limit(state: &DaemonState, session_id: Option<&str>) -> Option<f64> {
    if let Some(session_id) = session_id
        && let Some(entry) = state.sessions.lock().await.get(session_id)
    {
        let usage = entry
            .usage
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        return usage.cost_limit.or_else(|| {
            (entry.spec.config.defaults.budget_usd > 0.0)
                .then_some(entry.spec.config.defaults.budget_usd)
        });
    }
    (state.run_template.budget_usd > 0.0).then_some(state.run_template.budget_usd)
}

pub(super) async fn handle_command_info(
    state: &DaemonState,
    session_id: Option<&str>,
    raw_command: &str,
) -> Result<Value, String> {
    let live_spec = if let Some(session_id) = session_id {
        state
            .sessions
            .lock()
            .await
            .get(session_id)
            .map(|entry| entry.spec.clone())
    } else {
        None
    };
    let store = MemoryStore::new(state.project_root.join(".peridot/memory.db"));
    let record = session_id.and_then(|id| {
        store
            .list_session_records()
            .unwrap_or_default()
            .into_iter()
            .find(|record| record.id == id)
    });
    let config = live_spec
        .as_ref()
        .map(|spec| spec.config.clone())
        .unwrap_or_else(|| state.run_config.as_ref().clone());
    let session_label = session_id.unwrap_or("<none>");
    let workspace_root = record
        .as_ref()
        .map(|record| record.workspace_root.clone())
        .unwrap_or_else(|| state.project_root.as_ref().clone());
    let workspace = workspace_root
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("<unknown>")
        .to_string();
    let model = live_spec
        .as_ref()
        .and_then(|spec| spec.model.clone())
        .unwrap_or_else(|| {
            if live_spec.is_some() {
                config.models.main.clone()
            } else {
                state.run_template.model.clone()
            }
        });
    let provider = config.auth.primary.clone();
    let mode = live_spec
        .as_ref()
        .map(|spec| spec.mode)
        .unwrap_or(config.defaults.mode);
    let permission = live_spec
        .as_ref()
        .map(|spec| spec.permission)
        .unwrap_or(state.run_template.permission);
    let reasoning_effort = live_spec
        .as_ref()
        .and_then(|spec| spec.reasoning_effort)
        .unwrap_or(state.run_template.reasoning_effort);
    let service_tier = match live_spec
        .as_ref()
        .and_then(|spec| spec.service_tier.as_ref())
    {
        Some(Some(tier)) => Some(tier.clone()),
        Some(None) => None,
        None => state.run_template.service_tier.clone(),
    };
    let status = if live_spec.is_some() {
        "running".to_string()
    } else if let Some(record) = record.as_ref() {
        format!("{:?}", record.status).to_ascii_lowercase()
    } else if session_id.is_some() {
        "unknown".to_string()
    } else {
        "workspace".to_string()
    };
    let turns_used = record.as_ref().map(|record| record.turns_used).unwrap_or(0);
    let total_tokens = record
        .as_ref()
        .map(|record| record.total_tokens)
        .unwrap_or(0);
    let total_cost_usd = record
        .as_ref()
        .map(|record| record.total_cost_usd)
        .unwrap_or(0.0);
    let mut items = vec![
        serde_json::json!({ "label": "session", "detail": session_label }),
        serde_json::json!({ "label": "workspace", "detail": workspace }),
        serde_json::json!({ "label": "status", "detail": status }),
        serde_json::json!({ "label": "model", "detail": model }),
        serde_json::json!({ "label": "provider", "detail": provider }),
        serde_json::json!({ "label": "mode", "detail": mode.to_string() }),
        serde_json::json!({ "label": "permission", "detail": permission.to_string() }),
        serde_json::json!({ "label": "reasoning", "detail": reasoning_effort.to_string() }),
        serde_json::json!({
            "label": "service tier",
            "detail": service_tier.as_deref().unwrap_or("standard")
        }),
        serde_json::json!({ "label": "turns", "detail": turns_used.to_string() }),
        serde_json::json!({ "label": "tokens", "detail": total_tokens.to_string() }),
        serde_json::json!({ "label": "cost", "detail": format!("${total_cost_usd:.4}") }),
    ];
    if let Some(record) = record.as_ref() {
        if let Some(task) = record.last_task.as_deref()
            && !task.trim().is_empty()
        {
            items.push(serde_json::json!({ "label": "last task", "detail": task }));
        }
        if let Some(branch) = record.worktree_branch.as_deref() {
            items.push(serde_json::json!({ "label": "worktree branch", "detail": branch }));
        }
    }
    Ok(serde_json::json!({
        "kind": "info",
        "title": "Session Info",
        "message": format!(
            "info: session {session_label} · workspace {workspace} · model {model} · provider {provider} · mode {mode} · permission {permission} · turn {turns_used} · tokens {total_tokens} · cost ${total_cost_usd:.4}"
        ),
        "severity": "info",
        "command": raw_command,
        "items": items,
        "session_id": session_id,
        "workspace": workspace,
        "workspace_root": workspace_root,
        "status": status,
        "model": model,
        "provider": provider,
        "mode": mode.to_string(),
        "permission": permission.to_string(),
        "reasoning_effort": reasoning_effort.to_string(),
        "service_tier": service_tier,
        "committee_mode": config.committee.mode.to_string(),
        "turns_used": turns_used,
        "total_tokens": total_tokens,
        "total_cost_usd": total_cost_usd,
    }))
}
