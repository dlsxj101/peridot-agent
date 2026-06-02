# Contributing

Peridot Agent follows `PERIDOT_SPEC_v1.md` as the product source of truth. Read it before changing crate boundaries, execution modes, security behavior, verification, hooks, skills, or release flows. For agent-workflow rules also read [`AGENTS.md`](AGENTS.md); for a user-facing overview see [`docs/user-guide.md`](docs/user-guide.md).

## Toolchain

The workspace MSRV is **Rust 1.95** (`workspace.rust-version` in `Cargo.toml`). `rust-toolchain.toml` selects the `stable` channel, so your default `stable` must be **≥ 1.95**. If `cargo` reports `rustc <older> is not supported`, install a current toolchain:

```bash
rustup toolchain install 1.95.0   # or newer
rustup component add clippy rustfmt --toolchain 1.95.0
```

## Local Checks

Run these before sending Rust changes (they are the same gate CI enforces):

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

For release or installer changes, also validate shell syntax:

```bash
bash -n install.sh
```

> Note: a few tests create temporary git repositories and make real commits
> (`peridot-git`, `peridot-agents`, `worktree_cleanup`, `ship`, the
> `peridot-verify` AGENTS-boundary test). In sandboxes that force commit
> signing through an external signing server, these can fail at the commit
> step even though the code is correct — that is an environment artifact,
> not a regression.

## Workspace Map

15 narrow crates (`crates/`):

| Crate | Responsibility |
|-------|----------------|
| `peridot-cli` | CLI subcommands, TUI host, `peridot daemon` JSON-RPC server |
| `peridot-core` | Deterministic agent loop, slash parsing, modes |
| `peridot-context` | 2-tier context manager, branching journal |
| `peridot-llm` | Provider-neutral LLM contracts (Anthropic, OpenAI, Codex OAuth, OpenRouter) |
| `peridot-tools` | File / shell / git / verify / agent tools |
| `peridot-verify` | Deterministic verification pipeline |
| `peridot-grader` | LLM-based grading |
| `peridot-agents` | Sub-agent runners |
| `peridot-memory` | SQLite session/skill store + curator |
| `peridot-project` | Project scanner, code map |
| `peridot-symbols` | Tree-sitter semantic symbol extraction (F1) |
| `peridot-mcp` | MCP client (stdio + HTTP) |
| `peridot-git` | Git automation / worktrees |
| `peridot-tui` | Ratatui UI, slash picker, rendering, i18n |
| `peridot-common` | Shared types |

The VS Code extension lives in `extensions/vscode` and talks to
`peridot daemon` over line-delimited JSON-RPC 2.0.

## Development Notes

- Keep the workspace crates narrow and aligned with the spec; put shared types in `peridot-common`.
- Use trait boundaries for providers, tools, subagents, scanners, verification, and persistence.
- Split files before they grow mixed-purpose; **500 lines is a strong warning sign** (see the active tech-debt items in [`docs/plans/roadmap-v1.0.md`](docs/plans/roadmap-v1.0.md)).
- Route user-facing commands through the daemon slash/RPC path so TUI and the extension stay behavior-identical.
- All user-facing TUI strings flow through `peridot_tui::tr(PhraseKey, Locale)` — a new visible string needs a new `PhraseKey` arm in both English and Korean.
- Schema additions to `TuiState`, `SessionRecord`, or `AgentRunEvent` need `#[serde(default)]` (fields) or new enum tags (variants) so disk-resumed sessions keep loading.
- Treat command blocklists, path sandboxing, AGENTS boundaries, prompt-injection defense, and external-content tagging as required product behavior — never bypass them for convenience.
- Prefer deterministic verification and mock-LLM tests over real API tests.
- Do not mark an implementation phase complete unless its spec completion criteria are met.

## Planning Docs

- Active roadmap: [`docs/plans/roadmap-v1.0.md`](docs/plans/roadmap-v1.0.md)
- Completed extension roadmaps: `docs/plans/extension-roadmap-v0.9.md` and `docs/plans/archive/`
- Agent runbooks: [`docs/agents/`](docs/agents/)
