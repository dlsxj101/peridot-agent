# Multi-Session Runbook

This runbook tracks Peridot's multi-session runtime: foreground/background
agent loops, per-session persistence, worktree isolation, subagent fan-in,
and the smaller CLI/TUI surfaces that make long-lived sessions inspectable.
The core runtime milestones are landed on `main`; the remaining work is
polish and hardening around session ergonomics.

## Current state on main

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
- TUI tab bar shows only the current foreground session title (single-session
  display); `Ctrl+T`/`Ctrl+W` cycle foreground; mascot bubble reflects the
  foreground state. Session list is shown in the transcript on startup when
  multiple sessions exist.
- LLM-generated session titles: after the first successful agent run, a
  lightweight LLM call (main model, `ReasoningEffort::Off`) generates a
  3–8 word title that replaces the placeholder.
- Startup reconciliation handles unclean shutdowns: sessions still marked
  `Running` are downgraded to `Suspended`; clean Peridot-managed worktrees are
  removed; dirty worktrees are preserved and surfaced as TUI / VS Code warnings.
- The landed milestone log below is intentionally append-only so older
  release notes still map to the implementation sequence.

Remaining scoped polish: none currently tracked in this runbook.

## Milestones

### M1 — Live SessionRouter spawn + event multiplex (landed)
- Replaced the single `tokio::spawn(run_agent_loop_with_events)` in `peridot-cli/src/main.rs` with router-driven `spawn_tui_agent_run(session_id, ...)`.
- Event channel now carries `(String session_id, TuiRuntimeEvent)`; the TUI listener filters by `current_session_id` for `apply_runtime_event` and updates `SessionDirectoryItem` counters for the rest via `TuiState::record_background_event`.
- `/session new`, `/session switch`, `/session close`, `/fork`, `/teammate`, `/worktree` all flow through the router instead of leaving "wiring pending" notices.

### M2 — Workspace isolation (landed)
- `/teammate` and `/worktree <branch>` materialise a real `git worktree` under `<workspace_root>/.peridot/worktrees/<session_id>` via `peridot_git::GitManager::add_worktree`. Branch defaults to `peridot/teammate-<session_id>` when not specified.
- `/session close` (and Esc-driven cancel) calls `GitManager::remove_worktree` so the worktree is torn down even when the agent loop ends abnormally.
- Two `Shared` sessions on the same cwd raise a transcript warning so silent file-write collisions are visible.
- Later M39 startup reconciliation handles process crashes by suspending stale records, removing clean leftover worktrees, and preserving dirty ones with a warning.

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

### M39 — Stale worktree reconciliation (landed)
- TUI startup and daemon `peridot.status` both call the shared worktree
  reconciler before reporting session state.
- Any `SessionRecord` still marked `Running` after a previous process exited is
  saved as `Suspended`, matching the existing resume semantics.
- If that stale record points at a Peridot-managed worktree under
  `.peridot/worktrees/`, the reconciler removes it only when `git status` is
  clean. Dirty worktrees are preserved with a warning so uncommitted operator or
  agent changes are never discarded.
- Missing worktree paths are treated as already cleaned: git worktree metadata
  is pruned best-effort and the record is moved back to the shared workspace.

### M38 — `peridot session list --status <state>` filter (landed)
- `peridot session list` now accepts `--status idle|running|suspended|done|failed` (case-insensitive). The match uses `SessionRecord.status` so sessions without a record are dropped from the filtered view (they would not match any lifecycle anyway).
- An unknown value is a hard error so the operator never sees a silently empty list because of a typo.

### M37 — `peridot session count` lifecycle breakdown (landed)
- One-shot tally of `SessionRecord`s grouped by `SessionLifecycle`: total / idle / running / suspended / done / failed. Useful for "is anything still in flight" or "do I have stale Running records the startup scan missed" without paging through `peridot session list`.
- `/session count` exposes the same breakdown inside the TUI and through the
  daemon-backed VS Code composer, so editor clients no longer need a separate
  terminal command for lifecycle counts.

### M36 — Input box title carries character count (landed)
- Once the user starts typing, the input box's border title reads `" N chars "` (using the Unicode character count, so emoji/multibyte input is counted correctly). An empty buffer keeps the box title blank so the idle state stays clean.

### M35 — `/help` regenerates from slash catalog (landed)
- The `/help` slash now renders one line per `SlashCommandSpec` returned by `slash_command_catalog()`: `<name> <arg_hint>  ·  <description> [<category>]`. Newly registered slashes therefore appear in `/help` automatically — no more drift between the catalog and the help text.
- Output remains a single `push_transcript` so the existing transcript-wrap logic handles long help lists.

### M35b — Editor slash catalog sync (landed)
- The daemon exposes the same TUI slash catalog through `session.command_catalog`.
- The VS Code extension fetches that catalog on activation and whenever the
  daemon status refreshes, then passes it into the webview state. The sidebar
  and webview no longer carry duplicated command arrays.
- Editor session-control slashes use daemon `session.command` results first and
  apply local UI updates only after success, keeping mode / permission / model /
  provider / committee / goal control state aligned with the TUI command
  semantics.
- `/branch` without arguments returns a structured `branch_picker` result for
  editor clients; the extension renders it as a picker and dispatches
  `/branch turn <id>` when the operator selects a turn.

### M35 — `/session resume <id|title>` interactive parity (landed)
- `session_resume_task_text` is now shared between CLI resume handling and interactive slash commands, so `peridot session resume <id>`, TUI `/session resume <id|title>`, and VS Code daemon `session.command` all produce the same continuation prompt.
- The TUI resolves visible session ids/titles first and starts the continuation in the current foreground session. The VS Code daemon resolves persisted titles through `session_list_result` and returns a `start_task` result, reusing the extension's existing automatic dispatch path.

### M34 — `peridot version --detailed` (landed)
- Bare `peridot version` still prints `peridot <semver>` for backwards compatibility with scripts that grep the first token.
- `peridot version --detailed` adds three indented follow-up lines: `target: <os>`, `arch: <arch>`, and `profile: <release|dev>` when the binary was built with `CARGO_BUILD_PROFILE` propagated. Helpful when triaging "which binary is the operator running" against a release vs a local dev build.

### M33 — `peridot session show --transcript-tail N` (landed)
- `peridot session show <id> --transcript-tail N` prints the most recent N transcript entries (kind marker + text) under a `transcript (last N):` header, reusing the M10 load-with-fallback helper so even sessions that only have `transcript.ndjson` render the tail. JSON output exposes them under `transcript_tail: [{ kind, text }]`.
- Pairs with `--notes-tail N` (M30) so a single `show` invocation can answer "what happened recently in this session" without follow-up replay calls.

### M32 — Status bar carries active subagent count (landed)
- `render_status_metrics` appends `subagents N` when one or more entries in `state.subagents` have status `running` or `starting`. Done / failed subagents are excluded so the count means "in-flight work" rather than "lifetime spawn count" — useful when a TUI has spawned several `/fork` or `/teammate` sessions and the operator wants a quick activity gauge from the bottom bar without opening the side panel.

### M31 — `peridot agents show` carries path and rule count (landed)
- Text output now leads with `# <path> (<N> non-blank lines)` so the operator can tell exactly which instruction file is being read (`AGENTS.md` vs `CLAUDE.md` vs `.peridot/AGENTS.md` vs `.github/copilot-instructions.md`) and how many real rules are inside it. JSON output adds a matching `rule_count` field.

### M30 — `peridot session show --notes-tail N` (landed)
- `peridot session show <id> --notes-tail N` prints the most recent N notes from `notes.ndjson` inline beneath the existing session/record block (text output) or under a `notes_tail` array (JSON output). Pairs with M24's `notes_count` so the operator sees both "how many notes exist" and "what were the latest ones" in a single call.

### M29 — `peridot config models` catalog (landed)
- Prints the two configured model names from `PeridotConfig.models`: `main` (used by `HarnessAgent`) and `goal_checker` (used by goal mode). JSON output mirrors the same fields. Pairs with M27's `peridot config providers` so an operator can introspect both halves of "what runs where" without `peridot config show`'s full dump.

### M28 — `/info` slash one-shot session summary (landed)
- New `SlashCommand::Info` (no argument). When typed inside the TUI, prints a single transcript line that bundles `session id · workspace · model · provider · mode · permission · turn · tokens · cost` so an operator can confirm the entire session context without combining `/cost`, status bar, and tab bar.
- Picker advertises the command in the same `session` category as `/cost`, `/note`, and `/provider`.

### M27 — `peridot config providers` catalog (landed)
- Lists the four live provider keys the CLI accepts (`claude-api`, `openai-api`, `openrouter-api`, `openai-oauth`) with short descriptions and marks the one currently set as `auth.primary` in the project config.
- JSON output mirrors the same shape (`{ active, providers: [{ name, description, active }] }`) so tooling can pick a sensible value before calling `peridot config set auth.primary <name>`.

### M26 — `/cost` slash includes model / provider / turn (landed)
- Re-renders the `/cost` notice in the transcript as `cost: $X · tokens: T · cache: H% · model: M · provider: P · turn: N` so a quick check answers "where am I, what am I running, and how much has it cost" in one glance.
- Provider falls back to `default` when no explicit `/provider` is in effect; turn is `state.current_turn` (0 before the first turn).

### M25 — `session note list --last N` (landed)
- `peridot session note <id> list` now accepts `--last N` to print only the most recent N entries from `notes.ndjson`. JSON output adds a `total` field next to `notes` so tooling knows when the slice is truncated; text output appends a "... showing X of Y notes; drop --last for the full list." footer in the same case.

### M24 — `peridot session show` carries notes summary (landed)
- `peridot session show <id>` now reads `notes.ndjson` (when present) and reports the note count plus the most recent note's body inline. Text output: `  notes: <count>  (<last text>)`; JSON output: `notes_count`, `last_note`.
- Sessions without a notes file render exactly as before, so the addition is backwards compatible.

### M23 — Status bar carries turn count (landed)
- `render_status_metrics` appends `turn N` once `state.current_turn` is greater than zero (i.e. the first `TurnStarted` event has fired). Idle sessions and freshly opened ones keep the metric hidden so the bar stays clean.

### M22 — `peridot session locate <id>` utility (landed)
- Prints the absolute path of `<project_root>/.peridot/sessions/<id>` along with whether it currently exists on disk. JSON output exposes the same shape (`{ id, path, exists }`).
- Useful for shell pipelines (`(peridot session locate id)/transcript.ndjson`) and for confirming where M16's export will source its files from before running.
- `/session locate <id|title>` exposes the same path lookup to interactive clients. TUI prints the resolved directory path, while VS Code receives a structured `session_locate` command result with a path row.

### M21 — `/note` slash inside the TUI (landed)
- New `SlashCommand::Note(String)` and `/note <text>` slash entry. Inside the TUI, the slash queues the body onto `TuiState.pending_notes` and prints a transcript line so the user can see the note landed.
- `run_interactive_with_events` now hands the host a mutable `&mut TuiState` via `on_persist`. The CLI host's persist closure drains `pending_notes` every tick and appends one `{ts, text}` JSON line per note to the foreground session's `notes.ndjson`, matching the M20 file format. `peridot session note <id> list` then surfaces both CLI- and TUI-added notes uniformly.

### M20 — Operator notes per session (landed)
- `peridot session note <id> add <text>` appends a `{ts, text}` JSON line to `<sessions>/<id>/notes.ndjson` (created on demand) so an operator can annotate sessions without touching the transcript.
- `peridot session note <id> list` prints `[unix_ts] text` lines in chronological order (or returns `{ id, notes: [...] }` under `--output json`).
- `peridot session note <id> clear` removes `notes.ndjson` if it exists. Each notes file is independent of `tui_state.json` / `transcript.ndjson`, so a session can be exported / imported / pruned without touching the notes.

### M19 — Workspace label in status bar (landed)
- `HeaderState.workspace_label: Option<String>` (#[serde(default)]) carries the project root's basename. `peridot-cli` populates it from `project_root.file_name()` at TUI startup (and on resume) so the status bar reads `workspace <name>` next to mode/permission.
- The label is per-session, so a `/teammate` worktree session that targets a different checkout will naturally show a different `workspace` label than the foreground session.

### M18 — AGENTS.md hot reload (landed)
- `HarnessAgent::set_agents_md_path` lets the host point the agent at the AGENTS-style instruction file the project resolves (`.peridot/AGENTS.md`, `AGENTS.md`, `CLAUDE.md`, or `.github/copilot-instructions.md`, in that priority order). `peridot_project::locate_agents_md` resolves this from the project root.
- Every turn the agent loop calls `refresh_agents_md`: it compares `(modified_unix, len)` against the last-seen fingerprint and, when the file has been edited mid-run, re-reads the content, appends a trusted `ContextEntry::PlanReminder` carrying the new rules into the context, and emits `AgentRunEvent::AgentsMdLoaded` so the TUI side panel reflects the refresh. The first turn after `set_agents_md_path` always fires the inject because the signature starts as `None`.
- The check no-ops silently when the file is missing or unreadable, so removing `AGENTS.md` mid-run never blocks the agent.

### M17 — Session import (landed)
- `peridot session import <dir> [--id <id>] [--force]` is the inverse of M16's export: it copies every file from the source directory into `<project_root>/.peridot/sessions/<id>/`. The id defaults to the source directory's base name unless `--id` is provided.
- After the copy, the imported `tui_state.json` (if any) is deserialised once to register a `SessionSummary` carrying the session's `last_task` so `peridot session list` / `peridot session show` / `peridot --resume` see the imported session immediately.
- `--force` clears the destination directory before importing; otherwise an existing session id is a hard error so the user never silently merges files from two different exports.

### M16 — Portable session export (landed)
- `peridot session export <id> --out <dir> [--force]` copies every file inside `<sessions_root>/<id>/` (currently `tui_state.json`, `transcript.ndjson`, `context.bin`, plus any subdirectory the agent writes) into a fresh destination directory. Combine with `tar`/`zip` to ship a session to another machine.
- `--force` clears the destination directory first; otherwise an existing path is a hard error so the user never accidentally overwrites a different export.
- Text output prints the file list; JSON output returns `{ id, source, destination, files: [...] }` for tooling that wants to verify the copy.

### M15 — AskUser freeform multi-line input (landed)
- `Shift+Enter` while answering an `AskUser::FreeForm` prompt now inserts a literal newline instead of submitting. `Ctrl+J` does the same (some terminals deliver `Shift+Enter` as bare Enter, so the chord is the cross-terminal fallback). `Enter` without modifiers still submits.
- Existing `Backspace` / `Ctrl+H` deletion logic stays unchanged; rendering of the freeform value already wraps long lines because `ask_user` panels use the same `Wrap { trim: false }` policy as the transcript.

### M14 — Bulk prune of finished sessions (landed)
- `peridot session prune [--status <state>] [--older-than-days N] [--dry-run]` walks every `SessionRecord` and removes the ones matching all filters: `delete_session` (SQLite summary), `delete_session_record` (SQLite record), and `remove_session_dir` (on-disk blobs / ndjson).
- `--status` accepts `idle | running | suspended | done | failed`. Unknown values fail loudly so a typo never accidentally targets the wrong cohort.
- `--older-than-days N` skips sessions whose `updated_at_unix` is newer than `N * 86_400` seconds before now.
- `--dry-run` prints what *would* be removed and returns; combine with `--status` / `--older-than-days` to audit before sweeping.

### M13 — Cross-session transcript search (landed)
- `peridot session search <query> [--session <id>] [--limit N]` walks every persisted session under `<sessions_root>` (or just the one named by `--session`), loads its transcript via the M10 helper, and prints every entry whose text contains the substring (case-insensitive).
- Text output lists each hit as `<session>[<index>] <kind> <text>`; JSON output returns `{ "query", "total", "hits": [...] }` so downstream tooling can paginate or filter further. `--limit` short-circuits the walk after N matches to keep large workspaces responsive.
- `/session search <query>` exposes the same backend to interactive clients. TUI queues a session command and prints the first 50 hits in the transcript; VS Code receives a structured `session_search` daemon result so the extension can render the hits without duplicating search logic.

### M12 — Session list/show carry SessionRecord (landed)
- `peridot session list` now joins each `SessionSummary` with the matching `SessionRecord` written by M3's throttled persistence path. Text output appends `status / tokens / cost / turns`; JSON output nests the full record under each entry. Sessions without a record fall back to the previous summary-only view, so behaviour stays backwards compatible.
- `peridot session show` mirrors the same join with a more readable multi-line layout (status, workspace, tokens, cost, turns, optional worktree branch and last task). JSON output exposes the same nested shape for tooling.
- `/session show <id|title>` exposes the persisted detail view to interactive clients. TUI resolves visible session targets when possible and prints the summary in the transcript; VS Code receives a structured `session_show` daemon result with the same lifecycle, usage, workspace, worktree, and notes-summary fields.

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

### M40 — Parent-child `agent_message` queue (landed in v0.6.0)
- `SessionHandle.inbox: Arc<Mutex<VecDeque<InboxMessage>>>` carries a per-session FIFO message queue alongside the existing cancel token and compaction flag.
- `RouterMessageBus` implements `peridot_tools::AgentMessageBus` against a shared `Arc<Mutex<SessionRouter>>`. It exposes `send_to_parent`, `send_to_child`, `drain_inbox`, plus a `with_current_session(id)` clone that binds the bus to a specific sender.
- `HarnessAgent::set_message_bus` + `set_session_id` install the bus into every `ToolContext` (so `agent_message` calls actually route) AND drain the harness's own inbox at the start of every turn. Drained entries become trusted `[peer message from <id>]` `PlanReminder` context entries — the model sees coordination signals on its very next LLM call without an explicit tool round-trip.
- `InnerLoopSubAgent` deliberately does NOT propagate the bus to grandchild `ToolContext` instances. This caps `agent_message` depth at one, matching the existing fork-bomb safety policy for `agent_delegate`.
- Sibling addressing is rejected: `send_to_child(from, target, _)` checks that `target.parent_id == from`. A subagent can only message its own direct children, never arbitrary peers.
- The new `agent_message` built-in tool ships in `peridot-tools/src/tools/agent.rs`. Without a bus attached (single-session run, headless, tests), it returns a polite noop result with a hint so the model can see why its note didn't deliver.

### M39 — Single-session tab bar + startup session list + LLM title generation (landed)
- Tab bar now renders only the current foreground session title instead of all sessions. `render_tab_bar_text` mirrors the same single-session display. Display-width-safe truncation at 48 columns preserves CJK safety.
- On startup, if two or more persisted sessions exist, a formatted session list (title + status, `>` for active) is pushed to the transcript as a notice so the operator sees what's available without a crowded tab bar.
- After the first successful `run_task_with_events` completes, a lightweight LLM call (`live_provider`, main model, `ReasoningEffort::Off`, max 30 tokens) generates a 3–8 word session title. The result flows through the new `TuiRuntimeEvent::SessionTitleUpdated { session_id, title }` variant and updates `SessionDirectoryItem.title` + sets `title_generated = true` to prevent redundant calls. Approval resumes skip title generation.

### M6 — Multi-session UX polish
- Per-session history (input recall) when swapping foreground. ✓ landed as
  M6-1: foreground swaps stash and restore the full per-session `TuiState`,
  including `input_history` and its cursor, so Up/Down recall never leaks
  prompts between sessions.
- Session search/picker (`Ctrl+T` chord) with prefix match + recency. ✓
  landed as M6-2: `Ctrl+T` opens a searchable session picker, ranks matches
  by latest activity, clears attention on the chosen foreground session, and
  keeps `Ctrl+W` as the quick cycle shortcut.
- Aggregate budget/cost line in the status bar, distinct from per-session totals. ✓
  landed as M6-3: multi-session status metrics show `all <tokens> tok / $<cost>`
  when background sessions add usage beyond the foreground session.

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
| Worktree leak when process crashes mid-run | startup / daemon status reconciliation suspends stale records, removes clean Peridot worktrees, and preserves dirty worktrees with a warning. |
| Cost double-counting across sessions | per-session totals + workspace aggregate stored separately, UI labels them. |
| Snapshot corruption on partial write | atomic `tempfile + rename`; never write to the final path directly. |

## Out of scope (until later)

- Cross-session memory sharing or context bleed.
- Network-replicated sessions (export/import a session to another machine).
- Mouse/touch input in the TUI.
- Container-level workspace sandboxing beyond git worktrees.
- Hot-swappable themes; the Peridot Night palette stays the default.
