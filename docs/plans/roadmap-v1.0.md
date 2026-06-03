# Roadmap — v1.0 (Code Health + Beyond-v1 Features)

The v0.6 → v0.9 extension roadmaps are fully landed (E8–E163, all
`landed`). With the TUI ↔ VS Code slash/RPC parity surface complete, the
next milestone shifts from breadth (new slash parity items) to **depth**:
paying down structural debt that has accumulated in the large daemon/TUI
files, and starting the explicitly-deferred Beyond-v1 features from
`PERIDOT_SPEC_v1.md` §21.5.

This document is organized into two tracks:

- **Track C — Code Health** (tech debt found in the 2026-06-02 review).
- **Track F — Features** (spec §21.5 Beyond-v1 items, sequenced).

Each item keeps the existing roadmap shape (`Status` / `Goal` / `Where`)
so it can be checked off the same way as v0.9.

---

## Code Review Snapshot (2026-06-02)

Reviewed at `claude/code-review-roadmap-tx5SA`, workspace `0.8.14`.

**Healthy:**

- `cargo fmt --all --check` clean.
- `cargo +1.95 clippy --workspace --all-targets -- -D warnings` and
  `cargo +1.95 test --workspace` both pass once the correct toolchain is
  used (see C1).
- Architecture matches the spec: 14 narrow crates, trait boundaries for
  providers/tools/subagents, daemon RPC dispatch delegates cleanly
  (19 method arms → dedicated `handle_*` functions).

**Findings (turned into Track C items):**

1. **Toolchain / MSRV drift (C1).** `Cargo.toml` sets
   `rust-version = "1.95"`, but `rust-toolchain.toml` pins only
   `channel = "stable"`. On a host whose `stable` is older than 1.95
   (the review environment had 1.94.1), every `cargo` invocation fails
   with `rustc 1.94.1 is not supported`. The pin should be explicit.
2. **`daemon.rs` monolith (C2).** `crates/peridot-cli/src/commands/daemon.rs`
   is 9,312 lines (5,882 non-test) with ~70 `handle_*` functions in one
   file — far past the 500-line warning in `AGENTS.md`. It mixes RPC
   transport, session lifecycle, skills, codemap, MCP, settings, and
   notes handling.
3. **Other oversized modules (C3).** Non-test code over 500 lines:
   `peridot-tui/src/state.rs` (3,176), `render.rs` (2,474),
   `input.rs` (1,730); `peridot-context/src/lib.rs` (1,660);
   `peridot-common/src/lib.rs` (1,512); `peridot-core/src/agent.rs`
   (1,466); `peridot-cli/src/run_loop.rs` (1,248);
   `peridot-memory/src/lib.rs` (1,244); `peridot-tools/src/tools/file.rs`
   (1,188).
4. **`unwrap`/`expect` in non-test paths (C4).** ~22 real
   `unwrap()`/`expect()`/`panic!` calls outside test modules in
   `daemon.rs` alone (the daemon is a long-lived process where a panic
   takes down every session). Worth an audit + conversion to recoverable
   errors on the request-handling path.
5. **Documentation gaps (C5).** Spec §22 still lists user-guide and
   contributing-guide as open `[ ]`. (License resolved this pass — root
   `LICENSE` added, matching the extension's MIT.)

---

## Track C — Code Health

### C1. Pin the build toolchain to the MSRV

- **Status**: landed (2026-06-02, documentation route).
- **Goal**: stop `stable < 1.95` hosts from failing every cargo command
  with a confusing error.
- **Where**: `rust-toolchain.toml`, `CONTRIBUTING.md`,
  `docs/user-guide.md`.
- **Result**: kept `channel = "stable"` (so contributors on current
  stable get the latest compiler instead of being force-downgraded to a
  pinned patch) but documented the requirement everywhere it surfaces:
  `rust-toolchain.toml` now carries an MSRV comment pointing at
  `CONTRIBUTING.md > Toolchain`, which spells out the `rustup` fix, and
  the user-guide troubleshooting section covers the same error. CI uses
  `dtolnay/rust-toolchain@stable`, whose runner stable is already ≥ 1.95,
  so the cargo `rust-version = 1.95` gate produces a clear message rather
  than a silent failure. A hard version pin was rejected because it would
  force every contributor onto one patch release and trigger an extra
  toolchain download for the common (up-to-date) case.

### C2. Split `daemon.rs` into a module tree

- **Status**: landed (2026-06-02).
- **Goal**: bring the daemon under the spec's "narrow files" rule and
  make RPC handlers reviewable in isolation.
- **Where**: `crates/peridot-cli/src/commands/daemon.rs` →
  `commands/daemon/{mod,session_cmd,inspect,approval,codemap,attach,branch,mcp,skills,notes,tests}.rs`.
- **Result**: `daemon.rs` (9,312 lines) became a directory module. The
  3,400-line test block plus nine cohesive clusters moved into
  submodules; `mod.rs` dropped from 5,882 non-test lines to ~2,500
  (`session_cmd` 984, `inspect` 529, `approval` 454, `codemap` 343,
  `attach` 328, `branch` 280, `mcp` 264, `skills` 242, `notes` 103).
  `dispatch_request` stays the thin router; submodule handlers are
  `pub(super)` and reach shared parent helpers (the `command_result`
  family, `update_session_spec`, context-snapshot helpers, `emit_*`)
  through `use super::*` — Rust lets descendants see private ancestor
  items, so the genuinely shared helpers stayed in `mod.rs` with no
  visibility churn. `resolve_session_target_id` is shared between the
  session and export handlers, so it lives in `session_cmd` and `attach`
  calls it through `super`. Behavior-preserving: `cargo fmt --all
  --check` clean, `clippy --workspace --all-targets -D warnings` clean,
  and the full daemon test suite passes (the only failing workspace
  tests are pre-existing git-commit-signing sandbox artifacts).

### C3. Carve down the other >500-line modules

- **Status**: in progress (first pass landed 2026-06-02); ongoing /
  opportunistic.
- **Goal**: chip away at the largest TUI / context / core files behind
  the daemon.
- **Where**: `peridot-tui/src/{state,render,input}.rs`,
  `peridot-context/src/lib.rs`, `peridot-core/src/agent.rs`.
- **Done so far**: `render.rs` (2,474 lines) became a directory module;
  the side-panel block renderers (request-context / committee / MCP /
  code-map / attachment / notes / goal, plus the welcome / subagent /
  theme helpers) moved into `render/sidebar.rs` (~335 lines).
  `render/mod.rs` keeps `draw`, the status bar, transcript/markdown
  styling, and layout. Shared helpers reach across via `use super::*`;
  `render_subagent_monitor` is `pub(crate)` and re-exported under
  `#[cfg(test)]` so the crate test module's `use super::render::*` still
  resolves it. Behavior-preserving: fmt/clippy clean and the TUI snapshot
  tests pass.
- **Context summarisation carve-out**: `peridot-context/src/lib.rs`
  (2,618 lines) dropped to ~2,219 by moving the deterministic
  summarisation / tool-output digest cluster (19 pure functions:
  `summarize_entries`, `compact_fragment`, the `digest_*` / `looks_like_*` /
  `summarize_*` content-shape helpers, `render_untrusted_content`,
  `append_evidence_footer`, `format_entries_for_summary`, the LLM-recap
  renderers, and `source_name`) into a new `summarize.rs`. The functions
  are `pub(crate)` and re-exported into `lib.rs` with `use summarize::*;`,
  so `ContextManager` call sites and the `#[cfg(test)]` module's
  `use super::*` resolve unchanged. `ContextManager` keeps lifecycle and
  the message post-processing (`merge_consecutive_roles`,
  `repair_tool_call_pairs`, …) stays in `lib.rs`. Behavior-preserving:
  fmt/clippy clean, the full peridot-context suite (47 tests) passes.
- **Plan for the rest**: stays opportunistic, lower priority than the
  feature track. Split `state.rs` (per-domain state), the remaining
  `render` transcript/markdown helpers, `input.rs`, the rest of
  `context/lib.rs`, and `agent.rs` by responsibility **when touching the
  area for other work**, to avoid churn for its own sake.

### C4. Audit non-test `unwrap`/`expect` on the daemon path

- **Status**: landed (2026-06-02).
- **Goal**: keep a single malformed request or missing field from
  panicking the long-lived daemon and killing every concurrent session.
- **Where**: `crates/peridot-cli/src/commands/daemon/*.rs`.
- **Result**: the audit found that the original worry was already
  handled — the daemon's request handlers parse params and propagate
  failures through `emit_error`, so **none** of the ~22 non-test
  `unwrap`/`expect` calls are on the request-parsing path. Every one is a
  `std::sync::Mutex` lock on live-session state (usage / plan / goal /
  approval-snapshot / ask-user-pending) or the session-router mutex,
  which only panics if another thread already panicked while holding the
  lock. Rather than convert idiomatic poison-unwraps into recoverable
  errors (poisoning means a bug already happened), the bare
  `.lock().unwrap()` sites were given descriptive
  `.expect("daemon mutex (<field>) poisoned")` messages to match the
  existing router `expect`s, so a poisoning panic now names the mutex
  involved.

### C5. Author the user and contributing guides

- **Status**: landed.
- **Goal**: close the two open §22 documentation items.
- **Where**: `docs/user-guide.md` (new), `CONTRIBUTING.md` (expanded),
  `PERIDOT_SPEC_v1.md` §22 checkboxes.
- **Result**: `docs/user-guide.md` covers install, first run, execution
  modes, the interactive TUI and headless scripting, slash commands,
  sessions, configuration, permissions/safety, MCP servers, the Git/GitHub
  workflow, verification/auto-fix, memory/skills/AGENTS.md, the VS Code
  extension, and troubleshooting. `CONTRIBUTING.md` documents the toolchain
  requirement (C1), the local fmt/clippy/test gate, the workspace map, and
  the spec-as-source-of-truth rule. Both §22 documentation checkboxes are
  marked done.

---

## Track F — Beyond-v1 Features (spec §21.5)

Sequenced by the spec's own priority recommendation (§21.5.8), minus the
items already completed during v1 (§21.5.9). Each is a multi-week effort;
treat them as separate milestones, not a single release.

### F1. LSP / Tree-sitter symbol index

- **Status**: in progress (first increment landed 2026-06-02).
- **Goal**: replace grep/glob text search with semantic
  `symbol_definition` / `symbol_references` / `symbol_outline` tools so
  the model attaches exact defs/usages instead of whole grep dumps.
- **Where**: new `peridot-symbols` crate, tool registry in
  `peridot-tools`, codemap cache in `peridot-project`.
- **Done so far**: new `peridot-symbols` crate parses **Rust, TypeScript /
  JavaScript / JSX, Python, Go, Java, Ruby, C, C++, C#, PHP, Bash, Scala, Lua,
  Kotlin, Swift, Haskell, Elixir, Zig, OCaml, Dart, Elm, and Julia** with
  tree-sitter and returns structured
  `Symbol`s (kind, name, 1-based line range, container) plus
  identifier-token `Reference`s, behind a `LanguageSymbols` trait with an
  extension dispatcher (`outline_for_extension` /
  `references_for_extension`). `SymbolKind` gained `Class` / `Interface` /
  `Method` / `Variable` for the new languages; TS class methods and Python
  methods carry their class as `container`, TS arrow-function consts are
  recognized as functions; Go methods carry their receiver type, Java
  methods/constructors and Ruby methods carry their class/module as
  `container`; Kotlin/Swift methods and properties carry their enclosing
  type (Swift folds class/struct/enum/extension into one node read via its
  `declaration_kind`; Kotlin interfaces and enums are `class_declaration`
  variants), Haskell type-class signatures carry the class and multi-equation
  functions are de-duplicated, and Elixir `def`/`defp`/`defmacro` (plain
  `call` nodes) carry their `defmodule`. `file_outline` / `workspace_symbols` /
  `symbol_search` use the tree-sitter parse for any supported extension
  (`.rs/.ts/.tsx/.js/.jsx/.mjs/.cjs/.mts/.cts/.py/.pyi/.go/.java/.rb/.c/.cpp/`
  `.cs/.php/.sh/.scala/.lua/.kt/.kts/.swift/.hs/.ex/.exs/.zig/.ml/.mli/.dart/`
  `.elm/.jl`),
  accurate kinds, class/impl association, multi-line-aware positions) and
  keep the line-based heuristic for the rest. The "is this a source file?"
  walk gate now delegates to `peridot_symbols::supports_extension`, so the
  set of walked files can no longer drift behind the grammar set (this also
  brought the already-supported Ruby/C#/PHP/Bash/Scala/Lua files, previously
  excluded by a stale hard-coded list, into the workspace walk). Dedicated `symbol_definition`
  (exact-name defs) and `symbol_references` (AST-aware usages — skips
  comments/strings; word-boundary textual fallback for unsupported
  languages) tools are registered and recommended in the grounding prompt.
  Per-language modules (`rust.rs` / `typescript.rs` / `python.rs`) over
  shared helpers; each has unit tests. Behavior-preserving: fmt/clippy
  clean, full suite green.
- **References** distinguish the definition site from usages: each
  `symbol_references` result is tagged `definition` or `usage` (the
  dispatch entry point cross-references occurrences against the file
  outline). `Reference` carries an `is_definition` flag.
- **Scope resolution (full lexical chain)**: each reference carries the
  fully-qualified scope chain (`outer::…::inner`) of the outline symbols that
  enclose it — every nested module / namespace / type / function body from
  outermost to innermost (e.g. `ui::Widget::render`), exposed as `scope` on the
  `symbol_references` rows. The path is built from the enclosing symbols' line
  ranges and each symbol's `container`, with adjacent duplicates collapsed, so
  a method whose owning type lives only in the `container` field still names
  its owner and a type that appears both as an enclosing node and as a
  container is named once. A definition occurrence reports its *parent* scope
  rather than itself; file-scope occurrences omit it. Computed from the
  existing outline ranges, so it is language-agnostic across every wired
  grammar (verified for Rust nesting and nested Python classes) and tells the
  model exactly which lexical scope a usage lives in.
- **Incremental cache**: `workspace_symbols` / `symbol_search` /
  `symbol_definition` cache each file's parsed outline by absolute path +
  (mtime, size), re-parsing only changed files. The cache is **persisted to
  `.peridot/symbol-cache.json`** (versioned, flushed once per query) and
  reloaded on startup, so a daemon/agent restart warm-starts instead of
  re-parsing the tree. A `notify`-based `SymbolCacheWatcher` (owned by the
  daemon for the workspace lifetime) invalidates cache entries on file
  changes so renamed/deleted files don't linger in the in-process or
  on-disk cache; it's non-fatal if it can't start (queries still re-check
  mtime/size). On a change the watcher **pre-warms** — re-parses the
  changed source file in the background so the next query is warm — bounded
  to 16 files per event (bigger batches like a branch switch just
  invalidate) and skipping non-source/oversized/deleted paths.
- **Remaining**: further language grammars (e.g. Nix, Clojure, Erlang,
  Solidity) via the same dispatcher; binding-level resolution on top of the
  lexical scope chain — resolving shadowing and which declaration a usage
  actually refers to (e.g. a local parameter named `foo` vs. a top-level
  `foo`), which needs per-language declaration tracking; optionally real LSP
  clients. Highest context-savings payoff.

### F2. Multimodal image input (vision routing)

- **Status**: in progress (attach UX + LLM vision layer + end-to-end
  resolver landed; downscale/OCR/config remain). Design doc:
  [`f2-multimodal-vision.md`](f2-multimodal-vision.md).
- **Goal**: actually send attached images to a vision-capable model
  instead of recording placeholder metadata.
- **Where**: `peridot-llm` provider adapters, `peridot-context`
  (`ContextEntry.images` + `to_messages`), `peridot-cli` attach,
  `peridot-core` vision gate.
- **Done so far**: `LlmMessage` carries an additive `images` field +
  `user_with_images` builder; all three provider adapters serialize images
  into their native blocks; `model_supports_vision(model)` capability gate.
  **End-to-end**: `/attach` base64-encodes images (≤5 MB) onto
  `ContextEntry.images`; `to_messages` emits `user_with_images` (role-merge
  keeps them discrete); the core loop strips images for text-only models
  via `enforce_vision_capability`. Tested at each layer.
- **Config**: `[vision] enabled` (core gate via `set_vision_enabled` /
  `enforce_vision_capability`) and `[vision] max_image_bytes` (attach cap,
  both TUI and daemon surfaces) are wired.
- **Surface indicators**: `/attach` reports whether an image is sent to
  vision models or kept as a placeholder (too large); the daemon attach
  response exposes a `vision` boolean for VS Code.
- **Downscaling**: over-cap images are decoded and halved until the JPEG
  re-encoding fits `max_image_bytes` (pure-Rust `image` crate) instead of
  dropping to a placeholder.
- **Remaining**: an OCR text-only fallback and an explicit vision-model
  override. See the design doc milestone 5 (OCR) and `[vision] model`.

### F3. Voice input

- **Status**: planned (spec §21.5.10, deferred to v2).
  Design doc: [`f3-voice-input.md`](f3-voice-input.md).
- **Goal**: dictate prompts via audio capture → transcription.
- **Where**: new optional crate (audio capture via `cpal`,
  transcription via local whisper.cpp or OpenAI Whisper API, VAD).
- **Notes**: lowest priority of the feature track; isolate behind a
  feature flag so the core CLI stays dependency-light. See the design
  doc for the `Transcriber` trait, backends, and TUI push-to-talk plan.

### F4. Web UI / browser client

- **Status**: planned (spec §21.5.10, deferred to v2).
  Design doc: [`f4-web-ui.md`](f4-web-ui.md).
- **Goal**: a browser front-end over the existing daemon RPC.
- **Where**: extends the `peridot daemon` JSON-RPC surface to
  HTTP/WebSocket; separate full-stack project (auth, multi-user, session
  isolation).
- **Notes**: largest effort (4–8 weeks). The daemon RPC contract built
  for the VS Code extension is the foundation; this reuses it over a web
  transport. See the design doc for the WS↔RPC bridge, auth/isolation,
  and the shared-rendering plan with the VS Code webview.

---

## Suggested Order

1. **C1** (toolchain pin) — unblocks reproducible builds, ~tiny.
2. **C5** (docs) — in progress this pass.
3. **C2** (daemon split) — do before adding RPC methods for F-track work.
4. **F1** (LSP/symbols) — biggest model-efficiency win.
5. **C4** (unwrap audit) and **C3** (file carving) — opportunistic,
   alongside whatever area is being touched.
6. **F2 → F3 → F4** as separate milestones.

## Notes

- Keep routing through the daemon slash/RPC path so TUI and extension
  behavior stays shared (carried over from the v0.9 notes).
- Track C items are behavior-preserving; lean on the existing test suite
  to prove no regression, and keep each split as its own commit.
