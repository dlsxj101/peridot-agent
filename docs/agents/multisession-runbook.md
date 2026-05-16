# Multi-Session Runbook

This runbook tracks the work that turns the multi-session scaffolding landed in PRs 8–11 into a fully live runtime. The data model, slash commands, persistence schema, and tab bar all already exist on `main`; what remains is wiring the foreground/background swap, concurrent agent loops, and subagent fan-in.

## Current state on main (after the 2026-05 TUI overhaul + M1/M2)

- `SessionRouter` is live: each foreground/background session owns a real
  `SessionHandle` with its own `CancelToken`, and every event is multiplexed
  through a single `(session_id, TuiRuntimeEvent)` channel.
- `/session new|switch|close` actually spawn / swap / close sessions, and
  `/fork`, `/teammate`, `/worktree` register new agent loops.
- `WorkspaceIsolation::Worktree { branch }` is honored: `/teammate` and
  `/worktree` materialise a real `git worktree` under
  `<workspace_root>/.peridot/worktrees/<session_id>` (branch
  `peridot/teammate-<session_id>` by default) and tear it down on
  `/session close`. Two `Shared` sessions on the same cwd trigger a status
  warning so collisions are visible.
- Background events flow into `TuiState::record_background_event` —
  `SessionDirectoryItem` carries status / tokens / cost / pending_attention
  per session without polluting the foreground transcript.
- `CancelToken` is per-handle: Esc only cancels the foreground session.
- `MemoryStore::SessionRecord` persists `tui_state.json` / `context.bin`
  per session atomically; `peridot session save/show/resume/delete`
  round-trips.
- TUI tab bar (`SessionDirectoryItem`) renders multiple sessions; `Ctrl+T`/
  `Ctrl+W` cycle foreground; mascot bubble reflects the foreground state.
- 75 peridot-tui tests + 40 peridot-cli tests cover the surface,
  0 ignored, 0 flaky.

Outstanding for the next milestones: throttled persistence on every tick
(M3), `LocalSubAgentRunner` fan-in so `/fork` actually shares context with
its parent transcript (M4), and the attention notifier line (M5).

## Milestones

### M1 — Live SessionRouter spawn + event multiplex (landed)
- Replaced the single `tokio::spawn(run_agent_loop_with_events)` in `peridot-cli/src/main.rs` with router-driven `spawn_tui_agent_run(session_id, ...)`.
- Event channel now carries `(String session_id, TuiRuntimeEvent)`; the TUI listener filters by `current_session_id` for `apply_runtime_event` and updates `SessionDirectoryItem` counters for the rest via `TuiState::record_background_event`.
- `/session new`, `/session switch`, `/session close`, `/fork`, `/teammate`, `/worktree` all flow through the router instead of leaving "wiring pending" notices.

### M2 — Workspace isolation (landed)
- `/teammate` and `/worktree <branch>` materialise a real `git worktree` under `<workspace_root>/.peridot/worktrees/<session_id>` via `peridot_git::GitManager::add_worktree`. Branch defaults to `peridot/teammate-<session_id>` when not specified.
- `/session close` (and Esc-driven cancel) calls `GitManager::remove_worktree` so the worktree is torn down even when the agent loop ends abnormally.
- Two `Shared` sessions on the same cwd raise a transcript warning so silent file-write collisions are visible.
- Outstanding for the next milestone: handle process crash mid-run so leftover worktrees are pruned on startup.

### M3 — Persistence round-trip + crash recovery
- Throttle `MemoryStore::save_session_blob` calls to ≥1s between snapshots per session.
- `peridot session resume <id>` rebuilds `TuiState` + `peridot-context` from disk and continues the agent loop.
- Add a startup scan: any `SessionRecord` with `status == Running` is downgraded to `Suspended` and surfaced for explicit resume.
- Tests: persist mid-run → kill process → resume; verify transcript + plan + token totals match.

### M4 — Subagent fan-in (`/fork`, `/teammate`, `/worktree`)
- Route `/fork <task>` through `LocalSubAgentRunner::fork` with `parent_id` set on the spawned `SessionHandle`.
- `/teammate <task>` defaults to a `Worktree(<auto-branch>)` session and registers a tab.
- `/worktree <branch> <task>` is the explicit form.
- Child events propagate to the parent's subagent tree (`SubagentMonitorItem`), and a child completion writes a one-line summary back into the parent transcript.
- Tests: parent + child concurrent run, parent transcript receives child progress, child cancel via `/session close <child_id>` interrupts cleanly.

### M5 — Attention notifier
- `SessionDirectoryItem::pending_attention` toggles on `ApprovalRequested` or `AskUser*` for background sessions.
- Status bar surfaces `🔔 N other sessions need attention` via a new `PhraseKey::SessionsNeedAttention` arm (En/Ko).
- Foreground swap clears the flag.
- Optional: opt-in `notify-rust` OS notification behind a feature flag.

### M6 — Multi-session UX polish
- Per-session history (input recall) when swapping foreground.
- Session search/picker (`Ctrl+T` chord) with prefix match + recency.
- Aggregate budget/cost line in the status bar, distinct from per-session totals.

## Cross-cutting checklist for every multi-session PR

- [ ] `cargo fmt --all --check`
- [ ] `cargo clippy --workspace -- -D warnings`
- [ ] `cargo test --workspace`
- [ ] `cargo build --release -p peridot-cli`
- [ ] New `PhraseKey` arms added for every new visible string (both En and Ko).
- [ ] New `AgentRunEvent` variants round-trip through the CLI adapter into a `TuiRuntimeEvent` arm.
- [ ] New struct fields carry `#[serde(default)]`; new enum variants are last so old serialised data deserialises.
- [ ] `SessionRouter` mutations stay single-threaded; readers use `try_read` with bounded timeout.
- [ ] No unbounded channels: bounded `mpsc::channel(1024)` or ring-buffer with oldest-drop.

## Risks tracked

| Risk | Mitigation |
|---|---|
| Foreground swap deadlock on `Arc<RwLock<TuiState>>` | `try_read` with timeout + single-writer (`SessionRouter::tick`). |
| Background event backpressure | bounded channel + oldest-drop; attention flag conveys "you missed N events". |
| Worktree leak when process crashes mid-run | startup scan reconciles open worktrees against `SessionRecord` lifecycle. |
| Cost double-counting across sessions | per-session totals + workspace aggregate stored separately, UI labels them. |
| Snapshot corruption on partial write | atomic `tempfile + rename`; never write to the final path directly. |

## Out of scope (until later)

- Cross-session memory sharing or context bleed.
- Network-replicated sessions (export/import a session to another machine).
- Mouse/touch input in the TUI.
- Container-level workspace sandboxing beyond git worktrees.
- Hot-swappable themes; the Peridot Night palette stays the default.
