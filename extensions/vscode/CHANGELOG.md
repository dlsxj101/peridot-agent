# Peridot Agent — Extension Changelog

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
