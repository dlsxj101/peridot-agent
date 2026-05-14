# Contributing

Peridot Agent follows `PERIDOT_SPEC_v1.md` as the product source of truth. Read it before changing crate boundaries, execution modes, security behavior, verification, hooks, skills, or release flows.

## Local Checks

Run these before sending Rust changes:

```bash
cargo fmt --all --check
cargo clippy --workspace -- -D warnings
cargo test --workspace
```

For release or installer changes, also validate shell syntax:

```bash
bash -n install.sh
```

## Development Notes

- Keep the 13 workspace crates narrow and aligned with the spec.
- Put shared types in `peridot-common`.
- Use trait boundaries for providers, tools, subagents, scanners, verification, and persistence.
- Treat command blocklists, path sandboxing, AGENTS boundaries, prompt-injection defense, and external-content tagging as required product behavior.
- Prefer deterministic verification and mock LLM tests over real API tests.
- Do not mark an implementation phase complete unless its spec completion criteria are met.
