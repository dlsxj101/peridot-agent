---
name: rust-workspace-skeleton
description: Create or extend the initial Peridot Rust Cargo workspace. Use for Session 1 skeleton work, adding the 13 crates, defining shared traits/types, wiring Cargo.toml members, or making the empty workspace compile.
---

# Rust Workspace Skeleton

## Required Crates
Create the workspace with `peridot-cli`, `peridot-tui`, `peridot-core`, `peridot-llm`, `peridot-context`, `peridot-tools`, `peridot-mcp`, `peridot-verify`, `peridot-agents`, `peridot-memory`, `peridot-project`, `peridot-git`, and `peridot-common`.

## Workflow
1. Add or update root `Cargo.toml` workspace members.
2. Put shared enums, IDs, errors, and simple result types in `peridot-common`.
3. Define traits at crate boundaries before adding behavior-heavy implementations.
4. Keep binary crates thin; delegate behavior to library crates.
5. Add minimal tests only where they prove skeleton contracts.
6. Verify `cargo build --workspace` and `cargo test --workspace`.

## Avoid
- Do not fill crates with placeholder abstractions that the spec does not require.
- Do not duplicate shared types across crates.
- Do not introduce dependencies before their phase needs them.
