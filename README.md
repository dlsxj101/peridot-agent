# Peridot Agent

Peridot Agent is a Rust CLI/TUI autonomous coding agent. The current implementation is an early, buildable foundation that follows `PERIDOT_SPEC_v1.md`.

## Status

Implemented:

- Cargo workspace with the 13 spec crates.
- Provider-neutral LLM contracts and a Claude Messages API provider.
- Append-only context manager with large-observation offload.
- Built-in file, shell, plan, git, verify, and agent tools.
- AGENTS.md path boundary enforcement.
- Bounded agent loop with deterministic mock provider support.
- Project scanner for Rust, Node, Python, Go, Make, AGENTS metadata, and git state.
- SQLite-backed session summary store.
- Headless CLI commands and initial TUI state model.

## Common Commands

```bash
cargo fmt --all --check
cargo clippy --workspace -- -D warnings
cargo test --workspace
```

```bash
cargo run -p peridot-cli -- --version
cargo run -p peridot-cli -- scan --output json
cargo run -p peridot-cli -- config init
cargo run -p peridot-cli -- session save demo "initial work"
cargo run -p peridot-cli -- session list
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

## Live Claude Loop

Live execution uses `ANTHROPIC_API_KEY` and the configured Claude API base URL:

```bash
ANTHROPIC_API_KEY=... cargo run -p peridot-cli -- run "inspect this project" --live
```

Live model responses must currently follow Peridot's JSON action protocol:

```json
{"action":"file_read","parameters":{"path":"README.md"}}
```

## Project Initialization

```bash
peridot config init
```

This creates `.peridot/config.toml`, `.peridot/hooks/`, `.peridot/skills/`, and gitignore entries for local memory, sessions, logs, and generated skills.
