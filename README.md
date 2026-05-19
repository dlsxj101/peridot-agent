# Peridot Agent

Peridot Agent is a Rust CLI/TUI autonomous coding agent with multi-session orchestration, multi-LLM committee mode, and native tool calling.

## Status

Current version: **0.7.2**

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
