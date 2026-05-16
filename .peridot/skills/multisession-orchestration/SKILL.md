---
name: multisession-orchestration
description: Wire and verify Peridot's multi-session runtime. Use when working on SessionRouter, concurrent agent loops, per-session CancelToken, workspace isolation (Shared / Worktree / Subprocess), session persistence (`MemoryStore::SessionRecord`), or the TUI tab bar swap.
---

# Multi-Session Orchestration

## Surfaces in play
- `peridot-cli/src/session_router.rs` — `SessionRouter`, `SessionHandle`, `WorkspaceIsolation`, `ActiveSession` (data model landed, live spawn pending).
- `peridot-core/src/cancel.rs` — `CancelToken` (`Arc<AtomicBool>`-based, cheap clone, cooperative).
- `peridot-core/src/agent.rs` — `HarnessAgent::set_cancel_token`; loop start checks `is_cancelled()` and emits `Interrupted { stage }`.
- `peridot-memory/src/lib.rs` — `SessionRecord`, `SessionLifecycle`, atomic blob persistence under `.peridot/sessions/<id>/`.
- `peridot-tui/src/session_directory.rs` — `SessionDirectoryItem`, `render_tab_bar`, `cycle_foreground`.
- `peridot-cli/src/main.rs` — adapter for new `AgentRunEvent` variants (`TurnEnded`, `PlanUpdated`, `BudgetUpdated`, etc.).

## Invariants
1. **One `CancelToken` per session.** Foreground Esc cancels only the foreground session; background sessions must own independent tokens.
2. **Event multiplex carries `(session_id, TuiRuntimeEvent)`.** The TUI enum itself stays unchanged; the router fans events in keyed by id.
3. **Workspace isolation.** Default foreground = `Shared`. Background `/session new --bg` or `/teammate` defaults to `Worktree(branch)` via `GitManager::add_worktree`. Two `Shared` sessions on the same cwd is a warning condition.
4. **Persistence is atomic.** All blob writes use `tempfile + rename`; partial writes must not corrupt `tui_state.json` or `context.bin`.
5. **JoinHandle panic isolation.** A panicking agent loop marks its session `SessionLifecycle::Failed` and never tears down the router.
6. **Backpressure.** Event channels use bounded `mpsc::channel(1024)` with oldest-drop or attention-flag fallback — never unbounded.
7. **TuiState swap is read-snapshot.** Foreground switch acquires `try_read` with timeout; the writer (`SessionRouter::tick`) is single-threaded.
8. **Cost/token aggregation.** Per-session totals + workspace aggregate are tracked separately; the UI labels them explicitly to prevent confusion.

## Build order (next milestones)
1. **PR-route-1 — spawn + event multiplex.**
   - Replace the single agent task in `peridot-cli/src/main.rs` with `SessionRouter::spawn_session(task, isolation)`.
   - Convert event delivery to `(String, TuiRuntimeEvent)` tuples; the TUI's `apply_runtime_event` consumes only the foreground tuple, the rest update `SessionDirectoryItem` stats.
   - Wire `/session new` / `/session switch` / `/session close` to router methods (slash commands already parsed in `peridot-core/src/slash.rs`).
2. **PR-route-2 — worktree isolation.**
   - `WorkspaceIsolation::Worktree(branch)` adds a `GitWorktree` to `SessionHandle`. Cleanup on `SessionLifecycle::Done|Failed`.
   - Warn on duplicate `Shared` cwd.
3. **PR-route-3 — persistence round-trip.**
   - Throttle `MemoryStore::save_session_blob` to 1s.
   - `peridot session resume <id>` reconstitutes `TuiState` + `peridot-context` from blobs; agent loop continues where it left off.
4. **PR-route-4 — subagent fan-in.**
   - `/fork` and `/teammate` route through `LocalSubAgentRunner` with `parent_id` set on the child `SessionHandle`.
   - Child events propagate to parent transcript via the subagent tree widget.
5. **PR-route-5 — attention notifier.**
   - `pending_attention` toggles when a background session emits `ApprovalRequested` or `AskUser*`.
   - Status bar surfaces `🔔 N other sessions need attention` (i18n `PhraseKey::SessionsNeedAttention`).

## Testing playbook
- **Concurrent loops**: spawn two sessions with deterministic mock-response files; assert event isolation (no leak across `session_id`).
- **Foreground swap**: switch foreground while background is mid-tool; assert TUI shows the new session's transcript and the prior session keeps ticking.
- **CancelToken scope**: Esc on foreground must not cancel background.
- **Workspace isolation**: two `Shared` writes to the same path collide → warning; `Worktree` isolated writes succeed independently.
- **Persistence round-trip**: save → kill process → resume; `TuiState` + transcript identity preserved.
- **JoinHandle panic**: induce panic in mock tool → router stays alive, session lifecycle = Failed.

## Out of scope for the first wiring pass
- Cross-session memory sharing.
- Network-replicated sessions (move to another machine).
- Container-level workspace sandboxing.
- OS-native notifications (status bar `🔔` is sufficient).

## When this skill applies
Pick this skill for any change that crosses the foreground/background boundary, touches `SessionRouter`, or persists/restores session state. Pair it with `ratatui-tui-qa` whenever the change also alters TUI rendering.
