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

### M3 — Persistence round-trip + crash recovery (landed)
- TUI loop calls `on_persist(&TuiState)` every tick; the CLI host throttles to one snapshot per second per session and writes `tui_state.json` atomically under `<project_root>/.peridot/sessions/<id>/` via `save_session_blob`.
- Every snapshot also updates `SessionRecord` (lifecycle / tokens / cost / turns / last_task) via `MemoryStore::save_session_record`.
- `peridot --resume <id>` now rebuilds the full foreground `TuiState` from the persisted blob when the run starts interactively; headless `--resume` keeps the existing prompt-injection behaviour.
- Startup scan downgrades any `SessionRecord` still marked `Running` to `Suspended` and surfaces the ids in the welcome transcript with the `peridot --resume <id>` hint.
- Context blob round-trip is wired end-to-end: the agent loop writes
  `<sessions_root>/<id>/context.bin` atomically after every turn via
  `ContextManager::snapshot_entries`, and `run_task_with_events` restores those
  entries through `ContextManager::restore_entries` before the next loop starts.
  `peridot --resume <id>` therefore reconstitutes both the TUI surface and the
  underlying conversation context.

### M4 — Subagent fan-in (`/fork`, `/teammate`, `/worktree`) (landed)
- `/fork`, `/teammate`, and `/worktree` register the new session with
  `SessionHandle.parent_id = current foreground id` and the
  `SessionDirectoryItem.kind` set to `fork` / `teammate` / `worktree` so the
  tree-shaped side panel can render the child under its parent.
- `TuiState::record_background_event` automatically promotes child events into
  `SubagentMonitorItem` entries (id + parent_id + tokens + kind + task)
  whenever the child's `parent_id` matches the foreground session — the
  parent transcript reflects child progress without forcing a tab swap.
- `/session close <child>` reuses the M2 worktree cleanup flow so cancelling
  a teammate / worktree subagent leaves no orphan directories or branches.
- Child sessions now inherit their parent's conversation. When
  `/fork`, `/teammate`, or `/worktree` spawns a child, `inherit_parent_context`
  copies `<sessions>/<parent>/context.bin` to `<sessions>/<child>/context.bin`
  before the agent loop starts, so `run_task_with_events` restores the parent's
  context entries on the first turn. Parents with zero completed turns leave
  the child with an empty context (silent no-op), matching the previous
  behaviour for that edge case.

### M13 — Cross-session transcript search (landed)
- `peridot session search <query> [--session <id>] [--limit N]` walks every persisted session under `<sessions_root>` (or just the one named by `--session`), loads its transcript via the M10 helper, and prints every entry whose text contains the substring (case-insensitive).
- Text output lists each hit as `<session>[<index>] <kind> <text>`; JSON output returns `{ "query", "total", "hits": [...] }` so downstream tooling can paginate or filter further. `--limit` short-circuits the walk after N matches to keep large workspaces responsive.

### M12 — Session list/show carry SessionRecord (landed)
- `peridot session list` now joins each `SessionSummary` with the matching `SessionRecord` written by M3's throttled persistence path. Text output appends `status / tokens / cost / turns`; JSON output nests the full record under each entry. Sessions without a record fall back to the previous summary-only view, so behaviour stays backwards compatible.
- `peridot session show` mirrors the same join with a more readable multi-line layout (status, workspace, tokens, cost, turns, optional worktree branch and last task). JSON output exposes the same nested shape for tooling.

### M11 — Live transcript tail (landed)
- `peridot session tail <id> [--from-now] [--interval-ms N]` prints the existing `transcript.ndjson` and then polls the file at `interval_ms` (default 200 ms, floored to 50 ms), printing every new line as it arrives with the same five-marker vocabulary the TUI uses. Ctrl+C terminates the watcher; no special signal handling needed.
- `--from-now` skips the existing journal and only prints entries written after the watch starts, useful when attaching mid-run.
- File truncation (e.g. by an external tool rotating the journal) resets the offset to 0 instead of stalling, so a fresh journal still streams correctly.

### M10 — Replay ndjson fallback (landed)
- `peridot session replay` now prefers the canonical `tui_state.json` snapshot but transparently falls back to `transcript.ndjson` when the snapshot is missing — which happens when a process was killed before the throttled `on_persist` could write but the per-tick M9 append already captured every entry. The result is that even uncleanly terminated sessions stay reviewable.
- The fallback parses ndjson line-by-line via `serde_json::from_str`, skipping blank lines and reporting the offending line number on bad payloads so a corrupted file points the operator at the right spot.

### M9 — Incremental transcript ndjson journal (landed)
- The on_persist callback now appends every newly observed `TranscriptEntry` to `<sessions_root>/<id>/transcript.ndjson` on every tick (no throttle). Per-session counts live in a `HashMap<String, usize>` inside the closure, so foreground swaps pick up the right entries for the right session.
- Append happens with `OpenOptions::append`; serde_json serialises each entry on its own line so external tools can `tail -f`/`jq` the live transcript without parsing JSON arrays.
- Failures (missing directory, write error) silently no-op so the UI thread never blocks on disk. The throttled `tui_state.json` snapshot from M3 keeps the canonical "load entire state" path.

### M8 — Per-session LLM provider override (landed)
- New `/provider <name>` slash command (also exposed in the slash picker / `/help`) records an explicit provider on `state.header.provider`. The status bar surfaces it as `provider <name>` in the metrics line.
- `apply_session_command`, submit, and approve callbacks all clone the project config and replace `auth.primary` with the session's provider before calling `spawn_tui_agent_run`, so concurrent sessions can run on different providers (e.g. one on `claude-api`, another on `openai-api` or `openrouter-api`) without mutating shared state.
- `HeaderState.provider` carries `#[serde(default)]`, so existing saved sessions resume with `None` (fall back to the config default).

### M7 — Replay journal CLI (landed)
- `peridot session replay <id> [--last N] [--step]` deserialises the persisted `tui_state.json` for that session and dumps the transcript entries with the same five-marker vocabulary the TUI uses (`▸ ◆ ❯ ✔ ✘ · ⚠ ? — …`). `--output json` returns the entries as a structured payload for tooling, and `--step` pauses for `Enter` between entries (type `q` to bail out early) so a session can be reviewed beat-by-beat.
- Reuses the on-disk format already written by the M3 throttled persistence path, so no new write codepath was needed.

### M6 — Per-session TuiState swap (landed)
- `run_interactive_with_events` now keeps a per-session `HashMap<String, TuiState>` of stashed states; every time `state.current_session_id` diverges from the foreground that was last rendered (Ctrl+T, Ctrl+W, `/session switch`), `swap_foreground_state` hot-swaps `state` so the visible transcript, plan, header counters, and active stream all jump to the new session.
- The latest `sessions` directory is always copied from the master view into the swapped-in state, so the tab bar stays consistent regardless of which session is foreground.
- Background sessions receive `apply_runtime_event` against their own stashed state in addition to the master `record_background_event` counter update — once the user swaps to them, the recorded transcript is already populated.
- Tests: round-trip `swap_foreground_state` between two sessions preserves each transcript and confirms the helper no-ops when target == previous.

### M5 — Attention notifier (landed)
- `TuiState::pending_attention_count()` reports how many non-foreground
  sessions are flagged `pending_attention`. The status bar renders
  `⚠ N{suffix}` using the new
  `PhraseKey::StatusSessionsAttentionSuffix` (En: " sessions need attention",
  Ko: "개 세션이 응답 대기 중") so the count flows through the existing
  i18n table.
- `render_text_snapshot` mirrors the indicator on a dedicated `attention:`
  line so headless previews and tests assert the message.
- Foreground swap from M1 already clears the `pending_attention` flag, so
  the indicator self-resolves once the user reads the background session.
- An optional `os-notify` cargo feature enables OS-level desktop notifications
  via `notify-rust`. When the feature is on, every background-session
  `ApprovalRequested` event fires a `Peridot: session needs attention` desktop
  notification carrying the gated tool's reason. The feature is off by default
  so the bare workspace build stays free of D-Bus / dbus / zbus link
  dependencies; `cargo build -p peridot-cli --features os-notify` opts in.

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
