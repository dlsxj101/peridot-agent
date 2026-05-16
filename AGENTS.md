# Peridot Agent Development Guide

This is the canonical instruction file for coding agents working on Peridot Agent.

## Project
name: Peridot Agent
description: A Rust CLI/TUI autonomous coding agent combining Manus-style harness engineering with Claude Code/Codex-style coding interfaces.
license: MIT

## Required Reading
- Read [PERIDOT_SPEC_v1.md](PERIDOT_SPEC_v1.md) before starting implementation.
- Treat the spec as the source of truth for architecture, crate boundaries, modes, security, verification, hooks, skills, and release sequence.
- If the spec and these instructions disagree, pause long enough to identify the conflict. Prefer the spec for product behavior and this file for repository workflow.

## Session Start Checklist
- Confirm the current implementation session from the spec's seven-session implementation guide.
- Inspect existing files before editing; this repo may have unrelated user changes.
- If `Cargo.toml` exists, run or review `cargo build --workspace` status before broad changes.
- Keep each turn scoped to one logical implementation unit.
- Use project skills in `.peridot/skills` when the task matches their descriptions.

## Session End Checklist
- Update or leave a clear handoff for incomplete work.
- For Rust code changes, run:
  - `cargo fmt --all --check`
  - `cargo clippy --workspace -- -D warnings`
  - `cargo test --workspace`
- If the workspace skeleton does not exist yet, note that cargo verification was not applicable.
- Do not mark a phase complete unless its spec completion criteria are met.

## Rust Rules
- Use a Cargo workspace with the 13 crates described in the spec.
- Keep crate responsibilities narrow; shared types belong in `peridot-common`.
- Use trait boundaries for providers, tools, subagents, scanners, verification, and persistence.
- Use `tokio` for async runtime.
- Use `thiserror` for domain errors and `anyhow` at application boundaries.
- Add doc comments for public functions and public types.
- Split files before they become large or mixed-purpose; 500 lines is a strong warning sign.

## Security Rules
- Never bypass command blocklists, path sandboxing, or AGENTS boundaries for convenience.
- Treat prompt-injection defense and external-content tagging as product requirements, not optional polish.
- Dangerous commands, dependency installs, publication, force pushes, and destructive git operations require explicit user approval unless the implemented permission system safely handles them.
- Hook failures should be useful, not noisy. Prefer warnings and clear logs unless blocking is essential.

## Verification Rules
- Prefer deterministic checks before LLM-based graders.
- Mock LLM tests are the default for integration behavior; real API E2E tests must be budgeted and isolated.
- Every implementation phase should leave the workspace buildable and testable.

## Skill And Hook Improvement
- Skills and hooks are living development aids. If one becomes slow, duplicated, noisy, stale, or inaccurate, update it in the same change that reveals the problem.
- Measure before expanding hooks. Default to no-op fallbacks when prerequisites are missing.
- Keep `on_failure = "block"` rare; use it only for checks that protect repository integrity.
- Only turn repeated manual work into a skill. Delete or merge skills that stop earning their context cost.

## TUI Multi-Session Notes
- The TUI uses a five-marker vocabulary (`▸ ◆ ❯ ✔/✘ ⚠`); never reintroduce decorative icons.
- All user-facing strings flow through `peridot_tui::tr(PhraseKey, Locale)` — adding a new visible string requires a new `PhraseKey` arm in both English and Korean tables.
- `AgentRunEvent` variants must round-trip through `peridot-cli/src/main.rs` adapter into a matching `TuiRuntimeEvent` arm; adding one without updating the adapter silently drops data.
- Multi-session work is gated by `peridot-cli/src/session_router.rs` — read `.peridot/skills/multisession-orchestration/SKILL.md` before changing its public surface.
- Multi-LLM committee work (Planner / Reviewer / Executor roles, `/committee` slash, `committee.ndjson` persistence) is gated by `.peridot/skills/committee-orchestration/SKILL.md` and `docs/agents/committee-runbook.md`. Mode defaults to Off; opting in is per-session.
- Schema additions to `TuiState`, `SessionRecord`, or `AgentRunEvent` need `#[serde(default)]` (struct fields) or new enum tags (variants) so disk-resumed sessions keep loading.

## Reference Docs
- [Implementation Playbook](docs/agents/implementation-playbook.md)
- [Rust Workspace Guidelines](docs/agents/rust-workspace-guidelines.md)
- [Security And Permissions](docs/agents/security-and-permissions.md)
- [Verification](docs/agents/verification.md)
- [Skill And Hook Maintenance](docs/agents/skill-hook-maintenance.md)
- [Multi-Session Runbook](docs/agents/multisession-runbook.md)
- [Committee Runbook](docs/agents/committee-runbook.md)
