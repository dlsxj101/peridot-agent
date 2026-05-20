# Peridot Agent — Extension Changelog

## [0.5.0] — 2026-05-20

### Added — platform-specific binary bundling

- The `vsce/v*` release pipeline now builds the `peridot` binary in a
  six-target matrix (`linux-x64`, `linux-arm64`, `darwin-x64`,
  `darwin-arm64`, `win32-x64`, `win32-arm64`) and produces one
  platform-tagged `.vsix` per target via `vsce package --target`. The
  Marketplace and Open VSX serve the matching `.vsix` to each user, and
  the binary lands at `<extension>/resources/peridot[.exe]` so a
  freshly-installed extension can run a task with zero extra setup —
  no manual `cargo build` or `peridot.binaryPath` override.
- Added `npm run bundle-binary` (debug or `--release`) for the local
  workflow: drops the workspace cargo build into `resources/peridot[.exe]`
  so a locally-packaged `.vsix` exercises the same path the release uses.
- Binary lookup priority in `peridotBin.ts` is unchanged:
  `peridot.binaryPath` override → `<extension>/resources/peridot[.exe]`
  → system PATH. Local developers without a bundled binary still fall
  through to `peridot` on PATH.

### Added — webview bundle, HUD, and inline diff preview

- Split the sidebar webview out of `sidebar.ts` into a dedicated `webview/`
  source tree (TypeScript + CSS) bundled by esbuild. The extension host
  bundle (`dist/extension.js`) and the webview bundle (`dist/webview.js`
  + `dist/webview.css`) are now built side-by-side with a single
  `npm run build`, and `vsce package` runs the production build through
  the `vscode:prepublish` hook.
- Added a HUD panel above the transcript that surfaces
  `usage_updated` (tokens / cost), `budget_updated` (cost vs limit, turns
  vs limit), `context_utilization_changed` (4-Tier context bar), and
  `committee_role_usage` (per-role tallies) so those events no longer
  scroll past as noise.
- Added an inline plan panel driven by `plan_updated` events; the
  current step is highlighted and prior steps render with a strikethrough.
- Added inline unified-diff rendering for `file_diff` transcript cards
  using the `diff` package; long diffs collapse to 120px with an
  expand/collapse toggle and the path doubles as an "open in editor"
  button.
- Added a pre-approval diff preview for `file_write` / `file_patch`:
  the extension host reads the target file from the workspace, computes
  the post-mutation content from the tool parameters, and ships
  before/after to the approval card before the operator decides.
- Added an `openFile` webview message so diff cards and approval cards
  can jump to the affected path with `vscode.open`.

### Changed — transcript noise + status latency

- Routed `agents_md_loaded`, `turn_started`, `turn_ended`,
  `assistant_started`, `assistant_finished`, `context_utilization_changed`,
  `usage_updated`, `budget_updated`, and `committee_role_usage` away from
  the transcript (HUD or Output channel instead) so the chat feed only
  carries actionable items.
- Folded `tool_started` and `tool_finished` for the same tool name into a
  single transcript card with a `running` → `done` state transition,
  removing the two-line-per-tool pattern.
- Cached `peridot.status` results for 5 seconds and reused the active
  daemon's RPC channel instead of spawning a fresh `peridot daemon`
  subprocess for every refresh. Workspace changes, login completion, and
  task termination still force a re-read.
- Replaced the "Ready." empty state with workspace / auth aware guidance
  (open a folder, sign in, mode / permission tips).

### Migration notes

- Extension version bumped 0.4.0 → 0.5.0.
- Build pipeline now requires `esbuild` (`devDependencies`) and `diff` /
  `@types/diff` (runtime). The previous `tsc`-only `npm run compile`
  invocation still works for a dev build but emits via esbuild now.
- No daemon-side changes were required; the JSON-RPC surface stays at
  `peridot.{version,status,echo}`, `session.{start,cancel}`,
  `interaction.respond`, `approval.respond`, `shutdown`.

## [0.4.0] — 2026-05-20

### Added — approval resume flow

- Added sidebar approve/deny controls for `approval_requested` events.
- Added daemon `approval.respond` JSON-RPC to resume paused sessions from the
  saved pending tool call after approval.
- Added approval scopes (`once`, `command`, `path`, `session`) for the editor
  approval path.

### Changed — transcript cleanup

- Removed the duplicate run-start transcript line by relying on the core
  `run_started` event.
- Rendered `agent_ask_user` tool-start details as a compact prompt summary
  instead of raw JSON.

## [0.3.0] — 2026-05-20

### Added — interactive control plane

- Wired daemon-backed `agent_ask_user` requests through the sidebar with
  inline answer cards for free-form, single-select, and multi-select prompts.
- Added `interaction.respond` JSON-RPC so editor clients can resolve pending
  agent questions without restarting the run.
- Added sidebar run controls for execution mode, permission mode, and optional
  model override.
- Added compact transcript cards for `approval_requested` and `file_diff`
  events so operator-blocked tool calls and file mutations are visible in the
  editor panel.

## [0.2.0] — 2026-05-20

### Added — usable sidebar status

- Added `peridot.status` daemon RPC for editor clients to read workspace,
  provider, model, permission, daemon version, and auth readiness.
- Added sidebar workspace/provider/model/auth badges.
- Added ChatGPT login and status refresh actions to the sidebar and Command
  Palette.
- Reduced raw JSON event noise in the transcript by rendering common daemon
  events as compact human-readable status lines.

## [0.1.1] — 2026-05-20

### Fixed — sidebar webview registration

- Marked the Chat view contribution as a WebView so VS Code/Cursor binds it
  to `registerWebviewViewProvider` instead of looking for a tree data provider.

## [0.1.0] — 2026-05-20

### Added — sidebar chat panel

- Added a Peridot Activity Bar container with a `Chat` WebView view.
- The sidebar can submit a task directly to `session.start` and stream
  daemon events into a transcript.
- The sidebar can cancel the current daemon session through
  `session.cancel`.
- Command Palette `Peridot: Run Task` and `Peridot: Cancel Current Task`
  now use the same sidebar-aware execution path.
- The extension runs in the workspace extension host so WSL/Cursor remote
  sessions resolve the daemon inside the active workspace environment.

## [0.0.4] — 2026-05-20

### Added — first task-run bridge

- `Peridot: Run Task` command prompts for a task, spawns `peridot daemon`,
  calls `session.start`, and streams daemon `event` notifications into the
  `Peridot` Output Channel.
- `Peridot: Cancel Current Task` sends `session.cancel` for the active
  daemon session.
- The extension JSON-RPC client now dispatches server-pushed
  notifications instead of dropping id-less daemon messages.

## [0.0.3] — 2026-05-20

### Changed — release pipeline retry build

No runtime behaviour changes from v0.0.2. This release exists to
exercise the fixed GitHub Release asset-upload permission after the
first `vsce/v0.0.2` publish run successfully reached the registries
but failed while attaching the packaged `.vsix` to the GitHub Release.

### Documentation

- Clarified that `peridot.binaryPath` falls back to a bundled binary
  only when one is present, then to `peridot` on the system PATH.
- Updated the extension README status from the original v0.0.1
  scaffold wording to the current Phase 0 verification surface.

## [0.0.2] — 2026-05-19

### Fixed — commands invisible in Cursor's Command Palette

Some Cursor builds (and older VS Code 1.74 hosts) don't auto-derive
activation events from `contributes.commands`, so v0.0.1's empty
`activationEvents` left the extension dormant: install succeeded, the
extension showed in the Extensions view, but `>Peridot` in the Command
Palette returned "No matching commands" because the host never asked
the extension to register them.

Explicitly opt into `onStartupFinished` so every host activates the
extension after window load. Adds a tiny startup cost (one
`activate()` call) but guarantees commands are registered on every
target editor — including Cursor builds where the implicit
contributes-derived activation didn't fire.

## [0.0.1] — 2026-05-19

Initial scaffold release. Goal: verify the publish + spawn + JSON-RPC
pipeline end-to-end before real agent work lands.

### Added

- `Peridot: Hello` command (sanity check that the extension activated).
- `Peridot: Check Daemon Version` command — spawns the bundled
  `peridot daemon` subprocess, sends `peridot.version`, displays both
  the daemon and extension versions.
- `peridot.binaryPath` setting for local-build overrides.
- JSON-RPC 2.0 client (`src/daemon.ts`) ready to grow into the v0.1.0
  agent driver — `send(method, params)` already handles request /
  response correlation and graceful shutdown.

### Distribution

- Published via `vsce` to the VS Code Marketplace under
  `dlsxj101.peridot-vscode`.
- Published via `ovsx` to the Open VSX Registry under the same id so
  Cursor / VSCodium / code-server users can install the same build.
- Platform-specific binary bundling lands in v0.0.2 (Phase 0 ends
  there); v0.0.1 expects the `peridot` binary on the system PATH.
