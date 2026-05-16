---
name: committee-orchestration
description: Wire and verify Peridot's multi-LLM committee (Planner / Reviewer / Executor). Use when working on CommitteeConfig, AgentRole, planner pre-flight, reviewer in-loop, ReviewerVerdict parsing, role-aware system prompts, per-role cost split, or committee.ndjson persistence.
---

# Committee Orchestration

## Surfaces in play
- `peridot-common::CommitteeConfig` / `CommitteeMode { Off, Planner, Full }` — TOML `[committee]` section, slash-toggleable per session.
- `peridot-core::AgentRole { Planner, Reviewer, Executor }` — `HarnessAgent::set_role` switches the per-role system-prompt suffix.
- `peridot-core::ReviewerVerdict { Approve, RequestChanges { comments }, Block { reason } }` and the matching `AgentRunEvent::ReviewerVerdict { turn_index, verdict }`.
- `peridot-core::AgentRunEvent::{ PlannerPlanReady, CommitteeRoleUsage }`.
- `peridot-cli::run_loop::{ run_planner_preflight_if_enabled, run_committee_loop_with_events, run_reviewer_pass, parse_reviewer_verdict, collect_diff_for_review }`.
- `peridot-tui::TuiState`'s committee fields: `committee_mode`, `committee_planner_cost`, `committee_planner_tokens`, `committee_reviewer_cost`, `committee_reviewer_tokens`, `pending_committee_events`.
- On-disk artifact: `<sessions>/<id>/committee.ndjson` (one JSON line per planner / reviewer / role-usage event).
- Slash: `/committee off|planner|full`. Status bar surfaces `committee <mode>` when active. `/cost` and `/info` append per-role breakdown.

## Invariants
1. **Mode = Off is byte-identical** to the legacy single-agent loop. Every fallback path (`models.planner = ""`, missing `[committee]` section) resolves to `models.main` and Executor role.
2. **Planner runs read-only.** `AgentRole::Planner.system_prompt_suffix()` forbids `file_write` / `file_patch` / `shell_exec`. The planner runs in `ExecutionMode::Plan` so the permission system also blocks those tools.
3. **Reviewer never calls tools.** It is a single `provider.complete()` call. Response must be a JSON object `{verdict, comments}` (with optional ```json fence — `strip_code_fence` handles both); any other shape raises and the executor continues without a verdict.
4. **Per-role cost never double-counts.** Executor turns emit `UsageUpdated` (already exists). Planner and Reviewer emit `CommitteeRoleUsage` instead — the TUI accumulates them into separate `committee_planner_cost` / `committee_reviewer_cost` totals.
5. **Reviewer runaway is auto-capped.** Three consecutive `RequestChanges` verdicts (configurable via `committee.max_review_passes`) escalate to auto-Block: cancel token fires, `Interrupted` event fires with stage `committee_review_loop`, and the executor halts.
6. **Mutating tool detection** uses `is_mutating_tool` — currently `{file_write, file_patch, shell_exec}`. Read-only turns skip the reviewer pass entirely so context-investigation turns stay cheap.
7. **Diff is bounded** at 8 KB before being sent to the reviewer (`truncate_diff`). Larger diffs surface a `(diff truncated for reviewer context)` marker so the reviewer is honest about not having the full picture.

## Configuration

```toml
[committee]
mode = "full"                # off | planner | full
planner_model = "claude-haiku-4-5"
reviewer_model = "openai-gpt-4o-mini"
executor_model = ""          # empty falls back to models.main
max_review_passes = 3
```

Slash-only quick start (no config edit needed):
```
/committee full
```

## Suggested model pairings

| Setup | Planner | Executor | Reviewer | Trade-off |
|---|---|---|---|---|
| **Minimal** | `claude-haiku-4-5` | `claude-opus-4-7` | (none — `mode = "planner"`) | One cheap planning pass; no per-turn review cost. |
| **Balanced** | `claude-haiku-4-5` | `claude-sonnet-4-6` | `claude-haiku-4-5` | All-Anthropic; ~2× cost over single-agent. |
| **Cross-provider** | `claude-haiku-4-5` | `claude-sonnet-4-6` | `openai-gpt-4o-mini` | Reviewer gives an outside-of-family second opinion; ~2.5× cost. |

## Testing playbook
- **Type-level**: every new `AgentRunEvent` variant must round-trip through the CLI adapter into a matching `TuiRuntimeEvent` arm. `cargo check` will catch any drift.
- **Reviewer parser**: `parse_reviewer_verdict` covers all three outcomes plus fenced JSON in `peridot-cli/src/tests.rs` (use it as the template when adding new fields).
- **Manual smoke** with `--mock-response-file`: build a mock response file whose first response is a `file_write` action, second response (for the reviewer) is `{"verdict":"request_changes","comments":"..."}`, third response is the executor's follow-up turn. Run with `[committee] mode = "full"` and verify the comment lands in the executor context and the reviewer event lands in `committee.ndjson`.
- **Block escalation**: configure `max_review_passes = 2` and seed two reviewer responses with `request_changes`. The third executor turn should never fire — the run should auto-Block and emit `Interrupted` with stage `committee_review_loop`.

## When this skill applies
- Adding a new `AgentRunEvent` variant that the committee should care about.
- Changing reviewer prompt / verdict schema.
- Tweaking which tools count as "mutating" for the reviewer trigger.
- Persisting / replaying committee events in new CLI surfaces.
- Cross-link to `multisession-orchestration` when the committee runs inside a `/teammate` or `/fork` subagent (the reviewer applies per-session).
