# Verification

## Default Local Verification
Run these before calling Rust implementation complete:

```bash
cargo fmt --all --check
cargo clippy --workspace -- -D warnings
cargo test --workspace
```

If the workspace skeleton does not exist yet, say so and run applicable shell syntax or file checks instead.

## Verification Pipeline
- Stage 1: deterministic checks such as file existence, syntax, and local lint hints.
- Stage 2: project build command from `ProjectProfile`.
- Stage 3: project tests from `ProjectProfile`.
- Stage 4: diff review against intended changes.
- Stage 5: grader agent using AGENTS style and boundaries as rubric.

## Test Strategy
- Unit tests cover crate-local logic.
- Integration tests use a mock LLM server and deterministic responses.
- E2E tests use real APIs only behind an explicit feature flag and budget.
- Prefer testing permission and recovery behavior with fixtures rather than live destructive commands.

## Required Test Areas
- Context append-only behavior, compaction, offload, and serialization.
- Tool registry, masking, permissions, hooks, blocklist, path sandbox.
- Provider request serialization, parsing fallback, error handling, usage accounting.
- State transitions, recovery, Goal Checker, structured variation.
- Project scanning, AGENTS parsing, config merge, boundary matching.
- TUI layout and slash command parsing.

## Failure Handling
- Do not hide failed verification. Record what failed and why.
- If a hook is too noisy or too slow, update the hook and document the reason in the same change.
- Do not weaken a test to make a phase pass unless the spec or implementation target changed.
