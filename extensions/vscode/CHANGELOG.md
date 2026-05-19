# Peridot Agent — Extension Changelog

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
