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

### E15. Plan Show Slash Parity

- **Status**: landed.
- **Goal**: make `/plan show` useful from the VS Code composer instead
  of returning a local-placeholder command result.
- **Where**: daemon live plan bookkeeping, `session.command`, VS Code
  command-result typing.
- **Result**: running daemon sessions now keep the latest `PlanUpdated`
  snapshot. `/plan show` returns a structured `plan` command result with
  done/total counts, current-step metadata, and one command row per plan
  step. TUI keeps its existing local side-panel rendering.

### E16. Session Save Slash Parity

- **Status**: landed.
- **Goal**: make `/session save` an actual daemon-backed persistence
  command from the VS Code composer.
- **Where**: daemon `session.command`, session record persistence, VS
  Code command-result typing.
- **Result**: `/session save` now persists the active daemon session
  record immediately and returns a structured `session_save` command
  result. Live token, cost, and turn totals are copied from the daemon's
  session tracker so explicit saves are useful before a run finishes.

### E17. Goal Control Slash Parity

- **Status**: landed.
- **Goal**: make `/goal pause`, `/goal resume`, `/goal clear`, and
  `/goal status` report daemon-owned goal state in VS Code instead of
  local placeholder status rows.
- **Where**: daemon live goal bookkeeping, `session.command`, VS Code
  command-result typing.
- **Result**: goal-mode daemon sessions now keep objective, status, and
  started timestamp metadata. Goal control slashes return structured
  `goal` command results with objective, status, step progress, and
  session id. `/goal <objective>` keeps the existing task-starting
  extension flow.

### E18. Session Rename/Delete Slash Parity

- **Status**: landed.
- **Goal**: make VS Code `/session rename` and `/session delete` mutate
  the same persisted session state as the TUI/CLI paths instead of only
  changing sidebar-local session cards.
- **Where**: daemon `session.command`, persisted session summaries and
  blobs, VS Code sidebar session reconciliation.
- **Result**: `/session rename <id|title> <new title>` updates the
  persisted session summary/record and any saved TUI blob, returning a
  structured `session_rename` result. `/session delete <id|title>`
  cancels a live daemon run when present, removes persisted records and
  session blobs, returns a structured `session_delete` result, and lets
  VS Code remove the matching local sidebar session by daemon id.

### E19. Semantic Rewind Slash Parity

- **Status**: landed.
- **Goal**: make `/rewind` roll back the model-visible session context,
  not just the local transcript, so the next turn no longer sees the
  rewound exchange.
- **Where**: shared context rewind helper, daemon `session.command`,
  TUI host command queue, VS Code composer draft state.
- **Result**: `/rewind` finds the last user context entry, removes that
  turn and later entries from the session context snapshot, and returns
  the restored prompt. TUI restores the prompt into the input box and
  queues the same context rollback through the host. VS Code receives a
  structured `rewind` result, removes the visible last exchange, and
  refills the composer draft with the restored prompt.

### E20. Deep Clear Slash Parity

- **Status**: landed.
- **Goal**: make VS Code `/clear` match the TUI's fresh-session
  semantics instead of only wiping sidebar-local state.
- **Where**: daemon `session.command`, live daemon session registry,
  persisted session summaries/records/blobs, VS Code clear handling.
- **Result**: `/clear` keeps the existing `action: "clear"` client
  contract, but the daemon now cancels the active session, drops the
  matching live registry entry, removes persisted session state, and
  emits session-list invalidation when anything changed. VS Code then
  clears the local transcript without sending a duplicate cancel request
  after the daemon already handled it.

### E21. Session Close Slash Parity

- **Status**: landed.
- **Goal**: make VS Code `/session close <id|title>` use the same daemon
  lifecycle path as TUI close/delete instead of only removing a local
  sidebar card.
- **Where**: daemon `session.command`, live daemon session registry,
  persisted session summaries/records/blobs, VS Code run bookkeeping.
- **Result**: `/session close` now returns a structured `session_close`
  command result. The daemon resolves the target, cancels and removes a
  live session when present, deletes persisted session state to match
  TUI close semantics, and emits session-list invalidation. VS Code
  removes the matching local sidebar session by daemon id and forgets any
  active run handle after daemon-side cancellation.

### E22. Session Switch Slash Parity

- **Status**: landed.
- **Goal**: make VS Code `/session switch <id|title>` resolve targets
  through the daemon's persisted/live session index instead of only the
  current sidebar-local cards.
- **Where**: daemon `session.command`, session list target resolution,
  VS Code sidebar session materialization.
- **Result**: `/session switch` now returns a structured
  `session_switch` result with resolved daemon id, title, status, and
  running state. VS Code selects an existing matching sidebar session or
  creates a local session card keyed by the daemon id before switching.

### E23. Goal Start Slash Parity

- **Status**: landed.
- **Goal**: make VS Code `/goal <objective>` launch through the shared
  daemon slash result path instead of locally re-parsing the objective.
- **Where**: daemon `session.command`, shared slash state delta, VS Code
  `start_task` handling.
- **Result**: `/goal <objective>` now returns a structured `start_task`
  result labeled `goal` plus a goal-mode state delta. VS Code applies the
  delta and starts the task through the existing daemon `session.start`
  path, while `/goal` with no objective remains a local composer-mode
  toggle.

### E24. Workspace Symbol Locate

- **Status**: landed.
- **Goal**: make the existing persisted code map useful as a quick
  definition-jump surface before a full LSP/tree-sitter implementation.
- **Where**: shared slash parser/catalog, TUI host codemap handler,
  daemon `session.command`, VS Code command palette integration.
- **Result**: `/codemap locate <symbol>` returns ranked symbol locations
  from `.peridot/codemap.json` without including TODO matches. TUI
  prints the ranked locations through the existing codemap report, daemon
  clients get structured `codemap` rows with file/line metadata, and VS
  Code `Peridot: Locate Workspace Symbol` opens the first matching
  indexed definition after appending the result to the sidebar transcript.

### E25. Current File Symbol Outline

- **Status**: landed.
- **Goal**: expose a lightweight file-outline workflow from the persisted
  code map before taking on full LSP/tree-sitter indexing.
- **Where**: shared slash parser/catalog, code map filtering helpers,
  TUI host codemap handler, daemon `session.command`, VS Code sidebar
  header and command palette.
- **Result**: `/codemap outline <path>` returns indexed symbols for one
  workspace file and omits TODO rows. TUI prints the file outline through
  the existing codemap transcript report, daemon clients receive
  structured file/line rows, and VS Code `Peridot: Outline Current File`
  plus the sidebar outline button run the command for the active editor
  file.

### E26. Workspace Symbol References

- **Status**: landed.
- **Goal**: add a pragmatic reference-search workflow on top of the
  persisted code map before full LSP/tree-sitter integration.
- **Where**: shared slash parser/catalog, code map reference scanner,
  TUI host codemap handler, daemon `session.command`, VS Code sidebar
  header and command palette.
- **Result**: `/codemap refs <symbol>` resolves indexed symbol names and
  scans source files for word-boundary textual references while skipping
  known definition lines. TUI prints a references section in the codemap
  report, daemon clients receive structured `reference` rows, and VS Code
  `Peridot: Find Workspace Symbol References` plus the sidebar references
  button render those rows in the existing code-map panel.

### E27. Workspace Code Map Status

- **Status**: landed.
- **Goal**: make the persisted code map's freshness visible before adding
  background file watching or incremental indexing.
- **Where**: shared slash parser/catalog, code map status helper, TUI host
  codemap handler, daemon `session.command`, VS Code sidebar header and
  command palette.
- **Result**: `/codemap status` checks `.peridot/codemap.json` without
  creating it, compares its generated timestamp with indexable source-file
  mtimes, and reports `missing`, `fresh`, or `stale` plus source/indexed
  file counts. TUI prints the status in the transcript, daemon clients
  receive a structured `codemap_status` result, and VS Code renders a
  status card with a refresh action when the index is stale or missing.

### E28. Stale-Aware Code Map Auto Refresh

- **Status**: landed.
- **Goal**: prevent TUI and extension users from seeing stale code-map
  results after source files change.
- **Where**: shared code map index loader, daemon `session.command`, TUI
  host codemap handlers, VS Code code-map result chips.
- **Result**: `/codemap`, `/codemap find`, `/codemap locate`, `/codemap
  outline`, and `/codemap refs` now compare the persisted index timestamp
  plus walked-file count and source fingerprint with the current indexable
  source inventory before returning results. Missing or stale indexes are
  rebuilt automatically through the shared loader, including after rapid
  same-second edits and source-file deletion, while explicit `/codemap
  status` remains a non-mutating check.

### E29. Skill Inventory Slash Parity

- **Status**: landed.
- **Goal**: make stored skills discoverable from the same shared
  composer surface that can already invoke `/skill-name`.
- **Where**: shared slash parser/catalog, TUI host skill inventory
  handler, daemon `session.command`, VS Code command-result rendering.
- **Result**: `/skills` and `/skills list` now list active stored skills
  through the shared slash path. TUI prints the active inventory with
  scope and pinned markers, daemon clients receive a structured
  `skills` command result, and VS Code renders a skill inventory card
  with copyable slash invocations.

### E30. Skill Inventory Editor Affordances

- **Status**: landed.
- **Goal**: make the new skill inventory reachable without remembering
  the slash command in VS Code.
- **Where**: VS Code command palette, sidebar header actions, shared
  daemon slash execution path.
- **Result**: VS Code now contributes `Peridot: Show Skills` and a
  sidebar header button. Both route through `/skills`, so the editor
  uses the same structured `skills` result as the composer and stays
  aligned with TUI slash behavior.

### E31. Skill Pin Controls

- **Status**: landed.
- **Goal**: let operators protect useful stored skills from automated
  curation without leaving the TUI or VS Code skill inventory flow.
- **Where**: shared slash parser/catalog, TUI host skill command queue,
  daemon `session.command`, VS Code skill inventory card actions.
- **Result**: `/skills pin <name>` and `/skills unpin <name>` now toggle
  the persisted `pinned_at_unix` marker through the shared slash path.
  TUI reports the update in the transcript, daemon clients receive the
  refreshed structured `skills` result, and VS Code skill rows expose
  pin/unpin buttons next to the existing copy action.

### E32. Skill Detail View

- **Status**: landed.
- **Goal**: let operators inspect a stored skill body from the same
  inventory flow that discovers and pins it.
- **Where**: shared slash parser/catalog, TUI host skill command queue,
  daemon `session.command`, VS Code skill inventory card actions.
- **Result**: `/skills show <name>` (alias `/skills view <name>`) now
  returns one skill's description, scope, pinned state, last-used
  timestamp, and body through the shared slash path. TUI prints the
  detail in the transcript, daemon clients receive a structured
  `skill_detail` result, and VS Code skill rows expose a detail button
  with a dedicated body preview card.

### E33. Skill Inventory Search

- **Status**: landed.
- **Goal**: keep stored skills usable as the inventory grows beyond what
  a single list can scan comfortably.
- **Where**: shared slash parser/catalog, `MemoryStore::search_skills`,
  TUI host skill command queue, daemon `session.command`, VS Code command
  palette/sidebar header.
- **Result**: `/skills search <query>` now searches active stored skills
  by name or body text and returns the same structured inventory shape as
  `/skills`. TUI prints filtered matches, daemon clients receive a
  query-tagged `skills` result, and VS Code exposes `Peridot: Search
  Skills` plus a sidebar header search button.

## Notes

- Keep attachment state session-local. Do not introduce hosted state.
- Prefer reconstructing artifact metadata from existing session context
  entries until there is a stronger reason to add a separate attachment
  registry file.
- Continue routing through daemon slash/RPC paths so TUI and extension
  behavior stays shared.
