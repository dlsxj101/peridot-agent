# Peridot Agent

Peridot Agent is a Rust CLI/TUI autonomous coding agent. The current implementation is an early, buildable foundation that follows `PERIDOT_SPEC_v1.md`.

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

Live execution uses environment credentials or credentials stored with `peridot login`:

```bash
ANTHROPIC_API_KEY=... cargo run -p peridot-cli -- run "inspect this project" --live
OPENAI_API_KEY=... cargo run -p peridot-cli -- login openai-api
OPENAI_OAUTH_CLIENT_ID=... cargo run -p peridot-cli -- login openai-oauth
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
curl -fsSL https://raw.githubusercontent.com/peridot-ai/peridot/main/install.sh | sh
```
