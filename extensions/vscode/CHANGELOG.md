# Peridot Agent — Extension Changelog

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
