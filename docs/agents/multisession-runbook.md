# Multi-Session Runbook

This runbook tracks the work that turns the multi-session scaffolding landed in PRs 8–11 into a fully live runtime. The data model, slash commands, persistence schema, and tab bar all already exist on `main`; what remains is wiring the foreground/background swap, concurrent agent loops, and subagent fan-in.

## Current state on main (after the 2026-05 TUI overhaul)

- Single foreground agent loop spawned in `peridot-cli/src/main.rs`.
- `SessionRouter` / `SessionHandle` types exist (`peridot-cli/src/session_router.rs`) but no live spawn or event multiplex yet.
- `CancelToken` plumbed through `HarnessAgent::set_cancel_token`; Esc routes through it and emits `AgentRunEvent::Interrupted`.
- `MemoryStore::SessionRecord` persists `tui_state.json` / `context.bin` per session atomically; `peridot session save/show/resume/delete` round-trips.
- Slash commands parsed but mostly no-op for `/session new`, `/session switch`, `/session close`, `/fork`, `/teammate`, `/worktree`.
- TUI `SessionDirectoryItem` + tab bar renders multiple sessions; `Ctrl+T`/`Ctrl+W` cycle foreground; mascot bubble reflects the foreground state only.
- 70 unit tests cover the surface, 0 ignored, 0 flaky.

## Milestones

### M1 — Live SessionRouter spawn + event multiplex
- Replace the single `tokio::spawn(run_agent_loop_with_events)` in `peridot-cli/src/main.rs` with `SessionRouter::spawn_session(task, isolation)`.
- Change the event channel to carry `(String session_id, TuiRuntimeEvent)`; the TUI listener filters by `foreground_id` for `apply_runtime_event`, and updates `SessionDirectoryItem` counters for everything else.
- Hook `/session new`, `/session switch`, `/session close`, `/session list` to router methods.
- Tests: two concurrent sessions, deterministic mock-response files, assert event isolation and foreground-only `TuiState` mutation.

### M2 — Workspace isolation
- `WorkspaceIsolation::Worktree(branch)` materialises a `GitWorktree` via `peridot_git::GitManager::add_worktree`.
- Background sessions default to `Worktree`; foreground = `Shared`.
- Warn (status bar + transcript notice) when two `Shared` sessions target the same cwd.
- Cleanup worktree on `SessionLifecycle::Done|Failed`.
- Tests: parallel writes — two `Shared` collide, two `Worktree` succeed.

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
