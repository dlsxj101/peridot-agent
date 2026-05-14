# Rust Workspace Guidelines

## Crate Boundaries
- `peridot-common`: shared errors, IDs, tool/result enums, config primitives, and small utilities.
- `peridot-cli`: clap parsing, setup/login/update/session commands, headless entrypoints.
- `peridot-tui`: Ratatui rendering, input handling, keybindings, layout state.
- `peridot-core`: agent loop, state machine, mode orchestration, recovery wiring.
- `peridot-llm`: provider traits, Claude/OpenAI providers, auth, streaming, caching, usage.
- `peridot-context`: append-only history, compaction, offload, message construction.
- `peridot-tools`: tool trait, registry, built-in tools, permission checks, hook invocation.
- `peridot-project`: scanner, AGENTS parsing, ProjectProfile, boundaries.
- `peridot-verify`: deterministic checks, build/test/lint, diff review, grader interface.
- `peridot-git`: git status/diff/log/commit/branch/worktree helpers.
- `peridot-memory`: SQLite stores, session persistence, learned skills/errors.
- `peridot-agents`: fork/worktree/teammate subagent orchestration.
- `peridot-mcp`: MCP stdio/http clients and schema-to-tool adaptation.

## Dependency Direction
- Prefer dependencies flowing from application crates into core/domain crates, not the reverse.
- Keep `peridot-common` lightweight; do not turn it into a dumping ground for behavior.
- Avoid cycles by moving shared interfaces into the narrowest crate that owns the abstraction.
- Provider-specific details must not leak into core state types unless represented as generic capability flags.

## Traits And Types
- Define traits at the boundary where callers need polymorphism.
- Keep trait methods async only when real implementations require async.
- Use typed structs for internal contracts; use `serde_json::Value` only at tool/provider wire boundaries.
- Use deterministic ordering (`BTreeMap`, sorted vectors) for serialized tool definitions and prompt inputs.

## Errors
- Use `thiserror` for crate-level domain errors.
- Use `anyhow::Result` in binaries and top-level orchestration where context matters more than matching.
- Preserve original error context. Recovery depends on accurate error classification.
- Avoid stringly typed errors for permission, parsing, context, and provider failures.

## Async
- Use `tokio`.
- Make cancellation and timeout paths explicit for shell, provider, MCP, hook, and subagent work.
- Do not hold locks across `.await` unless the lock is designed for it and the critical section is tiny.

## Public API Hygiene
- Add doc comments for public functions, traits, structs, and enum variants.
- Keep public fields rare; prefer constructors when invariants matter.
- Add tests near the crate that owns the behavior.
- Split modules before a file mixes unrelated concerns or approaches 500 lines.
