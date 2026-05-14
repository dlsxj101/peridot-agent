---
name: crate-boundary-review
description: Review Peridot Rust crate ownership and dependency direction. Use when moving code between crates, adding shared types, designing traits, reviewing PRs, or checking for responsibility leaks and circular dependencies.
---

# Crate Boundary Review

## Review Steps
1. Map each changed file to the crate responsibility in `docs/agents/rust-workspace-guidelines.md`.
2. Check whether shared contracts belong in `peridot-common` or in a narrower owning crate.
3. Confirm provider-specific details do not leak into `peridot-core`.
4. Confirm TUI, CLI, git, MCP, memory, and verification behavior remains behind their crate boundaries.
5. Look for cyclic dependency pressure and move only the smallest stable interface needed.
6. Recommend tests in the crate that owns the behavior.

## Signals To Fix
- A crate imports a high-level application crate for a low-level type.
- JSON `Value` spreads beyond tool/provider wire boundaries.
- A public type encodes one provider when the spec calls for provider traits.
- A module grows because unrelated responsibilities are accumulating.
