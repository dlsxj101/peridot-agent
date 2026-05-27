# Extension Roadmap — v0.8.x

This follows the archived v0.7 roadmap. The next theme is making the
workspace intelligence surfaced in VS Code durable enough for repeated
use, while keeping the daemon/TUI/extension surfaces shared.

## Current Focus

### E5. Persistent Workspace Code Map Index

- **Status**: landed.
- **Goal**: promote `/codemap` from a one-off scan into a workspace-local
  index stored under `.peridot/`, with an explicit refresh command.
- **Where**: `crates/peridot-cli/src/commands/codemap.rs`,
  `crates/peridot-core/src/slash.rs`,
  `crates/peridot-cli/src/commands/daemon.rs`,
  `extensions/vscode/src/extension.ts`
- **Result**: `/codemap` renders from `.peridot/codemap.json` when
  present, creates it on first use, `/codemap refresh` rebuilds it, and
  VS Code exposes refresh from the command palette/sidebar.

### E6. Code Map Search Commands

- **Status**: landed.
- **Goal**: add targeted symbol/TODO search over the persisted index so
  the model and operator can retrieve relevant entries without rescanning.
- **Where**: daemon command catalog, TUI slash picker, extension command
  rendering
- **Result**: `/codemap find <query>` loads the persisted index, filters
  symbols and TODO markers by query tokens, and returns the same
  structured code-map result shape. TUI renders the filtered transcript,
  and VS Code exposes command-palette/sidebar search into the existing
  code-map panel.

### E7. Attachment Follow-up

- **Status**: landed.
- **Goal**: make attachments visible in the sidebar transcript as durable
  context artifacts, not just command-result rows.
- **Where**: `extensions/vscode/webview/index.ts`,
  `crates/peridot-cli/src/commands/attach.rs`
- **Result**: `/attach` daemon results now include structured attachment
  metadata. VS Code renders inlined text and image-placeholder
  attachments as compact blocks with file-open and copy actions.

## Notes

- Keep index data workspace-local. Do not add hosted state.
- Treat LSP/tree-sitter as a later project; E5/E6 should remain a fast
  parser-free cache over the existing lightweight scanner.
- Continue exposing shared behavior through daemon slash/RPC paths so TUI
  and extension do not fork semantics.
