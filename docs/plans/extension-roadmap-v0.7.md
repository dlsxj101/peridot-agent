# Extension Roadmap — v0.7.x

This replaces the archived v0.6 roadmap now that settings polish,
Hermes skill parity, session sync, walkthroughs, skill autocomplete,
and compaction visualization have landed.

## Current Focus

### E1. Workspace Code Map Entry Point

- **Status**: landed.
- **Goal**: make the shared `/codemap` capability discoverable from VS
  Code without requiring users to know the slash command.
- **Where**: `extensions/vscode/package.json`,
  `extensions/vscode/src/extension.ts`, `extensions/vscode/src/sidebar.ts`,
  `extensions/vscode/webview/index.ts`
- **Result**: the command palette and sidebar header can run a code map
  scan, then append the structured result rows to the current transcript.

### E2. Code Map Panel Polish

- **Status**: next.
- **Goal**: promote `codemap` rows from generic command rows into a
  compact symbol/TODO explorer with source chips and file-open affordance.
- **Where**: `extensions/vscode/webview/index.ts`,
  `extensions/vscode/webview/style.css`
- **Done when**: symbols and TODOs are visually grouped, searchable in
  the panel, and still use the existing `openFile` bridge.

### E3. Extension PR Workflow Surface

- **Status**: next.
- **Goal**: expose the existing GitHub PR workflow tools from VS Code as
  explicit command palette entries and sidebar actions.
- **Where**: daemon command catalog, `extensions/vscode/src/extension.ts`,
  sidebar command rendering
- **Done when**: operators can open/status/merge PRs through the
  extension without dropping into the terminal, while retaining safe-mode
  approval gates.

### E4. Attachment UX Spike

- **Status**: planned.
- **Goal**: design the extension side of `/attach <path>` and image/file
  paste before the provider-level multimodal project starts.
- **Where**: composer webview, daemon payload schema, persisted session
  transcript
- **Done when**: text-only attachments work as explicit context entries
  and image attachments have a schema-compatible placeholder.

## Notes

- Keep new editor affordances backed by daemon RPC or shared slash command
  behavior so TUI and extension do not drift.
- Do not add hosted state. Sessions remain workspace-local under
  `.peridot/`.
- Prefer command palette entries for discoverability, then sidebar
  affordances once the behavior is stable.
