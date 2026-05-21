# Changelog

All notable changes to Peridot Agent are documented in this file.

The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

This is the first dedicated CHANGELOG entry; earlier releases (0.4.x – 0.5.x)
were documented inline in [PERIDOT_SPEC_v1.md](PERIDOT_SPEC_v1.md) and on
[GitHub Releases](https://github.com/dlsxj101/peridot-agent/releases). All
0.6.0 changes are additive — no breaking API removals.

---

## [0.8.9] — 2026-05-22

### Added — daemon-backed slash command RPC for editor clients

`peridot daemon` now exposes `session.command`, letting VS Code / Cursor
clients execute the same project-state slash commands as the TUI. The new RPC
handles branch snapshots and limbs, MCP list/add/remove/test, TODO scanning,
working-tree diff, checkpoint undo, context-top inspection, and live compact
requests. Running sessions share the compact flag with the harness loop; idle
editor sessions can still operate on their persisted context snapshots.

### Added — persistent editor sessions and structured command UI

The VS Code extension now stores open chat sessions, transcripts, daemon
session ids, queued prompts, and run options in workspace storage so an
Extension Host reload does not wipe the chat list. Daemon command results render
as structured branch/MCP/TODO/context/diff blocks instead of plain status text.

### Fixed — OAuth and packaging polish

ChatGPT OAuth now always surfaces a visible manual login link in the chat when
the browser handoff is attempted, and the VSIX package includes an MIT license
file to remove the publish warning.

### Migration notes

- Workspace 0.8.8 → 0.8.9.
- Extension package 0.5.13 → 0.5.14.
- No config keys changed.

---

## [0.8.6] — 2026-05-21

### Added — daemon session continuation for editor clients

`session.start` now accepts an optional `session_id`. When a VS Code / Cursor
client sends a follow-up prompt for an existing inactive session, the daemon
reuses that session id and reloads the previous context snapshot instead of
starting a disconnected conversation. New sessions are still created when the
client omits `session_id`, and active duplicate starts continue to return the
existing active id.

### Fixed — lossy text reads for invalid UTF-8 files

`file_read` now falls back to UTF-8 replacement decoding when a workspace text
file contains invalid byte sequences. The tool result includes the readable
content and marks the summary with `(invalid UTF-8 bytes replaced)` so clients
can keep working on Windows-created or mixed-encoding project files instead of
failing with `stream did not contain valid UTF-8`.

### Migration notes

- Workspace 0.8.5 → 0.8.6.
- No CLI flags or config keys changed.

---

## [0.8.0] — 2026-05-20

### Added — async daemon runtime plus real session control

`peridot daemon` now runs on tokio instead of the original synchronous
stdin loop. The daemon keeps Windows-friendly blocking stdin reads in a
dedicated bridge task, drains all outgoing JSON-RPC lines through one
async stdout writer, and shares daemon state across spawned session tasks
with `Arc<Mutex<...>>`. That keeps response and notification frames as
single, newline-delimited JSON values even when multiple sessions emit
events concurrently.

New JSON-RPC methods:

| Method | Result |
|---|---|
| `session.start` | Starts the real harness loop and returns `{ "session_id": "session-<pid>-<n>" }`. |
| `session.cancel` | Cancels an active session by id and returns `{ "cancelled": bool, "session_id": string }`. |

`session.start` accepts `task` plus optional `mode`, `permission`, and
`model` overrides. The daemon builds its static `PeridotConfig` and
`AgentTaskOptions` template once at startup, then clones and adjusts that
template for each new session. Each session emits a lightweight
`started` notification, forwards serialized `AgentRunEvent` values as
JSON-RPC `event` notifications, and ends with a `finished` or `error`
notification.

The existing v0.0.x extension checks (`peridot.version`, `peridot.echo`,
`shutdown`) remain unchanged, so Phase 0 clients keep working while Phase
1 clients can begin driving real agent runs over the same stdio channel.

### Migration notes

- Workspace 0.7.10 → 0.8.0.
- `tokio` workspace features now include `io-std` and `sync` for the
  daemon's async stdio and channel runtime.
- `approval.respond` and daemon-backed `ask_user` remain deferred to the
  next extension phase.

---

## [0.7.10] — 2026-05-19

### Changed — Peridot deer mascot redrawn at 16×16, eight mood-specific frames

The mascot lived at 8×8 with a 7-entry palette and one or two
frames per mood. v0.7.10 redraws it from a reference pixel-art
deer (tall paired antlers, big round head with two black eyes
flanking a pink nose, peridot gem at the chest, stocky body
ending in two brown hooves). Sprite is now 16×16 with a 9-entry
palette, but the rendered footprint stays at **8 cols × 4 rows
of terminal cells** — same as before — because each 2×2 sub-
pixel block is compressed into one cell via Unicode quadrant-
block glyphs (`▘▝▖▗▙▟▛▜▀▄▌▐█ `). Pixel detail is 4× higher in
the same screen real estate.

Per-mood frame design:

| Mood | Frames | What changes |
|---|---|---|
| Idle | 2 | Slow blink (pupils → highlight tone) |
| Thinking | 2 | Right-antler tip shifts inward (head tilt) |
| ToolRunning | 3 | Chest gem pulses dim → mid → bright |
| ApprovalWaiting | 1 | Pupils flanked by sparkle highlights (alert) |
| AskUser | 1 | Sparkle pixels on the head crown (raised ears) |
| Done | 2 | Hooves lift one row (happy bounce) |
| Failed | 1 | Antler branches collapsed + sad closed eyes |
| Interrupted | 1 | Antlers straight up + enlarged 2-cell pupils |

### Implementation notes

- `peridot-tui/src/mascot/frames.rs` defines a 9-colour palette
  (`peridot_palette`) tuned to the reference art: deep antler
  green, mid body green, light body highlight, a 3-step peridot
  gem (outer / core / sparkle), eye black, nose pink, hoof brown.
- Each frame is 16×16 `Pixel`s. The design rule is "≤ 2 distinct
  colours per 2×2 quadrant" so every cell maps cleanly to a
  quadrant glyph with one foreground + one background colour.
  When a frame breaks the rule the renderer picks the first two
  palette indices and falls back to those — sprite still renders,
  just a touch less faithful.
- `peridot-tui/src/mascot/render.rs` was rewritten around
  `quadrant_cell` (2×2 → glyph + fg + bg) and `quadrant_glyph`
  (4-bit mask → Unicode codepoint).
- Sprite-rendering test suite trimmed from the half-block layout
  to a quadrant-aware one: 5 tests covering "fills every cell",
  "transparent → reset colours", "solid → █", "mixed → correct
  quadrant glyph", "no-op when area too small".

### Migration notes

- No config or API surface changes for end users.
- Workspace 0.7.9 → 0.7.10.

---

## [0.7.9] — 2026-05-19

Four TUI / runtime UX fixes from live v0.7.8 use.

### Fixed — phantom caret blinked next to status bar while agent was working

`render.rs` always called `frame.set_cursor_position(...)`, so the
textarea caret was drawn every tick — even while the agent was
streaming and the operator hadn't started a new draft. On some
terminals the caret blink painted on top of the previous frame
made it look like a stray cursor was flashing next to the spinner
or the elapsed counter. Now the caret is suppressed while
`AgentRunStatus::Running` and `state.input` is empty. The moment
the user starts typing the caret returns; agent finishes → caret
returns. `crates/peridot-tui/src/render.rs`.

### Fixed — `/clear` did not actually clear the conversation

The old handler called `state.transcript.clear()` and stopped
there. The agent's `ContextManager`, the token / cost counters,
the plan steps, the side-panel stats, the pending input queue —
all of it stayed put, so the next message still recalled the
previous task and the cost meter kept climbing. Fixed two ways:

- TUI side: `TuiState::reset_for_clear` wipes every visible
  surface — transcript, activities, side-panel stats, plan,
  subagents, header tokens / cost / cache rate, active tools,
  streaming buffer, approval / ask-user panels, spinner, input
  queue.
- Host side: a new `SessionCommandEvent::ClearAndRestart` makes
  `peridot-cli` cancel the running agent, close the old session,
  delete its persisted context snapshot, and register a fresh
  session in the same workspace. The next user message starts
  with zero recall and zero token spend.

`crates/peridot-tui/src/state.rs`, `crates/peridot-tui/src/
input.rs`, `crates/peridot-cli/src/main.rs`.

### Fixed — Esc interrupt did not actually stop in-flight LLM calls

`CancelToken` was polling-only, and the only places that polled
it were the agent loop's turn boundary and `shell_exec`. The
LLM provider had no awareness of cancel at all, so an Esc press
during a streaming completion did nothing until the model
finished naturally — sometimes 10–30 seconds later. Added
`CancelToken::cancelled()` (an async future that polls every
50ms) and raced it against the streaming completion in
`stream_completion_with_chunks` via `tokio::select!`. When the
race resolves on the cancel side, the streaming future is
dropped (reqwest aborts the underlying connection) and the
agent loop surfaces an `Interrupted` event immediately. Esc
now feels as responsive as the spec suggests.

`crates/peridot-common/{Cargo.toml, src/cancel.rs}`,
`crates/peridot-core/src/{agent.rs, usage.rs}`.

### Fixed — `verify_build` ran `cargo build --workspace` on non-Rust projects

The verify tools hard-coded `cargo build --workspace`,
`cargo test --workspace`, and `cargo clippy --workspace -- -D
warnings` as their fallback when the model didn't pass a
`command` parameter. On Python / Node-only projects the tool
spawned `cargo`, hit `127: command not found`, and the auto-fix
loop blamed the operator's repo. Now each verify tool calls
`ProjectScanner::new().scan(project_root)` and uses
`profile.commands.{build,test,lint}` as the fallback; the
hard-coded cargo string is only the last-resort fallback when
no project markers can be inferred.

Bonus: `scanner.rs` learned to peek into common monorepo
sub-directories (`frontend`, `web`, `client`, `ui`, `app`,
`apps/web`, `apps/frontend`, `packages/web`, `packages/
frontend`, `backend`, `server`, `api`, `service`) when the
root has no language markers of its own. A Python backend at
the root + a Vite frontend under `frontend/` — exactly the
operator's repo that surfaced this bug — now resolves to
`cd frontend && npm run build`.

`crates/peridot-tools/Cargo.toml`,
`crates/peridot-tools/src/tools/verify.rs`,
`crates/peridot-project/src/scanner.rs`.

### Migration notes

- Workspace 0.7.8 → 0.7.9. No config or API surface changes for
  end users.
- `peridot-tools` now depends on `peridot-project`. No
  dependency cycle (`peridot-project` only depends on
  `peridot-common`).
- `peridot-common` now depends on `tokio` (was previously
  std-only). Needed for `CancelToken::cancelled()`.

---

## [0.7.8] — 2026-05-19

### Fixed — auto-grade looped forever on chat / Q&A turns

When `defaults.auto_grade_on_done = true` (the default), every
`agent_done` invocation was fed to the LLM grader. The grader's
system prompt only knows how to evaluate coding tasks — "Pass when
the change addresses the task" — so any non-coding turn (chat,
explanation, "do you remember our last conversation?") finished
with an empty `git diff HEAD` and got rejected with
"No change was provided to address the request". The
recommendations were folded back into context, the agent dutifully
re-answered, the next `agent_done` produced another empty diff,
and the loop repeated until `max_turns` ran out. Operators saw
the cascade as

```
⚠ recovery: auto-grade failed: No change was provided …
⚠ recovery: auto-grade failed: No changes were provided …
```

Fixed by short-circuiting the grader when the worktree diff is
empty: the gate now logs `[auto-grade] Skipped: no worktree
changes to grade` to the plan reminder, fires `AgentRunEvent::
Finished`, and exits cleanly. Code paths where the agent is
genuinely supposed to ship a change still see the grader (an
empty diff in those cases means the model wrongly claimed done
without editing anything — but the grader's reject-loop never
made progress on that scenario either, so we lose nothing).

`crates/peridot-core/src/agent.rs`.

### Added — `HarnessAgent::set_grader_diff_provider`

Internal-only hook so tests can pre-load a non-empty diff and
exercise the grader-rejection path that the empty-diff fast path
would otherwise skip. Production code never sets it; the default
(`None`) keeps the `collect_git_diff` call we've always had.

### Migration notes

- Pure bug-fix release. No config or API surface changes for
  end users.
- Workspace 0.7.7 → 0.7.8.

---

## [0.7.7] — 2026-05-19

Three more TUI papercuts from live v0.7.6 use on Windows.

### Fixed — textarea filled with raw escape sequences after `shell_exec`

After a `shell_exec` finished (e.g. `npm ci`, `cargo build`, vite),
the input textarea started receiving raw ANSI escapes
(`[A`, `[B`, `[5~`, `[6~`) instead of arrow / PageUp / PageDown
events. Two root causes:

1. **Child inherited the TUI's tty stdin.** `shell_exec` spawned
   child processes with no explicit `stdin` setting, so the child
   inherited the TUI's controlling terminal. Keystrokes raced
   between the child and the TUI input loop, and child libraries
   that send termios escape sequences to /dev/tty (spinner libs,
   npm progress, vite dev banner) reset the parent's keypad-mode
   on exit. Now `shell_exec` always sets
   `Stdio::null()` for child stdin — applies to both the cancel-
   token / timeout path and the legacy `output()` fast path.
   `crates/peridot-tools/src/tools/shell.rs`.
2. **TUI did not re-assert raw mode after a child returned.**
   Even with child stdin closed, a child can write termios escape
   sequences directly to its controlling terminal and corrupt the
   parent's state. Re-asserts `enable_raw_mode()` at the top of
   every event-loop tick when `is_raw_mode_enabled()` reports
   false. `enable_raw_mode` is idempotent, so the steady-state
   cost is one ioctl per tick. Applies to both `run_interactive`
   and `run_interactive_with_events`.
   `crates/peridot-tui/src/input.rs`.

### Fixed — cursor lagged behind the actual position when typing Korean / CJK

`render.rs` computed the textarea cursor X position with
`prefix.chars().count()`, which treats every Unicode scalar as one
terminal cell. CJK glyphs (한국어, 中文) and most emoji occupy two
cells, so the rendered caret fell behind the actual edit position
by one cell per wide glyph already on the line. Typing "안녕하세요"
would leave the caret hovering over the third character even though
the cursor index was at the end of the string. Switched to
`unicode_width::UnicodeWidthStr::width(tail)` so the caret's cell
position matches what the terminal is actually drawing.
`crates/peridot-tui/src/render.rs`.

### Migration notes

- Workspace 0.7.6 → 0.7.7. Pure bug-fix release, no API changes.
- Children that legitimately needed to read the operator's
  keystrokes (interactive REPLs invoked through `shell_exec`) now
  see immediate EOF on stdin. None of the in-repo helpers do, and
  agent shell commands are non-interactive by policy, so this is
  the right default. Add an opt-in flag if a future use case
  requires the old behaviour.

---

## [0.7.6] — 2026-05-19

Three Windows / OAuth bug fixes that surfaced during real-world v0.7.5
setup on a Windows 11 host. All three blocked the first-run experience
(`peri` -> "OpenAI OAuth direct / ChatGPT login" path) before any agent
turn could run.

### Fixed — OAuth URL truncated to first `&` on Windows

`open_browser` on Windows used to spawn `cmd /C start "" <url>` with
the URL passed as a regular Rust argument. `cmd.exe`'s internal
parser ignores `CreateProcess` arg quoting and re-splits the raw
command line, treating `&` as a command separator. OAuth URLs are
`&`-joined query strings, so the browser only ever received the
fragment before the first `&` ("https://auth.openai.com/oauth/authorize?response_type=code")
which OpenAI then rejected as a malformed authorize request. Fixed
by assembling the command line with `CommandExt::raw_arg` so the
URL lives inside its own pair of double quotes:
`cmd /C start "" "<url>"`. `crates/peridot-cli/src/commands/auth.rs`.

### Fixed — setup wizard's next prompt required Enter twice

After a successful OAuth callback the model picker
(`OpenAI OAuth main model: 1. gpt-5.5 ... Choose [1]:`) silently
swallowed the user's first keystroke. Root cause:
`wait_for_oauth_code` spawned a background stdin reader so the user
could paste the redirect URL as a fallback, but that reader's
`std::io::stdin().read_line()` blocked indefinitely and outlived
the listener path. When the local HTTP listener received the OAuth
callback and the function returned, the zombie reader was still
blocked on stdin — the next time the wizard called `read_line` from
the main thread, the user's `2` was consumed by the zombie (whose
channel `tx` had already been dropped) and the wizard saw nothing
until the user pressed Enter again, at which point the wizard
defaulted to choice 1. Removed the background reader; paste-fallback
remains in the path that already handles `TcpListener::bind`
failure. `crates/peridot-cli/src/commands/auth.rs`.

### Fixed — `400 No tool output found for function call` after a failed tool

When a tool errored (e.g. `file_read` on a missing path), the agent
loop appended the assistant's `tool_calls` entry to the conversation
but bubbled the `Err` to the recovery layer without appending the
matching `function_call_output`. The recovery layer added its plan-
reminder and looped, sending the now-malformed history back to
Responses-style providers (OpenAI Codex), which rejected it with
`400 No tool output found for function call <id>`. The user saw a
silent stall punctuated by repeated 400s. Fixed by synthesising a
failed `ToolResult` and appending it as the paired
`function_call_output` *before* bubbling the error — recovery
still runs (existing `recovery_message` plan-reminder still lands
in context), but the conversation stays well-formed for native-
tool-call providers. `crates/peridot-core/src/agent.rs`.

### Migration notes

- Pure bug-fix release; no API or config surface changes.
- Workspace version 0.7.5 → 0.7.6. Extension version stays at 0.0.1
  (no extension changes).
- Windows users still on v0.7.0 or earlier (no self-update fix)
  must first bootstrap via the v0.7.5 manual install from the
  release page; `peridot update` then carries them forward from
  v0.7.5 to v0.7.6 normally.

---

## [0.7.5] — 2026-05-19

Extension foundation. Adds the `peridot daemon` JSON-RPC subcommand
and a VS Code extension scaffold (`extensions/vscode/`) so future
phases can build chat UI / diff viewer / approval flow against a
stable wire protocol.

### Added — `peridot daemon` subcommand

- New `crates/peridot-cli/src/commands/daemon.rs`. Drives a stdin
  loop, parses line-delimited JSON-RPC 2.0 requests, dispatches each
  to its handler, writes the response (one `\n`-terminated JSON line)
  to stdout. Flushes after every write so editor extensions see
  responses in real time.
- v0.0.1 method surface (just enough to verify the pipeline
  end-to-end before real agent work lands):
  - `peridot.version` → `{ "version": "0.7.5" }`
  - `peridot.echo` → echoes `params.text` back to the client
  - `shutdown` → cleanly closes the stdin loop (ack carries
    `{ "shutdown": true }` when the client included an `id`)
- Spec-compliant error codes: -32700 parse error, -32600 invalid
  request, -32601 method not found, -32602 invalid params.
- 9 unit tests cover happy path, malformed JSON, missing `jsonrpc`
  field, unknown methods, notification-vs-request shutdown.
- 1 e2e test (gated behind the `e2e` feature) spawns the real
  `peridot daemon` binary over stdio and round-trips
  version+echo+shutdown so the framing, flushing, and binary
  argument parsing are all exercised together.

### Added — VS Code extension scaffold

- `extensions/vscode/package.json` registers two commands
  (`peridot.hello` sanity toast, `peridot.checkVersion` daemon
  round-trip) under publisher `dlsxj101`.
- `extensions/vscode/src/daemon.ts`: TypeScript JSON-RPC client.
  Spawns the daemon subprocess, correlates requests/responses by
  monotonically increasing id, exposes `send(method, params)` and
  `shutdown()`. Built to grow into the v0.1.0 agent driver
  (notification dispatcher, session lifecycle).
- `extensions/vscode/src/peridotBin.ts`: 3-tier binary lookup —
  `peridot.binaryPath` config override → bundled `<extension>/
  resources/peridot[.exe]` → system PATH. Bundling pipeline lands
  in v0.0.2.
- `extensions/vscode/src/extension.ts`: command registration +
  graceful daemon spawn-and-shutdown wrapper for the
  `checkVersion` command.

### Added — extension CI/CD

- `.github/workflows/vscode-ci.yml`: TS compile + `.vsix` package +
  artifact upload on every push to `extensions/vscode/**`.
- `.github/workflows/vscode-release.yml`: on `vsce/v*` tag,
  publishes the `.vsix` to **both** VS Code Marketplace (`vsce
  publish`) and Open VSX Registry (`ovsx publish`), then attaches
  the `.vsix` to a freshly created GitHub Release.
- Tag prefix scheme — Rust releases stay on `v*`, extension
  releases on `vsce/v*` — so each pipeline only fires for its own
  artefacts.

### Migration notes

- The Rust workspace gains no new dependencies; daemon uses
  `serde`/`serde_json` already in the tree.
- Extension publishing requires two repository secrets
  (`VSCE_PAT`, `OVSX_PAT`); the workflow's first job verifies
  they're present and fails fast with a clear error otherwise.
- v0.0.1 .vsix expects the `peridot` binary on the system PATH —
  bundled binaries arrive in v0.0.2 once the platform-target
  publish matrix is dialled in.

---

## [0.7.4] — 2026-05-19

Repo layout cleanup before extension work. No behaviour or API change
for the 14 published crates — only paths and the workspace `members`
list moved. Out-of-process callers (`cargo run -p peridot-cli`, the
`-p` flag in CI) are unaffected because Cargo resolves by package
name, not directory.

### Changed — repository layout

- All 14 `peridot-*` crates moved from the workspace root to
  `crates/peridot-*/`. The root is now uncluttered:

  ```
  peridot-agent/
  ├── Cargo.toml          # workspace root
  ├── README.md, CHANGELOG.md, AGENTS.md, PERIDOT_SPEC_v1.md
  ├── install.sh
  ├── crates/             # 14 Rust crates
  │   ├── peridot-cli/
  │   ├── peridot-core/
  │   └── …
  ├── extensions/         # NEW — non-Rust client surfaces land here
  │   └── vscode/         # placeholder; TS extension comes in v0.8.x
  ├── docs/
  └── .github/workflows/
  ```

- `Cargo.toml` workspace `members` updated to `crates/peridot-*`.
- Internal `path = "../peridot-X"` dependencies untouched —
  siblings stay siblings under `crates/`, so the resolution
  semantics don't change.
- `AGENTS.md` filesystem-path references updated
  (`peridot-cli/src/main.rs` → `crates/peridot-cli/src/main.rs`).

### Added — extension scaffold placeholder

- `extensions/vscode/.gitkeep` reserves the TypeScript extension
  directory and points future contributors at the SPEC §21.5.10
  deferral note plus the VS Code Extension API docs. The actual
  extension lands in a subsequent release once the
  `peridot daemon` JSON-RPC surface ships.

### Migration notes

- `cargo run -p peridot-cli` still works exactly as before — Cargo
  resolves `-p` against package names, not directories.
- IDE jumps from `path = "../peridot-common"` etc. continue to
  resolve because both sides moved together.
- Doc snapshots in `docs/plans/*.md` that reference legacy paths
  like `peridot-tui/src/render.rs` are historical and were not
  rewritten; treat them as time-stamped records, not live
  pointers.

---

## [0.7.3] — 2026-05-19

Defaults flipped, harness self-tuning added. Operator no longer has to
study the config to get the safe, end-to-end-completes-the-task
behaviour: every mutation auto-verifies, every `agent_done` is graded,
the 7-day idle pass promotes repeated patterns into skills, and recent
tool-usage signals flip `git.auto_commit` / `git.auto_branch` for you
at most once per project.

### Changed — defaults flipped on

- `defaults.auto_verify_after_mutation`: `false` → `true`. Every
  successful `file_write` / `file_patch` / `shell_exec` is followed
  by `verify_build` so a broken compile surfaces while the diff is
  still fresh.
- `defaults.auto_grade_on_done`: `false` → `true`. Every
  `agent_done` is gated by the LLM grader; failed verdicts inject
  recommendations as a `PlanReminder` and the loop continues for
  another turn instead of stopping. Manus-style "really finish the
  task" out of the box.
- `memory.auto_skill_reflection`: `false` → `true`. Cross-session
  n-gram promotion runs as Phase 2 of the 7-day idle Curator
  trigger, so the cost only materialises when the project has been
  idle for a week. Active sessions pay nothing.
- `peridot-cli::commands::config::set_config_key` visibility raised
  from `pub(super)` to `pub(crate)` so the new harness-learning
  pass can drive the same write path as `peridot config set`.

### Added — harness self-tuning

- New `peridot-cli::harness_learn` module. Watches the most recent
  30 sessions (capped at 60 days of age) and proposes config
  adjustments when a clear behavioural signal emerges:
  - `git.auto_commit = true` when `git_commit` appeared in ≥ 50%
    of sampled sessions.
  - `git.auto_branch = true` when `git_branch` appeared in ≥ 50%
    of sampled sessions.
- New SQLite table `harness_adjustments` (one row per auto-tuned
  field) so each field is auto-adjusted at most once across the
  project's lifetime — once the harness has spoken, the operator
  owns the field. Sample size below `MIN_SAMPLE_SIZE = 5` falls
  through silently.
- New `MemoryStore` methods: `recent_tool_sequences`,
  `was_field_auto_adjusted`, `record_harness_adjustment`.
- Phase 3 of the 7-day idle Curator trigger
  (`peridot-cli::main::maybe_run_idle_curator`) runs the harness-
  learning pass after Curator + Reflection. Each applied
  adjustment writes an `AuditEvent` (`harness_learn` action) so
  the operator can read the audit log to see why the toggle moved.

### Migration notes

- Defaults change is *behavioural* — operators with active
  `.peridot/config.toml` files keep whatever they had written
  explicitly. The flip only affects fresh projects and projects
  that left the field defaulted.
- The `harness_adjustments` table is created via
  `CREATE TABLE IF NOT EXISTS`; existing DBs upgrade seamlessly.
- Operators who want the legacy "harness never touches my config"
  behaviour can set `memory.auto_skill_reflection = false`
  explicitly — this turns off Phase 2, but Phase 3 (harness_learn)
  still runs. To disable Phase 3 fully, set the watched fields to
  their target value upfront (e.g. `git.auto_commit = true`) or
  pre-stamp the `harness_adjustments` table with a manual entry.

---

## [0.7.2] — 2026-05-19

Cross-session reflection: the harness now spots tool-call patterns the
operator runs across many sessions and promotes them into auto-skills
via an LLM reflection pass. Closes the second half of Hermes Agent's
Self-Improvement Loop — the single-session capture (`auto_skills`)
already handled "this one session looks skill-worthy"; this release
adds "this pattern keeps showing up across sessions."

### Added — cross-session n-gram reflection

- New SQLite tables `tool_sequences` (one row per completed session,
  pipe-joined tool list + truncated task summary) and `tool_ngrams`
  (rolling occurrence counters keyed by a stable hash of the tool
  list). `MemoryStore::save_tool_sequence` populates both, capped at
  50 n-gram updates per session so long sessions can't blow up the
  table. Self-repeats (`file_read x 4`) are filtered before counting.
- `MemoryStore::list_promotion_candidates(min_count, max_results)`
  returns un-promoted n-grams that have crossed the threshold,
  sorted by occurrence_count descending so the reflection pass
  tackles the most-used patterns first.
- `MemoryStore::mark_ngram_promoted(hash, at_unix)` stamps the row
  so future passes skip it, preventing the same pattern from being
  re-promoted on every idle trigger.
- New `peridot-cli::curator::run_ngram_reflection`: pulls candidates,
  asks the LLM (one batch, capped at `memory.ngram_batch_cap = 8`)
  whether each pattern is skill-worthy, writes promoted ones as
  `pattern-<title>.md` under `.peridot/skills/auto/` with
  `review_required: true`. The LLM cannot promote a pattern the
  operator never actually ran — the prompt requires the model to
  echo the exact pipe-joined tool string, which the harness
  correlates against the supplied candidates before saving.
- 7-day idle trigger (`maybe_run_idle_curator`) now runs the
  reflection pass after the standard Curator pass when
  `memory.auto_skill_reflection = true`.

### Added — config surface

- `memory.auto_skill_reflection: bool` (default `false`) — opt-in
  master switch.
- `memory.ngram_min_count: u32` (default `5`) — occurrences before a
  pattern is eligible for promotion.
- `memory.ngram_max_length: u32` (default `3`) — bigrams + trigrams
  by default; widening pays diminishing returns.
- `memory.ngram_batch_cap: usize` (default `8`) — LLM batch cap,
  mirrors the Curator's `MAX_SKILLS_PER_RUN`.

### Migration notes

- Two new tables, both created via `CREATE TABLE IF NOT EXISTS` on
  first `MemoryStore::initialize` — existing DBs from 0.7.1 keep
  loading. Historical sessions get no n-grams (only sessions
  recorded after the upgrade contribute).
- Single-session auto-skill workflow (`save_auto_skill` after the
  4-condition gate) is unchanged. Cross-session promotion is
  additive.
- The reflection pass is gated behind `memory.auto_skill_reflection`
  and never runs unless the operator opts in, so token cost is
  zero by default.

---

## [0.7.1] — 2026-05-19

Polish pass before extension work begins. Three additive changes that
prepare the agent loop and the provider trait surface for first-class
extension/desktop clients (no breaking API removals; `AgentRunEvent` /
`TuiRuntimeEvent` gain a new variant but the enums remain
`#[serde(tag = "kind")]` and existing variants are unchanged).

### Added — before/after diff rendering for every file mutation

- New `AgentRunEvent::FileDiff(FileDiffPayload)` variant. Fires after a
  successful `file_write` or `file_patch` carrying the project-relative
  path, the previous content (`None` when the file was just created),
  and the new content. Surfaced to the TUI through a matching
  `TuiRuntimeEvent::FileDiff` and rendered by a new
  `TuiState::record_file_diff` that uses the existing
  `peridot-tui::diff_hunks` LCS algorithm to emit per-line
  `TranscriptKind::Diff` entries (red `-` / green `+`, 40-line cap with
  `... +N more diff lines` clip footer).
- `file_write` now participates in the diff stream too — previously only
  `file_patch` got a transcript diff because the tool's params didn't
  carry a "before" half. The harness fills the gap by reading the
  previous content from the on-disk
  `.peridot/checkpoints/<id>.json` snapshot it was already writing for
  rollback, so no new disk writes are required.
- `write_file_checkpoint` returns a `FileCheckpoint` struct
  (`id`, `relative_path`, `absolute_path`, `previous_content`) instead
  of just the id. Callers that only needed the id still work via
  `.id` field access.
- `execute_tool_call_with_runtime` now returns
  `(ToolResult, Option<FileDiffPayload>)`. The 2 internal call sites
  that drive the event stream forward the diff via
  `AgentRunEvent::FileDiff`; the 2 internal call sites without an
  event sink discard it. Existing `execute_tool_call` /
  `execute_tool_call_with_denied_paths` wrappers unwrap the tuple, so
  external callers see no signature change.

### Added — anti-hallucination guardrail

- New `Grounding rules` block in the base system prompt
  (`peridot-core/src/prompt.rs::system_prompt_for_mode`), applied to
  every mode and every role. Forces the model to read source with
  file_read / file_outline / file_search before answering "how does X
  work?" questions, requires a concrete `path:line` (or tool name +
  quote, or URL + quote) citation for every load-bearing factual
  claim, and forbids softening speculation into confident assertions.
  Lives in Section B (Protocol) of the prompt so it stays inside the
  provider cache breakpoint and costs zero per turn after the first
  one. Covered by a `every_mode_prompt_contains_grounding_rules`
  regression test.

### Changed — provider trait surface

- `LlmProvider::pricing()` and `LlmProvider::auth_method()` are no
  longer decorative — both are now consulted by `peridot doctor` via
  the new `provider:pricing` / `provider:auth_method` checks.
  Implemented through a new `crate::providers::inspect_provider`
  helper that builds a provider stub without requiring credentials so
  the canonical pricing table is always reportable.
- `OpenAiProvider::auth_method()` now downgrades to `NotConfigured`
  when `api_key` is absent (previously echoed the stored auth method
  regardless of whether credentials were actually present).
- `OpenAiCodexProvider::auth_method()` now reports `NotConfigured`
  when the OAuth access token is empty (previously always reported
  `OAuth`). The trait method becoming honest is load-bearing for the
  doctor's "right config, just no keys yet" signal.

### Documented

- `LlmProvider::supports_prefill()`: doc comment now explicitly
  records that the method is intentionally not wired into the agent
  loop. Response prefill is Anthropic-only, and Peridot's Claude
  surface is API-key only (Claude OAuth / subscription path is not
  supported), so the lowest-common-denominator stance defers wiring
  until first-class Claude OAuth lands. Provider impls keep returning
  their honest capability so the trait surface stays accurate.
- `LlmProvider::supports_cache()` / `supports_thinking()` /
  `pricing()` / `auth_method()`: doc comments now point to the
  production caller for each so the wiring is discoverable from the
  trait definition.

---

## [0.7.0] — 2026-05-19

Production-quality pass before extension work begins. Twelve targeted
improvements across sandbox safety, context quality, MCP operability,
approval UX, observability, and PR workflow. No breaking API removals;
new fields on `SecurityConfig`, `McpServerConfig`, and `ContextEntry`
all carry `#[serde(default)]` so on-disk sessions and configs from
0.6.x continue to load.

### Added — sandbox & safety

- `security.docker_read_only_rootfs` (`--read-only` + `--tmpfs /tmp`)
  so Docker-sandboxed shell commands can't pollute the container fs
  outside `/workspace`.
- `security.docker_memory_limit` (e.g. `"512m"`) forwarded as
  `--memory` so a runaway container gets OOM-killed instead of pinning
  the host.
- `security.shell_command_timeout_seconds` (default `0` = unlimited).
  When set, `shell_exec` kills the child via the same path as Esc
  cancel and reports a recoverable timeout error.
- `security.shell_dry_run` returns a synthetic `ToolResult` describing
  the would-be invocation (program + args + cwd) without actually
  launching it. Useful for safety drills and CI smokes.

### Added — context quality

- Pinned memory: `ContextEntry.pinned` plus
  `ContextManager::append_pinned`, `pinned_count`, `unpin_where`.
  Pinned entries survive both deterministic and LLM-driven compaction.
- More accurate token estimator: `estimate_tokens_for_text` swaps the
  legacy `chars/4` heuristic for a CJK-aware word + punctuation + long-
  identifier scheme that lands within 5-10% of real BPE counts on
  representative mixed inputs (no new dependency).
- Content-aware tool-output digesting: unified diffs collapse to hunk
  count + filenames, stacktraces collapse to anchor + first 2 frames,
  test output collapses to the result line + first failure. Driven by
  `digest_string_content` and consumed by every compaction path.

### Added — MCP operability

- `McpClient::list_tools()` is now schema-cached per-server with a
  configurable TTL (`McpServerConfig::schema_cache_seconds`, default
  300s). `invalidate_tools_cache()` for explicit refresh.
- `McpClient::health_check()` returns measured latency for a probe call.
- `McpServerConfig::default_permission` + `tool_permission_overrides`
  let the operator drop a server (or a single tool) from the default
  "Everything is System" gating down to `read` / `write` /
  `destructive`. Resolved by `resolve_mcp_permission_level`.
- MCP tool calls now write to `audit.jsonl` with the resolved
  permission level alongside the existing `params` payload.
- New `peridot mcp doctor` subcommand: runs validate + health probe +
  tool count across every configured server in one shot.

### Added — approval & recovery

- Permission-denied errors now get a dedicated `recovery_message`
  branch instead of rotating through the generic templates. The
  directive explicitly forbids retrying the same call and steers the
  model toward read-only alternatives or an `agent_ask_user` escape
  hatch.

### Added — observability

- New `peridot doctor` subcommand: end-to-end health audit covering
  `.peridot/` layout, provider auth (per primary), models config,
  AGENTS metadata, MCP servers, and security posture. Returns non-zero
  on any fail so it composes with shell pipelines.

### Added — PR workflow

- New `peridot ship` subcommand: branch → commit → push → PR in one
  call. Refuses to land on `main` / `master` / `trunk` unless
  `--allow-protected-branch` is passed. `--no-pr` skips the `gh pr
  create` step for safer dry runs.

### Added — test coverage

- Mock-LLM e2e regressions in `peridot-core/src/tests/harness.rs`:
  pending_resume sidecar round-trip and AGENTS.md hot reload.
- Serde compat regressions for `ContextEntry`: legacy (no `pinned`)
  and forward-compat (with `pinned`) payloads both round-trip.

### Added — auto-fix smarts

- `VerifyFailureState` now carries `hints: Vec<String>` — `file:line`
  tokens extracted from the verifier output. The directive surfaces
  them as "Likely culprit(s)" so the model jumps straight to the
  failing file. Recognises Rust (`src/foo.rs:12:5`), Python (`File
  "src/foo.py", line 12`), TypeScript / JS / Go (`foo.ts:12`).

### Added — scanner reach

- Gradle (`build.gradle` / `build.gradle.kts`, wrapper-aware), Maven
  (`pom.xml`, wrapper-aware), CMake (`CMakeLists.txt`), Swift Package
  Manager (`Package.swift`), and .NET (`*.csproj` / `*.sln`) all flow
  through `peridot scan` with reasonable default build/test commands.

### Changed

- `peridot-core`: extracted `approval_required_error`,
  `is_mutating_tool_name`, `truncate_chars`, `recent_verify_summary`
  into a new `agent_helpers` module so `agent.rs` reads top-to-bottom
  without stepping over stateless utility code.
- `peridot-cli/src/main.rs`: added a `ShipArgs` struct mirroring the
  v0.6.0 `VerifyArgs` pattern so `Command::Ship` typechecks despite
  the rustc 1.95 ICE that fires on inline struct variants with mixed
  optional / boolean flags inside `main`'s match.

### Migration notes

- `McpServerConfig` gained three optional fields. Existing config.toml
  files keep working; the new fields surface their defaults
  (`default_permission = "system"`, `schema_cache_seconds = 300`,
  `tool_permission_overrides = {}`).
- `SecurityConfig` gained four optional fields. Existing config.toml
  files keep working; sandbox behaviour is unchanged until the
  operator opts into `docker_read_only_rootfs`, `docker_memory_limit`,
  `shell_command_timeout_seconds`, or `shell_dry_run`.
- `ContextEntry::pinned` defaults to `false`, so 0.6.x session blobs
  load with no special handling.

---

## [0.6.0] — 2026-05-19

### Added

- **Verify pipeline grader integration**: `VerifyPipeline::run_all_with_grader(provider, model, task)` runs the LLM grader after every deterministic stage (build / test / lint / diff-review) passes. Surfaced through `peridot verify --with-grader --grader-task "<text>"`. The grader stage carries the verdict summary and `passed` mirrors the LLM verdict. Skipped automatically when deterministic stages fail so no API tokens are wasted on duplicating a known-negative verdict.
- **`agent_message` built-in tool**: subagents can now route notes to a `parent` or named `child:<session_id>`. Recipients see the message at the start of their next turn as a `[peer message from <id>]` PlanReminder. Backed by the new `AgentMessageBus` trait + `SessionRouter` inbox queue per session. `agent_message` registers in `register_builtin_tools`; the TUI's pre-existing display handler now has a real source to render.
- **`VerifyStage::Lint` variant**: lint failures report as `Lint` instead of being mislabelled as `Deterministic`. Affects `peridot verify` text output and JSON serialisation (`"stage": "lint"`).
- **`peridot-grader` crate**: extracted from `peridot-core/src/grader.rs` so both `peridot-verify` and `peridot-core` can invoke `grade_work` without inducing a dependency cycle. `peridot-core::grader::*` keeps re-exporting `grade_work` / `GraderVerdict` for backward compatibility.
- **Anthropic prompt cache_control automation**: `anthropic_payload_with_cache` stamps three breakpoints when `provider.supports_cache()` is true — last tool definition, system block, and the most recent assistant/tool_result entry. Trailing user prompts stay unmarked so new user input never busts the cache. Skipped automatically for providers (e.g. OpenAI Chat Completions) that disable caching.
- **`SessionRouter::RouterMessageBus`**: shared-router message bus implementation. Provides `send_to_parent` / `send_to_child` / `drain_inbox` with per-session FIFO queues. Includes the safety check that a session can only message its own direct children (no sibling addressing).
- **`HarnessAgent::set_message_bus` / `set_session_id`**: harness drains its inbox at the start of every turn and folds received messages into context as PlanReminders. `InnerLoopSubAgent` intentionally does not propagate the bus to grandchild contexts (depth-1 cap, fork-bomb safety).
- **`VerifyArgs` clap struct**: extracted `peridot verify` flags into a dedicated `Args` struct to work around a rustc 1.95 ICE when inline struct variants with optional boolean flags appeared inside `main`'s match.

### Changed

- **`LlmProvider::supports_thinking()` now consulted by the harness**: `HarnessAgent::run_turn_with_events` AND-gates the thinking flag with `provider.supports_thinking()`, so Goal mode runs against providers that don't support thinking (OpenAI Chat Completions, etc.) no longer send a payload field the server will ignore.
- **`LlmProvider::supports_cache()` now consulted by `ClaudeProvider`**: cache_control marking is gated by this method, making the capability advertisement load-bearing instead of decorative.
- **`LocalSubAgentRunner::Teammate` now provisions a real worktree**: previously returned a placeholder string. Now shares the worktree machinery with `Worktree`; the kinds differ only in lifecycle (long-running) and routing (parent↔child message bus).
- **`LocalSubAgentRunner::Fork` summary clarified**: changed from `"fork subagent prepared"` to `"fork workspace prepared (shared with parent) for task: ..."` so the operator can tell whether the agent loop actually executed (only `InnerLoopSubAgent` does that; `LocalSubAgentRunner` only prepares the workspace).
- **SPEC v1.9 documentation realignment**:
  - 4-Tier compaction notation collapsed to 2-Tier (deterministic + LLM). The original Tier 0 / Tier 2 / Tier 3 stages were absorbed into existing paths and never had distinct implementations.
  - Append-Only principle restated as **in-turn only**. Compaction is explicitly allowed to reconstruct the entries vec; the last substantive user/tool result is preserved via `preserved_anchor` + `COMPACTION_KEEP_TAIL`.
  - Tool list (SPEC §7.2) corrected: `agent_fork` / `agent_worktree` collapsed into `agent_delegate(kind=...)`; the 9 tools added since v0.5.0 (`file_outline`, `symbol_search`, `workspace_symbols`, `git_push`, `gh_pr_*`, `skill_list`, `skill_view`) are now listed alongside the new `agent_message`.
  - Plan-mode blocklist (SPEC §2.1) updated: `agent_fork`/`agent_worktree` removed, `agent_delegate` added.
- **Workspace version**: 0.5.1 → 0.6.0 (minor bump for additive feature surface — new crate, new tool, new trait, new CLI flags; no breaking removals).

### Fixed

- **`run_lint()` no longer mislabels lint failures as `Deterministic`**: the stage tag now matches the actual check.
- **Provider capability methods are no longer dead code**: `supports_cache()` and `supports_thinking()` are now reachable from production paths. `supports_prefill()` / `pricing()` / `auth_method()` remain trait obligations but stay decorative pending future work.

### Migration notes

- All API changes are additive. Existing callers of `peridot_core::grader::*` keep working through re-exports.
- The Anthropic wire payload now carries `cache_control` markings by default. The response shape already surfaced `cache_creation_input_tokens` / `cache_read_input_tokens` (parsed since v0.4.x), so dashboards and logs continue to work; expect non-zero cache stats starting with the second turn of any session.
- `peridot session show <id>` continues to render the same shape; no on-disk format changed.
- `peridot verify` (no flags) is identical to v0.5.1. Grader behaviour only activates when `--with-grader` is passed together with `--grader-task <TEXT>`.
- Existing `agent_delegate` callers see no API change; only the placeholder summary text shifted for `kind=fork` and `kind=teammate`.

---

## Earlier versions

See [PERIDOT_SPEC_v1.md](PERIDOT_SPEC_v1.md) version history (v1.0 – v1.8) for
0.4.x and 0.5.x change notes, and the [GitHub Releases](https://github.com/dlsxj101/peridot-agent/releases)
page for download artefacts.
