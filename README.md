# Peridot Agent

Peridot Agent is a Rust CLI/TUI autonomous coding agent with multi-session orchestration, multi-LLM committee mode, and native tool calling.

## Status

Current version: **0.8.9**

### What's new in v0.8.9

- **Editor slash commands now run through the daemon.** The VS Code panel calls
  `session.command` for branch, MCP, compact, TODO, diff, undo, and context
  commands, so those flows share the same project-state behavior as the TUI.
- **Extension chats survive reloads.** Open chat sessions, transcripts,
  daemon session ids, queued prompts, and run options are restored from
  workspace storage after an Extension Host reload.
- **Command and login UI got sturdier.** Branch/MCP/TODO/context results render
  as structured panel rows, ChatGPT OAuth exposes a visible manual login link,
  and the VSIX package now includes an MIT license file.

### What's new in v0.8.8

- **VS Code slash commands now cover the TUI catalog.** The extension picker
  includes goal, reasoning, fast tier, session, branch, MCP, TODO, and helper
  commands; `/reasoning`, `/think`, `/fast`, and `/goal` update real run state.
- **Read-only shell inspection no longer triggers auto-build verification.**
  Auto-verify now follows direct file edits (`file_write` / `file_patch`) so
  environment questions do not get polluted with cargo build output.
- **The chat UI is quieter and clearer.** `Run started` is hidden, token stats
  align to the right, and copied assistant answers show a check mark for 3
  seconds.

### What's new in v0.8.7

- **Slash commands are keyboard-selectable in both the TUI and extension.**
  Typing `/` opens a command picker, arrow keys move the selection, and Enter
  accepts or runs the highlighted command.
- **The VS Code panel now treats slash commands as session controls.** `/clear`
  clears the transcript and daemon context, mode/permission/model slashes update
  composer state, and `/session ...` commands manage open sessions.
- **Tool file paths open in the editor.** Tool rows with a file path can jump to
  the file, including line/column when the tool provides them.

### What's new in v0.8.6

- **Editor daemon sessions now continue across follow-up prompts.** `session.start`
  can target an existing session id so Cursor / VS Code clients keep the same
  context snapshot until the operator explicitly clears or opens a new session.
- **File reads tolerate non-UTF-8 bytes.** Workspace reads now replace invalid
  UTF-8 bytes instead of failing the whole tool call.

### What's new in v0.8.5

- **Recovery loops now stop quickly on repeated errors.** Provider,
  parser, network, credential, and configuration errors wait 3 seconds
  before retrying and stop after 3 recovery attempts with a clear transcript
  message instead of burning through the full turn budget.

### What's new in v0.8.4

- **Windows home directory detection now accepts `USERPROFILE`.** Native
  Windows runs no longer fail auth/config/cache setup with `HOME is required`
  when launched from Cursor or other Windows extension hosts.

### What's new in v0.8.3

- **Approval tests stop after the approved command completes.** Auto-grade now
  compares the worktree diff against the run-start baseline, so pre-existing
  dirty files do not trigger unrelated recovery turns after a no-code approval
  run.
- **`rm -rf` hard-blocking is more precise.** True root deletes such as
  `rm -rf /` and `rm -rf /*` still hard-block, while explicit subpath deletes
  like `/home/.../tmp-approval-test` fall through to the normal destructive
  approval flow.

### What's new in v0.8.2

- **Hard-blocked shell commands no longer masquerade as approval requests.**
  Commands such as `rm -rf /` now return a paired failed tool output instead
  of entering the approval-resume path. This prevents fake approval cards,
  `session is not waiting for approval`, and follow-up OpenAI Codex
  `No tool output found for function call ...` recovery loops.

### What's new in v0.8.1

- **Approval resume now preserves native tool-call history.** When an editor
  approval resumes a saved pending tool call, Peridot now records the resumed
  tool result as the matching native tool output. This fixes OpenAI Codex
  `400 Bad Request: No tool output found for function call ...` recovery loops
  after approving a gated command.

### What's new in v0.8.0

- **`peridot daemon` now runs on tokio.** stdin is read through a blocking-compatible bridge, stdout is drained by a single async writer, and concurrent sessions can emit JSON-RPC frames without interleaving.
- **Editor clients can start and cancel real agent sessions.** New `session.start` and `session.cancel` methods let VS Code / Cursor spawn the existing harness loop, receive serialized `AgentRunEvent` notifications, and cooperatively interrupt active runs by session id.
- **Daemon session notifications are now live.** Every started session emits a `started` notification, forwards the core agent event stream as JSON-RPC `event` notifications, and ends with `finished` or `error`.

### What's new in v0.7.10

- **Peridot deer mascot redrawn at 16×16** with eight mood-specific frame sets (Idle blink, Thinking head-tilt, ToolRunning 3-frame gem pulse, ApprovalWaiting alert eyes, AskUser raised ears, Done bounce, Failed droop, Interrupted startle). Rendered footprint stays at 8 cols × 4 rows because every 2×2 pixel block compresses into one terminal cell via Unicode quadrant-block glyphs.

### What's new in v0.7.9

Four UX fixes from live v0.7.8:

- **Phantom caret next to status bar.** The textarea caret was drawn every tick, even while the agent was streaming and the input was empty. Now suppressed when the agent is `Running` and the user hasn't started a draft.
- **`/clear` actually clears.** Transcript, side-panel stats, header tokens / cost / cache, plan, active tools, panels, queues — all wiped. A new `SessionCommandEvent::ClearAndRestart` then cancels the live agent, closes the session, deletes its persisted context, and opens a fresh session. Next message starts with zero recall.
- **Esc actually interrupts.** `CancelToken::cancelled()` is now an async future; `stream_completion_with_chunks` races it against the streaming LLM call. When you hit Esc, the streaming future is dropped and the connection aborts within ~50ms — no more 10-30s wait for the model to finish.
- **`verify_build` uses project-detected commands.** Hard-coded `cargo build --workspace` fallback is gone. `verify_build` / `verify_test` / `verify_lint` now scan the project (root Cargo.toml/package.json/pyproject.toml + common monorepo subdirs like `frontend/`, `apps/web/`, `backend/`, …) and pick the right command. A Python backend + Vite `frontend/` resolves to `cd frontend && npm run build`.

### What's new in v0.7.8

- **Auto-grade no longer loops forever on chat / Q&A turns.** Empty `git diff HEAD` now short-circuits the grader: a plan-reminder is logged, `Finished` fires, the run exits cleanly. Previously the grader rejected every non-coding turn ("No change was provided to address the request"), the agent re-answered, and the loop spun until `max_turns`.

### What's new in v0.7.7

Three more TUI fixes from live v0.7.6 use on Windows:

- **Textarea filled with raw escape sequences after `shell_exec`** — child processes (npm, vite, spinner libs) inherited the TUI's tty stdin and reset its termios on exit, so arrow keys / PageUp / PageDown arrived as raw `[A` / `[B` / `[5~`. Fixed by (1) setting `Stdio::null()` for shell child stdin and (2) re-asserting `enable_raw_mode()` every event-loop tick.
- **Korean / CJK cursor lag** — `render.rs` used `chars().count()` to position the textarea caret; CJK glyphs occupy 2 cells, so the caret fell behind one cell per wide character. Switched to `UnicodeWidthStr::width`.

### What's new in v0.7.6

Three Windows / OAuth bug fixes from a real v0.7.5 first-run on Windows 11:

- **OAuth URL truncated at first `&`** — `cmd /C start "" <url>` had no quoting around the URL, so cmd.exe treated every `&` as a command separator and the browser opened `https://auth.openai.com/oauth/authorize?response_type=code` instead of the full URL. Fixed by quoting the URL via `CommandExt::raw_arg`.
- **Setup wizard required Enter twice** — `wait_for_oauth_code` spawned a background stdin reader that outlived the listener path and silently swallowed the user's next keystroke (e.g. their `2` for `gpt-5.5-fast`). Reader removed; paste-fallback still runs when `TcpListener::bind` fails.
- **`400 No tool output found for function call`** — when a tool errored, the agent loop appended the `tool_call` but bailed before appending the matching `function_call_output`, malforming the conversation for OpenAI Codex (Responses API). Fixed by synthesising a failed `ToolResult` and appending it before bubbling the error to the recovery layer.

### What's new in v0.7.5

Extension foundation:

- **`peridot daemon`** subcommand — line-delimited JSON-RPC 2.0 over stdio. v0.0.1 surface (`peridot.version` / `peridot.echo` / `shutdown`) verifies the publish pipeline end-to-end before real agent work lands. Real `session.start` / `approval.respond` / `ask_user.respond` arrive in v0.7.6.
- **VS Code extension scaffold** at `extensions/vscode/` (`dlsxj101.peridot-vscode`). Two commands (`Peridot: Hello`, `Peridot: Check Daemon Version`) prove the spawn + JSON-RPC pipeline.
- **Publish pipeline**: `.github/workflows/vscode-release.yml` triggers on `vsce/v*` tag, publishes to **both** VS Code Marketplace and Open VSX Registry, attaches `.vsix` to GitHub Release. CI compiles + packages on every PR.
- Tag prefix scheme: Rust releases on `v*`, extension releases on `vsce/v*`.

### What's new in v0.7.4

Repo layout cleanup before extension work:

- All 14 Rust crates moved from workspace root to `crates/peridot-*/` for a less cluttered root.
- New `extensions/` directory reserves space for non-Rust client surfaces (VS Code, JetBrains, desktop). VS Code scaffold lands in a subsequent release once `peridot daemon` ships.
- No behaviour change: `cargo run -p peridot-cli` and all other package-name commands work identically. Path-dependency resolution (`../peridot-common`) preserved because siblings stayed siblings under `crates/`.

### What's new in v0.7.3

Operator no longer needs to study the config to get the "Manus-style: finish the task end-to-end" behaviour:

- **Auto-verify-after-mutation default ON**: every `file_write` / `file_patch` / `shell_exec` is followed by `verify_build` so broken compiles surface immediately.
- **Auto-grade-on-done default ON**: every `agent_done` runs the LLM grader; failed verdicts inject recommendations and continue the loop instead of stopping. Manus-style "really finish" out of the box.
- **Auto-skill-reflection default ON**: the 7-day idle pass now also promotes repeated tool-call patterns (5+ occurrences) into auto-skills, with `review_required: true`.
- **Harness self-tuning**: the same idle trigger watches recent tool usage and auto-flips `git.auto_commit` / `git.auto_branch` when ≥ 50% of recent sessions used the corresponding tool manually. Each field is auto-adjusted at most once; the operator owns it after that. Every change writes an `AuditEvent` (`harness_learn` action) so the audit log explains why a default moved.

### What's new in v0.7.2

Cross-session reflection — second half of the Hermes-style Self-Improvement Loop:

- **N-gram pattern detection**: every completed session's tool sequence is recorded; bigrams and trigrams accumulate counts in `tool_ngrams`. A pipe-joined tool list serves as the stable hash key. Self-repeats (`file_read × 3`) are filtered.
- **LLM reflection pass**: the 7-day idle Curator trigger now also asks an LLM whether any pattern that crossed `memory.ngram_min_count` (default 5) is worth promoting into a skill. Promoted patterns land as `.peridot/skills/auto/pattern-<title>.md` with `review_required: true`.
- **Opt-in**: `memory.auto_skill_reflection = false` by default. Single-session capture (`auto_skills`) keeps working unchanged; this is purely additive.
- **Knobs**: `ngram_min_count`, `ngram_max_length`, `ngram_batch_cap` for tuning the threshold, window width, and batch cost.

### What's new in v0.7.1

Polish pass before extension work begins:

- **Before/after diff for every file mutation**: new `AgentRunEvent::FileDiff` carries `(path, before, after)` after every successful `file_write` / `file_patch`, so the TUI now renders a real unified diff for both tools (previously only `file_patch` had one because its params carried `old_text` / `new_text`; `file_write` was new-content-only). The before half comes from the `.peridot/checkpoints/<id>.json` snapshot the harness was already writing for `/undo`, so no extra disk writes. Future extension / desktop clients consume the same event for their own diff viewers.
- **Provider-trait gap closed**: `LlmProvider::pricing()` and `auth_method()` are now consulted by `peridot doctor` via the new `provider:pricing` and `provider:auth_method` checks (previously declared on every impl but never called). OpenAI / OpenAI-Codex providers now downgrade `auth_method()` to `NotConfigured` when credentials are absent, matching ClaudeProvider's behaviour.
- **`supports_prefill()` intent documented**: doc comment on the trait method now explicitly records the deferral — Anthropic-only, Claude OAuth not supported, lowest-common-denominator stance keeps the optimisation deferred until first-class Claude OAuth lands.
- **Grounding rules in system prompt**: new `Grounding rules` block enforces "read source before answering, cite `path:line` for every load-bearing claim, hedge instead of fabricating confidence." Applied to every mode and every role; lives in Section B (Protocol) so the provider cache stays warm.
- **Documentation cleanup**: SPEC §7.2 tool count corrected (33, not 34); §21.5.10 deferral list trimmed — turn-level branching, diff hunk staging, auto-fix loop were already implemented in v1 and have moved to §21.5.9.

### What's new in v0.7.0

Production-quality pass before extension work begins:

- **Sandbox hardening**: Docker `--read-only` rootfs + tmpfs, memory limits, per-command timeouts, and a `shell_dry_run` mode for safety drills.
- **Better token estimator**: CJK-aware word/punctuation heuristic in `peridot-context` replaces the legacy `chars/4` (no new BPE dependency).
- **MCP operations**: `tools/list` schema cache with TTL, `health_check()` latency probe, per-server `default_permission`, per-tool `tool_permission_overrides`, MCP calls in `audit.jsonl`, and a new `peridot mcp doctor` subcommand.
- **Approval recovery**: permission-denied errors now get a dedicated "read-only alternative or ask the user" directive instead of rotating through generic templates.
- **`peridot doctor`**: end-to-end health audit (config, provider auth, MCP servers, AGENTS metadata, security posture) with non-zero exit on fail.
- **`peridot ship`**: one-shot branch → commit → push → PR with a protected-branch guard.
- **Pinned memory**: `ContextEntry.pinned` survives compaction; expose `append_pinned` / `unpin_where`.
- **Content-aware compaction**: diff / stacktrace / test-output specialised summarisers.
- **Auto-fix culprit hints**: verifier output parsed for `path:line` tokens (Rust / Python / JS / Go), surfaced in the auto-fix directive.
- **Scanner reach**: Gradle, Maven, CMake, Swift Package Manager, and .NET now flow through `peridot scan`.
- **Mock-LLM e2e regressions**: pending_resume round-trip + AGENTS hot reload + serde compat tests.

### What's new in v0.6.0

Nine SPEC-consistency issues from the v0.5.1 audit are now resolved (see [CHANGELOG.md](CHANGELOG.md) for the full list):

- **Verify pipeline grader**: `peridot verify --with-grader --grader-task "<text>"` now runs the LLM grader after deterministic stages, so the CLI verify report includes the grader stage that previously only existed in the agent loop's `auto_grade_on_done` path.
- **Anthropic prompt cache_control**: 3-breakpoint cache markings (tools / system / conversation prefix) are now stamped automatically for providers that advertise `supports_cache()`. Expect lower input-token costs on long sessions; cache stats surface via `usage.cache_read_input_tokens`.
- **`agent_message` built-in tool**: subagents can now message their parent or named children via `agent_message {target, message}`. The recipient sees the note as a `[peer message from <id>]` PlanReminder at the start of its next turn.
- **Lint stage gets its own variant**: `VerifyStage::Lint` is no longer aliased to `Deterministic`, so failing lints show up as `FAIL Lint:` instead of `FAIL Deterministic:` in verify reports.
- **Fork / Teammate isolation parity**: `LocalSubAgentRunner` now provisions a real git worktree for both `Worktree` and `Teammate` kinds (Fork stays shared-workspace by design).
- **New `peridot-grader` crate**: the grader logic moved out of `peridot-core` so `peridot-verify` can call it without a dependency cycle. `peridot-core::grader::*` keeps re-exporting the public API.
- SPEC v1.9 updates: 4-Tier compaction → 2-Tier (deterministic + LLM), Append-Only is `in-turn` only, tool list reflects the 33 actually registered tools (`agent_fork`/`agent_worktree` merged into `agent_delegate`).

### Implemented

- Cargo workspace with 13 spec crates (`peridot-cli`, `peridot-core`, `peridot-llm`, `peridot-tui`, etc.).
- Provider-neutral LLM contracts with Claude Messages, OpenAI Chat Completions, OpenAI Codex OAuth, and OpenRouter providers. Native tool calling and streaming.
- Append-only context manager with large-observation offload and live context utilization indicator.
- Built-in file, shell, plan, git, verify, and agent tools with progressive disclosure (`skill_list`, `skill_view`).
- AGENTS.md path boundary enforcement.
- Bounded agent loop with deterministic mock provider support, Goal Checker, budget guardrails, parse-failure recovery, and intent clarification flow (`agent_ask_user`).
- Project scanner for Rust, Node, Python, Go, Make, AGENTS metadata, and git state.
- SQLite-backed session summary store with session save/resume.
- Multi-session runtime: `SessionRouter`, `CancelToken`, workspace isolation, `/fork`, `/teammate`, `/worktree` subagent spawning.
- LLM-generated session titles after first response (main model, no reasoning overhead).
- Multi-LLM committee mode: Planner / Reviewer / Executor pipeline with per-role cost tracking.
- LLM Curator sub-agent with 30/90-day auto-archive rules, skill curation, and `memory_search`.
- Ratatui-backed interactive TUI with i18n (English/Korean), mascot, side panel, approval/ask-user panels, branch picker, and single-session tab bar.
- CLI surfaces: `agents`, `skill`, `mcp`, `verify`, `setup`, `login`/`logout`, `session`, `config`, `env`, `update`.
- MCP stdio and HTTP initialize, `tools/list`, `tools/call`, auth headers, and ToolRegistry adapters.
- Deterministic verification pipeline and git worktree helpers.
- Configured tool hooks with warn/block behavior and audit JSONL logging.
- OpenAI API-key and OAuth PKCE login storage; OpenRouter managed env storage.
- GitHub Actions CI, six-target release packaging, `install.sh`, checksum-verified self-update (with Windows rename-then-copy), and startup update notices.
- Unicode-safe display-width truncation and Windows `KeyEventKind::Press` filtering for cross-platform TUI stability.

## Common Commands

```bash
cargo fmt --all --check
cargo clippy --workspace -- -D warnings
cargo test --workspace
```

```bash
cargo run -p peridot-cli -- --version
cargo run -p peridot-cli -- scan --output json
cargo run -p peridot-cli -- setup
cargo run -p peridot-cli -- config init
cargo run -p peridot-cli -- verify --output json
cargo run -p peridot-cli -- session save demo "initial work"
cargo run -p peridot-cli -- session list
cargo run -p peridot-cli -- session resume demo
cargo run -p peridot-cli -- agents show
cargo run -p peridot-cli -- skill list
cargo run -p peridot-cli -- mcp list
```

## Deterministic Agent Loop

Use `--mock-response-file` to exercise the full model-response parsing and tool-execution loop without API calls:

```bash
cat > /tmp/peridot-responses.jsonl <<'JSONL'
{"action":"file_write","parameters":{"path":"hello.py","content":"print(\"Hello World\")\n"}}
{"action":"agent_done","parameters":{"summary":"created hello.py"}}
JSONL

cargo run -p peridot-cli -- run "create hello.py" \
  --mock-response-file /tmp/peridot-responses.jsonl \
  --headless --output json
```

## Live Providers

Peridot runs with the configured live provider by default. Start the TUI with `peridot`, or pass a task directly:

```bash
peridot
peridot "inspect this project"
peridot run "inspect this project" --headless
```

Live execution uses environment credentials or credentials stored with `peridot login`:

```bash
ANTHROPIC_API_KEY=... cargo run -p peridot-cli -- run "inspect this project"
OPENAI_API_KEY=... cargo run -p peridot-cli -- login openai-api
OPENAI_OAUTH_CLIENT_ID=... cargo run -p peridot-cli -- login openai-oauth
```

OpenRouter keys can be managed by Peridot instead of exported in every shell. The value is stored in the user-local Peridot env store at `~/.peridot/env` with private file permissions:

```bash
cargo run -p peridot-cli -- env set OPENROUTER_API_KEY sk-or-...
cargo run -p peridot-cli -- env list
```

Configure providers with the welcome wizard:

```bash
cargo run -p peridot-cli -- config init     # first-time project setup
cargo run -p peridot-cli -- config wizard   # re-run at any time
```

Or update individual settings without opening an editor:

```bash
cargo run -p peridot-cli -- config set auth.primary openrouter-api
cargo run -p peridot-cli -- config set api.base_url https://openrouter.ai/api
cargo run -p peridot-cli -- config set models.main openai/gpt-4o-mini
```

Example provider configurations:

```toml
# OpenRouter
[auth]
primary = "openrouter-api"
[api]
base_url = "https://openrouter.ai/api"
[models]
main = "anthropic/claude-sonnet-4-6"
```

```toml
# ChatGPT subscription (OAuth direct)
[auth]
primary = "openai-oauth"
[api]
base_url = "https://chatgpt.com/backend-api/codex"
[models]
main = "gpt-5.5"
```

```toml
# Anthropic API
[auth]
primary = "claude-api"
[models]
main = "claude-sonnet-4-6"
```

## Updates

```bash
cargo run -p peridot-cli -- update --check
cargo run -p peridot-cli -- update --force
```

Interactive sessions honor `[updates]` config, check at most once per interval, and print a one-line notice. `peridot update` verifies `SHA256SUMS` before replacing the current binary and keeps the `peri` alias in place. On Windows, the running executable is renamed before replacement.

## Project Initialization

```bash
peridot setup
```

This creates `.peridot/config.toml`, `.peridot/hooks/`, `.peridot/skills/`, gitignore entries for local memory/logs/generated skills, and an `AGENTS.md` draft when no compatible instruction file exists.

## Hooks And Audit

Tool hooks are configured in `.peridot/config.toml` and must execute scripts under `.peridot/hooks/`:

```toml
[hooks]
timeout_seconds = 30

[[hooks.tool]]
event = "pre:file_write"
run = ".peridot/hooks/check-write.sh {path}"
on_failure = "block"
only_paths = ["src/**"]
```

Tool calls append audit entries to `.peridot/logs/audit.jsonl`.

## Release

CI runs formatting, Clippy, and the workspace test suite on pushes and pull requests. Tags matching `v*` build release archives for Linux, macOS, and Windows on x86_64 and aarch64. Unix targets publish `.tar.gz`; Windows publishes both `.tar.gz` and `.zip`.
Release publishing also attaches `SHA256SUMS`, a checksum-verifying `install.sh`, and a generated `peridot.rb` Homebrew formula with `peridot` plus the `peri` alias.

```bash
curl -fsSL https://raw.githubusercontent.com/dlsxj101/peridot-agent/main/install.sh | sh
```
