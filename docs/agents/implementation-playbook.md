# Implementation Playbook

Use this playbook with `PERIDOT_SPEC_v1.md`. The spec is authoritative; this file is the working checklist.

## Global Rules
- Start every implementation session by reading the spec sections relevant to that phase.
- Preserve the phase order unless the user explicitly changes priorities.
- Keep each phase buildable before moving to the next phase.
- Avoid product shortcuts that would break later harness invariants, especially cache stability, append-only context, permissions, and deterministic serialization.

## Session 1: Workspace Skeleton
Goal: create a compiling Cargo workspace with empty crates and foundational traits/types.

Required shape:
- Workspace root `Cargo.toml`.
- Crates: `peridot-cli`, `peridot-tui`, `peridot-core`, `peridot-llm`, `peridot-context`, `peridot-tools`, `peridot-mcp`, `peridot-verify`, `peridot-agents`, `peridot-memory`, `peridot-project`, `peridot-git`, `peridot-common`.
- Define early shared types in `peridot-common`; avoid copy-pasted enums across crates.

Done when:
- `cargo build --workspace` passes.
- `cargo test --workspace` passes.
- `peridot --version` prints a version.

## Session 2: Engine Loop
Goal: LLM call, response parsing, tool execution, context feedback, and basic CLI operation.

Key work:
- Claude provider skeleton with request/streaming path and usage tracking.
- Append-only context manager with local offload and Tier 0 trimming.
- Initial built-in tools for shell, file, plan, scratchpad, and done.
- Core loop that injects `todo.md`, parses responses, executes tools, and observes results.

Done when:
- A simple task can create and modify a file.
- Unit tests cover parsing fallback, registry behavior, and context basics.

## Session 3: Code Intelligence
Goal: project scanning, AGENTS parsing, verification, and git automation.

Key work:
- Language and build command detection.
- AGENTS field parsing and boundaries.
- Build/test/lint verification tools.
- Git status, diff, log, commit, and branch tools.

Done when:
- Rust, JS/TS, and Python projects are detected.
- Boundaries block prohibited paths.
- Code changes can verify and commit as a logical unit.

## Session 4: Modes And Permissions
Goal: implement plan/execute/goal modes, safe/auto/yolo permissions, ask_user, and state transitions.

Key work:
- Read-only Plan Mode.
- Goal Mode with max turns, budget, pause/resume/status, and independent Goal Checker.
- Permission-level classification for every tool.
- Slash commands and CLI flags for mode/permission.

Done when:
- Plan Mode changes no files.
- Goal Mode can run autonomously and stop when complete.
- ask_user supports select, multiselect, freeform, defaults, and explanations.

## Session 5: Long-Running Reliability
Goal: compaction, recovery, grader, audit, and prompt-injection hardening.

Key work:
- 2-Tier compaction (deterministic + LLM). v1.x originally specified 4 tiers; the implementation absorbed the others into these two and the SPEC was realigned in v1.9.
- Stuck detection and error-specific recovery.
- Diff review and grader agent. The grader lives in the standalone `peridot-grader` crate (extracted in v0.6.0 to break a verify ↔ core dependency cycle) and is invoked from two places: `HarnessAgent::auto_grade_on_done` for the agent loop, and `VerifyPipeline::run_all_with_grader` / `peridot verify --with-grader` for the CLI verify pipeline.
- Audit logs for shell and file changes.

Done when:
- A long task survives compaction.
- Known failure modes trigger recovery instead of repetition.
- Grader feedback can cause a fix loop. Both invocation paths (agent loop + verify pipeline) reach the same `grade_work` implementation.

## Session 6: Memory, Subagents, MCP, Hooks
Goal: persistence, skills, external tools, subagents, hooks, and OpenAI provider support.

Key work:
- SQLite session/skills/errors memory.
- Fork, worktree, and teammate subagents.
- MCP stdio/http client.
- Tool/event/lifecycle hook runtime.
- OpenAI OAuth/API provider.

Done when:
- Session resume works.
- Subagents can run isolated tasks.
- MCP tools appear in the same registry as built-ins.
- Hooks can warn/block according to config.

## Session 7: TUI, Headless, Release
Goal: polished UX and release readiness.

Key work:
- Ratatui layouts, background agent event streaming, side panels, ask_user screen, menus, slash commands, approvals, and keybindings.
- Terminal-native input editing, including Backspace/Ctrl-H, Delete, cursor motion, history, multiline-safe rendering, and submit-without-exit behavior.
- In-TUI run continuity: status, plan/goal surfaces, thinking, tool activity, tool results, usage, and session save/resume should stay visible while the agent continues. Internal recovery directives should remain in debug/activity output rather than the main transcript.
- Headless JSON/text output with exit codes.
- CI, release workflow, install script, docs.

Done when:
- TUI works in full, compact, and minimal layouts.
- TUI tests cover input editing regressions, runtime events, tool previews/results, approval panels, slash commands, and render snapshots.
- Headless mode is scriptable.
- Release artifacts can be built by CI.

## Session 8: Multi-Session Runtime
Goal: turn the multi-session scaffolding (PR8–PR11) into a live runtime with concurrent agent loops, workspace isolation, subagent fan-in, and crash recovery.

Key work:
- Live `SessionRouter` spawning via `tokio::spawn`; per-session `CancelToken`; `(session_id, TuiRuntimeEvent)` multiplex.
- Workspace isolation through `peridot_git::GitManager::add_worktree` for backgrounded sessions; warn on `Shared` cwd collisions.
- Throttled atomic persistence of `TuiState`/`peridot-context` blobs under `.peridot/sessions/<id>/`; resume reconstitutes the full run.
- Subagent fan-in: `/fork`, `/teammate`, `/worktree` route through `LocalSubAgentRunner` with `parent_id` correlation in the side panel tree.
- Attention notifier surfaces background `ApprovalRequested`/`AskUser*` via tab badges and a status bar line backed by a new `PhraseKey`.

Done when:
- Two or more concurrent agent loops run with isolated events and budgets.
- Foreground swap (`Ctrl+T`/`Ctrl+W`) hot-swaps `TuiState` without losing the running session.
- `peridot session resume <id>` continues a previously suspended session in place.
- Crash mid-run leaves the on-disk session loadable and lifecycle = `Suspended` until explicit resume.
- See [Multi-Session Runbook](multisession-runbook.md) for the milestone-level breakdown.
