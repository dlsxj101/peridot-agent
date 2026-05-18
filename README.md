# Peridot Agent

Peridot Agent is a Rust CLI/TUI autonomous coding agent with multi-session orchestration, multi-LLM committee mode, and native tool calling.

## Status

Current version: **0.5.1**

Implemented:

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
