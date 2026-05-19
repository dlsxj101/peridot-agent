# Changelog

All notable changes to Peridot Agent are documented in this file.

The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

This is the first dedicated CHANGELOG entry; earlier releases (0.4.x – 0.5.x)
were documented inline in [PERIDOT_SPEC_v1.md](PERIDOT_SPEC_v1.md) and on
[GitHub Releases](https://github.com/dlsxj101/peridot-agent/releases). All
0.6.0 changes are additive — no breaking API removals.

---

## [0.6.0] — 2026-05-19

### Added

- **Verify pipeline grader integration**: `VerifyPipeline::run_all_with_grader(provider, model, task)` runs the LLM grader after every deterministic stage (build / test / lint / diff-review) passes. Surfaced through `peridot verify --with-grader --grader-task "<text>"`. The grader stage carries the verdict summary and `passed` mirrors the LLM verdict. Skipped automatically when deterministic stages fail so no API tokens are wasted on duplicating a known-negative verdict.
- **`agent_message` built-in tool**: subagents can now route notes to a `parent` or named `child:<session_id>`. Recipients see the message at the start of their next turn as a `[peer message from <id>]` PlanReminder. Backed by the new `AgentMessageBus` trait + `SessionRouter` inbox queue per session. `agent_message` registers in `register_builtin_tools`; the TUI's pre-existing display handler now has a real source to render.
- **`VerifyStage::Lint` variant**: lint failures report as `Lint` instead of being mislabelled as `Deterministic`. Affects `peridot verify` text output and JSON serialisation (`"stage": "lint"`).
- **`peridot-grader` crate**: extracted from `peridot-core/src/grader.rs` so both `peridot-verify` and `peridot-core` can invoke `grade_work` without inducing a dependency cycle. `peridot-core::grader::*` keeps re-exporting `grade_work` / `GraderVerdict` for backward compatibility.
- **Anthropic prompt cache_control automation**: `anthropic_payload_with_cache` stamps three breakpoints when `provider.supports_cache()` is true — last tool definition, system block, and the most recent assistant/tool_result entry. Trailing user prompts stay unmarked so new user input never busts the cache. Skipped automatically for providers (e.g. OpenAI Chat Completions) that disable caching.
- **`SessionRouter::RouterMessageBus`**: shared-router message bus implementation. Provides `send_to_parent` / `send_to_child` / `drain_inbox` with per-session FIFO queues. Includes the safety check that a session can only message its own direct children (no sibling addressing).
- **`HarnessAgent::set_message_bus` / `set_session_id`**: harness drains its inbox at the start of every turn and folds received messages into context as PlanReminders. `InnerLoopSubAgent` intentionally does not propagate the bus to grandchild contexts (depth-1 cap, fork-bomb safety).
- **`VerifyArgs` clap struct**: extracted `peridot verify` flags into a dedicated `Args` struct to work around a rustc 1.95 ICE when inline struct variants with optional boolean flags appeared inside `main`'s match.

### Changed

- **`LlmProvider::supports_thinking()` now consulted by the harness**: `HarnessAgent::run_turn_with_events` AND-gates the thinking flag with `provider.supports_thinking()`, so Goal mode runs against providers that don't support thinking (OpenAI Chat Completions, etc.) no longer send a payload field the server will ignore.
- **`LlmProvider::supports_cache()` now consulted by `ClaudeProvider`**: cache_control marking is gated by this method, making the capability advertisement load-bearing instead of decorative.
- **`LocalSubAgentRunner::Teammate` now provisions a real worktree**: previously returned a placeholder string. Now shares the worktree machinery with `Worktree`; the kinds differ only in lifecycle (long-running) and routing (parent↔child message bus).
- **`LocalSubAgentRunner::Fork` summary clarified**: changed from `"fork subagent prepared"` to `"fork workspace prepared (shared with parent) for task: ..."` so the operator can tell whether the agent loop actually executed (only `InnerLoopSubAgent` does that; `LocalSubAgentRunner` only prepares the workspace).
- **SPEC v1.9 documentation realignment**:
  - 4-Tier compaction notation collapsed to 2-Tier (deterministic + LLM). The original Tier 0 / Tier 2 / Tier 3 stages were absorbed into existing paths and never had distinct implementations.
  - Append-Only principle restated as **in-turn only**. Compaction is explicitly allowed to reconstruct the entries vec; the last substantive user/tool result is preserved via `preserved_anchor` + `COMPACTION_KEEP_TAIL`.
  - Tool list (SPEC §7.2) corrected: `agent_fork` / `agent_worktree` collapsed into `agent_delegate(kind=...)`; the 9 tools added since v0.5.0 (`file_outline`, `symbol_search`, `workspace_symbols`, `git_push`, `gh_pr_*`, `skill_list`, `skill_view`) are now listed alongside the new `agent_message`.
  - Plan-mode blocklist (SPEC §2.1) updated: `agent_fork`/`agent_worktree` removed, `agent_delegate` added.
- **Workspace version**: 0.5.1 → 0.6.0 (minor bump for additive feature surface — new crate, new tool, new trait, new CLI flags; no breaking removals).

### Fixed

- **`run_lint()` no longer mislabels lint failures as `Deterministic`**: the stage tag now matches the actual check.
- **Provider capability methods are no longer dead code**: `supports_cache()` and `supports_thinking()` are now reachable from production paths. `supports_prefill()` / `pricing()` / `auth_method()` remain trait obligations but stay decorative pending future work.

### Migration notes

- All API changes are additive. Existing callers of `peridot_core::grader::*` keep working through re-exports.
- The Anthropic wire payload now carries `cache_control` markings by default. The response shape already surfaced `cache_creation_input_tokens` / `cache_read_input_tokens` (parsed since v0.4.x), so dashboards and logs continue to work; expect non-zero cache stats starting with the second turn of any session.
- `peridot session show <id>` continues to render the same shape; no on-disk format changed.
- `peridot verify` (no flags) is identical to v0.5.1. Grader behaviour only activates when `--with-grader` is passed together with `--grader-task <TEXT>`.
- Existing `agent_delegate` callers see no API change; only the placeholder summary text shifted for `kind=fork` and `kind=teammate`.

---

## Earlier versions

See [PERIDOT_SPEC_v1.md](PERIDOT_SPEC_v1.md) version history (v1.0 – v1.8) for
0.4.x and 0.5.x change notes, and the [GitHub Releases](https://github.com/dlsxj101/peridot-agent/releases)
page for download artefacts.
