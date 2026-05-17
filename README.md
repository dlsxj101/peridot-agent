# Peridot Agent

Peridot Agent is a Rust CLI/TUI autonomous coding agent. The current implementation is a buildable v0.1-ready foundation that follows `PERIDOT_SPEC_v1.md`.

## Status

Implemented:

- Cargo workspace with the 13 spec crates.
- Provider-neutral LLM contracts with Claude Messages and OpenAI Responses providers, including provider-native streaming response parsing.
- Append-only context manager with large-observation offload.
- Built-in file, shell, plan, git, verify, and agent tools.
- AGENTS.md path boundary enforcement.
- Bounded agent loop with deterministic mock provider support, Goal Checker, budget guardrails, and parse-failure recovery reminders.
- Project scanner for Rust, Node, Python, Go, Make, AGENTS metadata, and git state.
- SQLite-backed session summary store.
- AGENTS, skill, MCP, verify, setup, login/logout, and session resume CLI surfaces.
- MCP stdio and HTTP initialize, `tools/list`, `tools/call`, auth headers, and ToolRegistry adapters.
- Deterministic verification pipeline and git worktree helpers.
- Configured tool hooks with warn/block behavior and audit JSONL logging.
- OpenAI API-key and OAuth PKCE login storage.
- OpenRouter API-key execution with Peridot-managed user-local environment storage.
- Headless CLI commands and Ratatui-backed interactive TUI shell.
- GitHub Actions CI, six-target release packaging, `install.sh`, checksum-verified self-update, and startup update notices.

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

Configure OpenRouter with the welcome wizard and run live:

```bash
cargo run -p peridot-cli -- config init
```

Run the wizard again at any time:

```bash
cargo run -p peridot-cli -- config wizard
```

Or update individual settings without opening an editor:

```bash
cargo run -p peridot-cli -- config set auth.primary openrouter-api
cargo run -p peridot-cli -- config set api.base_url https://openrouter.ai/api
cargo run -p peridot-cli -- config set models.main openai/gpt-4o-mini
# `models.goal_checker` and `models.compaction` are not separately
# configurable — they always follow `models.main`.
```

The resulting config uses the same values as:

```toml
[auth]
primary = "openrouter-api"

[api]
base_url = "https://openrouter.ai/api"

[models]
main = "openai/gpt-5.2"
```

```bash
cargo run -p peridot-cli -- run "inspect this project" --headless
```

For ChatGPT Pro through the local Codex app-server, log in with Codex first, then use the `codex` provider:

```toml
[auth]
primary = "codex"

[models]
main = "gpt-5.5"
```

Live model responses must currently follow Peridot's JSON action protocol:

```json
{"action":"file_read","parameters":{"path":"README.md"}}
```

## Updates

```bash
cargo run -p peridot-cli -- update --check
cargo run -p peridot-cli -- update --force
```

Interactive sessions honor `[updates]` config, check at most once per interval, and print a one-line notice. `peridot update` verifies `SHA256SUMS` before replacing the current binary and keeps the `peri` alias in place.

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
