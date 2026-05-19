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
- Stage 1: deterministic checks such as file existence, syntax, and local lint hints (`VerifyStage::Deterministic`).
- Stage 2: project build command from `ProjectProfile` (`VerifyStage::Build`).
- Stage 3: project tests from `ProjectProfile` (`VerifyStage::Test`).
- Stage 4a: project lint / typecheck (`VerifyStage::Lint`, v0.6.0). Lint failures used to be mislabelled as `Deterministic`; they now show as `Lint`.
- Stage 4b: diff review against intended changes (`VerifyStage::DiffReview`). Also enforces AGENTS.md boundary patterns.
- Stage 5: LLM grader using the task description, captured `git diff HEAD`, and the deterministic summary as inputs (`VerifyStage::Grader`).

### Stage 5 (Grader) — invocation

`VerifyPipeline::run_all()` runs stages 1-4 only. To include the grader, call `run_all_with_grader(provider, model, task)` or invoke `peridot verify --with-grader --grader-task "<text>"`. The grader is skipped automatically when any deterministic stage fails — the agent already has a verdict and burning an API call to repeat "yes, it's still broken" wastes budget.

The grader itself lives in the `peridot-grader` crate so both `peridot-verify` (CLI verify pipeline) and `peridot-core` (agent loop's `auto_grade_on_done` gate) can call it without a dependency cycle. `peridot_core::grader::*` re-exports the public API.

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
