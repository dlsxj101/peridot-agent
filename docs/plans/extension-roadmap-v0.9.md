# Extension Roadmap — v0.9.x

This follows the archived v0.8 roadmap. The next theme is turning
workspace/session artifacts into durable, inspectable objects across the
shared daemon, TUI, and VS Code extension surfaces.

## Current Focus

### E8. Session Attachment Inventory

- **Status**: landed.
- **Goal**: make files already attached to a session discoverable after
  the initial `/attach` command result has scrolled away.
- **Where**: `crates/peridot-cli/src/commands/attach.rs`,
  `crates/peridot-core/src/slash.rs`,
  `crates/peridot-cli/src/commands/daemon.rs`,
  `extensions/vscode/webview/index.ts`
- **Result**: `/attachments` reconstructs attachment artifacts from the
  active session context snapshot. TUI prints a compact inventory, and
  VS Code renders a session attachment list with file-open and copy
  affordances.

### E9. Attachment Lifecycle Controls

- **Status**: landed.
- **Goal**: let operators remove stale attachments from the active
  session context without manually editing `.peridot/sessions`.
- **Where**: context snapshot mutation helpers, daemon slash command
  handling, TUI transcript confirmation, extension attachment inventory.
- **Result**: `/detach <path>` removes matching attachment PlanReminder
  entries from the current session context. TUI reports removals, daemon
  JSON returns removed and remaining artifacts, and VS Code attachment
  cards expose a confirm-before-detach action.

### E10. Session Artifact Export

- **Status**: landed.
- **Goal**: let operators export session artifacts such as attachments,
  notes, and replay timeline data into a portable directory without
  hand-copying files from `.peridot/sessions`.
- **Where**: `peridot session export`, daemon command results, VS Code
  command palette/sidebar affordances.
- **Result**: `peridot session export` preserves the full-copy default
  and adds repeatable `--artifact attachments|notes|timeline|full`
  selectors. `/export [attachments|notes|timeline|full]` runs through
  the shared TUI/daemon slash path and writes to `.peridot/exports`.
  VS Code can export the active session's attachments, notes, and replay
  timeline from the command palette, sidebar header, or composer slash,
  then reveals the generated portable directory.

### E11. Stale Worktree Reconciliation

- **Status**: landed.
- **Goal**: make crash-leftover worktrees visible and safe across TUI and
  editor clients.
- **Where**: shared `peridot-cli` worktree reconciler, TUI startup, daemon
  `peridot.status`, VS Code sidebar context.
- **Result**: sessions still marked `Running` after an unclean shutdown are
  downgraded to `Suspended`. Clean Peridot-managed worktrees under
  `.peridot/worktrees/` are removed automatically, missing worktree records are
  reconciled, and dirty worktrees are preserved with a TUI / VS Code warning.

### E12. Session Lifecycle Count Slash

- **Status**: landed.
- **Goal**: let operators check persisted session lifecycle totals from the
  same composer/slash surface they use for session list and export.
- **Where**: `peridot session count`, shared slash catalog, TUI host command
  handling, daemon `session.command`, VS Code command-result rendering.
- **Result**: `/session count` now reports total / idle / running /
  suspended / done / failed counts in the TUI transcript and returns a
  structured `session_count` command result for VS Code.

### E13. Session Info Slash Parity

- **Status**: landed.
- **Goal**: make `/info` useful from the VS Code composer instead of
  returning a local-placeholder command result.
- **Where**: daemon `session.command`, shared slash state, VS Code
  command-result typing.
- **Result**: `/info` now returns a structured `info` command result
  containing session id, workspace, provider, model, mode, permission,
  reasoning, service tier, turn count, token total, and cost total. TUI
  keeps its existing local transcript summary while editor clients get
  daemon-backed data from the same command path.

### E14. Cost Slash Parity

- **Status**: landed.
- **Goal**: make `/cost` useful from the VS Code composer and keep it
  aligned with TUI aggregate usage semantics.
- **Where**: daemon live session bookkeeping, `session.command`, VS Code
  command-result typing.
- **Result**: `/cost` now returns a structured `cost` command result
  with current-session usage, aggregate executor usage, committee role
  usage, total all-in tokens/cost, per-session rows, and the active
  budget cap when one exists. Running daemon sessions track
  `UsageUpdated`, `BudgetUpdated`, and `CommitteeRoleUsage` events so
  editor clients do not have to wait for persisted records at run end.

## Notes

- Keep attachment state session-local. Do not introduce hosted state.
- Prefer reconstructing artifact metadata from existing session context
  entries until there is a stronger reason to add a separate attachment
  registry file.
- Continue routing through daemon slash/RPC paths so TUI and extension
  behavior stays shared.
