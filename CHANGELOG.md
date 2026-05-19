# Changelog

All notable changes to Peridot Agent are documented in this file.

The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

This is the first dedicated CHANGELOG entry; earlier releases (0.4.x – 0.5.x)
were documented inline in [PERIDOT_SPEC_v1.md](PERIDOT_SPEC_v1.md) and on
[GitHub Releases](https://github.com/dlsxj101/peridot-agent/releases). All
0.6.0 changes are additive — no breaking API removals.

---

## [Unreleased]

### Added — anti-hallucination guardrail

- New `Grounding rules` block in the base system prompt
  (`peridot-core/src/prompt.rs::system_prompt_for_mode`), applied to
  every mode and every role. Forces the model to read source with
  file_read / file_outline / file_search before answering "how does X
  work?" questions, requires a concrete `path:line` (or tool name +
  quote, or URL + quote) citation for every load-bearing factual
  claim, and forbids softening speculation into confident assertions.
  Lives in Section B (Protocol) of the prompt so it stays inside the
  provider cache breakpoint and costs zero per turn after the first
  one. Covered by a `every_mode_prompt_contains_grounding_rules`
  regression test.

### Changed — provider trait surface

- `LlmProvider::pricing()` and `LlmProvider::auth_method()` are no
  longer decorative — both are now consulted by `peridot doctor` via
  the new `provider:pricing` / `provider:auth_method` checks.
  Implemented through a new `crate::providers::inspect_provider`
  helper that builds a provider stub without requiring credentials so
  the canonical pricing table is always reportable.
- `OpenAiProvider::auth_method()` now downgrades to `NotConfigured`
  when `api_key` is absent (previously echoed the stored auth method
  regardless of whether credentials were actually present).
- `OpenAiCodexProvider::auth_method()` now reports `NotConfigured`
  when the OAuth access token is empty (previously always reported
  `OAuth`). The trait method becoming honest is load-bearing for the
  doctor's "right config, just no keys yet" signal.

### Documented

- `LlmProvider::supports_prefill()`: doc comment now explicitly
  records that the method is intentionally not wired into the agent
  loop. Response prefill is Anthropic-only, and Peridot's Claude
  surface is API-key only (Claude OAuth / subscription path is not
  supported), so the lowest-common-denominator stance defers wiring
  until first-class Claude OAuth lands. Provider impls keep returning
  their honest capability so the trait surface stays accurate.
- `LlmProvider::supports_cache()` / `supports_thinking()` /
  `pricing()` / `auth_method()`: doc comments now point to the
  production caller for each so the wiring is discoverable from the
  trait definition.

---

## [0.7.0] — 2026-05-19

Production-quality pass before extension work begins. Twelve targeted
improvements across sandbox safety, context quality, MCP operability,
approval UX, observability, and PR workflow. No breaking API removals;
new fields on `SecurityConfig`, `McpServerConfig`, and `ContextEntry`
all carry `#[serde(default)]` so on-disk sessions and configs from
0.6.x continue to load.

### Added — sandbox & safety

- `security.docker_read_only_rootfs` (`--read-only` + `--tmpfs /tmp`)
  so Docker-sandboxed shell commands can't pollute the container fs
  outside `/workspace`.
- `security.docker_memory_limit` (e.g. `"512m"`) forwarded as
  `--memory` so a runaway container gets OOM-killed instead of pinning
  the host.
- `security.shell_command_timeout_seconds` (default `0` = unlimited).
  When set, `shell_exec` kills the child via the same path as Esc
  cancel and reports a recoverable timeout error.
- `security.shell_dry_run` returns a synthetic `ToolResult` describing
  the would-be invocation (program + args + cwd) without actually
  launching it. Useful for safety drills and CI smokes.

### Added — context quality

- Pinned memory: `ContextEntry.pinned` plus
  `ContextManager::append_pinned`, `pinned_count`, `unpin_where`.
  Pinned entries survive both deterministic and LLM-driven compaction.
- More accurate token estimator: `estimate_tokens_for_text` swaps the
  legacy `chars/4` heuristic for a CJK-aware word + punctuation + long-
  identifier scheme that lands within 5-10% of real BPE counts on
  representative mixed inputs (no new dependency).
- Content-aware tool-output digesting: unified diffs collapse to hunk
  count + filenames, stacktraces collapse to anchor + first 2 frames,
  test output collapses to the result line + first failure. Driven by
  `digest_string_content` and consumed by every compaction path.

### Added — MCP operability

- `McpClient::list_tools()` is now schema-cached per-server with a
  configurable TTL (`McpServerConfig::schema_cache_seconds`, default
  300s). `invalidate_tools_cache()` for explicit refresh.
- `McpClient::health_check()` returns measured latency for a probe call.
- `McpServerConfig::default_permission` + `tool_permission_overrides`
  let the operator drop a server (or a single tool) from the default
  "Everything is System" gating down to `read` / `write` /
  `destructive`. Resolved by `resolve_mcp_permission_level`.
- MCP tool calls now write to `audit.jsonl` with the resolved
  permission level alongside the existing `params` payload.
- New `peridot mcp doctor` subcommand: runs validate + health probe +
  tool count across every configured server in one shot.

### Added — approval & recovery

- Permission-denied errors now get a dedicated `recovery_message`
  branch instead of rotating through the generic templates. The
  directive explicitly forbids retrying the same call and steers the
  model toward read-only alternatives or an `agent_ask_user` escape
  hatch.

### Added — observability

- New `peridot doctor` subcommand: end-to-end health audit covering
  `.peridot/` layout, provider auth (per primary), models config,
  AGENTS metadata, MCP servers, and security posture. Returns non-zero
  on any fail so it composes with shell pipelines.

### Added — PR workflow

- New `peridot ship` subcommand: branch → commit → push → PR in one
  call. Refuses to land on `main` / `master` / `trunk` unless
  `--allow-protected-branch` is passed. `--no-pr` skips the `gh pr
  create` step for safer dry runs.

### Added — test coverage

- Mock-LLM e2e regressions in `peridot-core/src/tests/harness.rs`:
  pending_resume sidecar round-trip and AGENTS.md hot reload.
- Serde compat regressions for `ContextEntry`: legacy (no `pinned`)
  and forward-compat (with `pinned`) payloads both round-trip.

### Added — auto-fix smarts

- `VerifyFailureState` now carries `hints: Vec<String>` — `file:line`
  tokens extracted from the verifier output. The directive surfaces
  them as "Likely culprit(s)" so the model jumps straight to the
  failing file. Recognises Rust (`src/foo.rs:12:5`), Python (`File
  "src/foo.py", line 12`), TypeScript / JS / Go (`foo.ts:12`).

### Added — scanner reach

- Gradle (`build.gradle` / `build.gradle.kts`, wrapper-aware), Maven
  (`pom.xml`, wrapper-aware), CMake (`CMakeLists.txt`), Swift Package
  Manager (`Package.swift`), and .NET (`*.csproj` / `*.sln`) all flow
  through `peridot scan` with reasonable default build/test commands.

### Changed

- `peridot-core`: extracted `approval_required_error`,
  `is_mutating_tool_name`, `truncate_chars`, `recent_verify_summary`
  into a new `agent_helpers` module so `agent.rs` reads top-to-bottom
  without stepping over stateless utility code.
- `peridot-cli/src/main.rs`: added a `ShipArgs` struct mirroring the
  v0.6.0 `VerifyArgs` pattern so `Command::Ship` typechecks despite
  the rustc 1.95 ICE that fires on inline struct variants with mixed
  optional / boolean flags inside `main`'s match.

### Migration notes

- `McpServerConfig` gained three optional fields. Existing config.toml
  files keep working; the new fields surface their defaults
  (`default_permission = "system"`, `schema_cache_seconds = 300`,
  `tool_permission_overrides = {}`).
- `SecurityConfig` gained four optional fields. Existing config.toml
  files keep working; sandbox behaviour is unchanged until the
  operator opts into `docker_read_only_rootfs`, `docker_memory_limit`,
  `shell_command_timeout_seconds`, or `shell_dry_run`.
- `ContextEntry::pinned` defaults to `false`, so 0.6.x session blobs
  load with no special handling.

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
