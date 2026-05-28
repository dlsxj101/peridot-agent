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
  path.

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

### E34. Skill Use From Inventory

- **Status**: landed.
- **Goal**: let operators apply a discovered skill without remembering
  the separate `/skill-name` invocation form.
- **Where**: shared slash parser/catalog, existing skill PlanReminder
  load path, TUI host skill command queue, daemon `session.command`, VS
  Code skill inventory/detail card actions.
- **Result**: `/skills use <name> [args]` now aliases the existing
  `/skill-name [args]` context-injection behavior. TUI routes it through
  the same skill load queue, daemon clients receive the existing `skill`
  command result, and VS Code skill inventory/detail rows expose a
  one-click Use action.

### E35. Skill Archive From Inventory

- **Status**: landed.
- **Goal**: let operators hide stale or noisy active skills without
  leaving the inventory flow or permanently deleting skill files.
- **Where**: shared slash parser/catalog, `MemoryStore::set_skill_archived`,
  auto-skill file archive helper, TUI host skill command queue, daemon
  `session.command`, VS Code skill inventory/detail card actions.
- **Result**: `/skills archive <name>` now marks the skill archived and
  moves matching auto-skill files into `.peridot/skills/archive/` when
  present. TUI refreshes dynamic skill suggestions, daemon clients
  receive a refreshed `skills` result with the archived row removed, and
  VS Code skill inventory/detail rows expose a confirm-before-archive
  action.

### E36. Archived Skill Restore Surface

- **Status**: landed.
- **Goal**: make archive reversible from the same shared skill inventory
  surfaces instead of requiring the standalone `peridot skill restore`
  command.
- **Where**: shared slash parser/catalog, TUI pending session commands,
  daemon `session.command`, existing archived-skill DB/file restore
  helper, VS Code sidebar command palette and inventory rows.
- **Result**: `/skills archived [query]` lists archived skill records and
  `/skills restore <name>` clears `archived_at_unix`, moves archived auto
  skill files back under `.peridot/skills/auto/` when present, refreshes
  TUI slash suggestions, and returns an updated active skill inventory to
  VS Code. The extension exposes an Archived Skills toolbar/command item
  and Restore buttons for archived rows.

### E37. Archived Skill Inspection Polish

- **Status**: landed.
- **Goal**: make archived skill review practical before restoring a
  record, so operators can inspect stale skill bodies instead of
  restoring blindly.
- **Where**: TUI/daemon `/skills show <name>` fallback lookup, VS Code
  command palette/sidebar actions, skill inventory/detail rendering.
- **Result**: `/skills show <name>` now renders archived skill bodies
  with archived metadata when the active inventory has no row. VS Code
  adds Search Archived Skills and archived rows expose both Show and
  Restore actions; archived skill detail cards render Restore instead
  of Use/Archive.

### E38. Session Notes Slash Parity

- **Status**: landed.
- **Goal**: make operator notes durable and inspectable from VS Code the
  same way they already are in TUI sessions.
- **Where**: shared slash parser/catalog, TUI pending session commands,
  daemon `session.command`, reusable session notes helpers, VS Code
  command-result rendering.
- **Result**: daemon-backed `/note <text>` now appends to the active
  session's `notes.ndjson`, matching the TUI persistence path. New
  `/notes [last N]` reads those persisted notes for TUI and daemon
  clients, and VS Code renders note lists as structured cards with copy
  actions.

### E39. Surface-Aware Slash Catalog

- **Status**: landed.
- **Goal**: keep the shared slash catalog authoritative while preventing
  editor users from seeing TUI-only composer suggestions.
- **Where**: TUI slash catalog metadata, daemon `session.command_catalog`,
  VS Code catalog normalization.
- **Result**: command catalog rows now include additive `surfaces`
  metadata. VS Code keeps accepting older catalogs without that field, but
  filters out TUI-only commands such as `/collapse` and `/lang` when the
  metadata is available.

### E40. Structured Slash Argument Options

- **Status**: landed.
- **Goal**: keep finite argument autocomplete consistent across TUI and
  VS Code without requiring each client to parse display-only hint text.
- **Where**: TUI slash catalog helpers, daemon `session.command_catalog`,
  VS Code catalog normalization and composer argument picker.
- **Result**: command catalog rows now include additive `arg_options`
  for finite choices such as `/reasoning <off|low|medium|high|xhigh>`.
  VS Code prefers those structured options and keeps its legacy
  `arg_hint` parser as a compatibility fallback for older daemons.

### E41. Command Catalog Surface Filter

- **Status**: landed.
- **Goal**: let editor clients request an already-filtered slash catalog
  while preserving the full TUI catalog for existing callers.
- **Where**: daemon `session.command_catalog`, VS Code catalog fetch.
- **Result**: `session.command_catalog` now accepts optional params such
  as `{ "surface": "vscode" }` and returns only commands whose
  `surfaces` include that client. Calls without params keep returning the
  full catalog for backwards compatibility. VS Code now requests the
  `vscode` surface explicitly and retains client-side filtering as an
  older-daemon compatibility guard.

### E42. Help Slash Catalog Parity

- **Status**: landed.
- **Goal**: make `/help` use the same daemon-backed slash metadata as the
  composer picker instead of formatting a VS Code-local help message.
- **Where**: daemon `session.command`, VS Code sidebar slash dispatch,
  generic command-result rendering.
- **Result**: `/help` now returns a structured `help` command result with
  one row per surface-filtered slash command. VS Code sends
  `surface: "vscode"` through `session.command`, so the help list and
  composer autocomplete are derived from the same filtered catalog.

### E43. Live Skill Autocomplete Refresh

- **Status**: landed.
- **Goal**: keep VS Code `/skill-name` autocomplete current when the
  stored skill inventory changes after the initial catalog load.
- **Where**: VS Code memory watcher, sidebar slash dispatch, shared daemon
  slash catalog fetch.
- **Result**: VS Code now refreshes the slash catalog when
  `.peridot/memory.db` changes and immediately after `/skills archive` or
  `/skills restore`. Active auto-skills added, archived, or restored during
  the session therefore appear or disappear from composer autocomplete
  without requiring a manual status refresh.

### E44. TUI-Only Sidepanel Surface Metadata

- **Status**: landed.
- **Goal**: keep editor slash suggestions focused on commands that have a
  meaningful VS Code behavior.
- **Where**: shared TUI slash catalog metadata, daemon
  `session.command_catalog`, daemon `/help` surface filtering.
- **Result**: `/sidepanel` is now marked as TUI-only, matching its status
  panel toggle semantics. TUI keeps accepting the command, while VS Code's
  surface-filtered autocomplete and `/help` output no longer suggest it.

### E45. Session New Slash Intent Parity

- **Status**: landed.
- **Goal**: stop requiring VS Code to locally re-parse `/session new`
  after the daemon has already parsed the shared slash command.
- **Where**: daemon `session.command`, VS Code sidebar slash result
  application.
- **Result**: `/session new [task]` now returns a structured
  `session_new` command result with an optional task. VS Code creates the
  new local session from that daemon result and starts the task when one is
  present, while retaining the old local fallback for older daemons.

### E46. Session List Sidebar Reconcile

- **Status**: landed.
- **Goal**: make `/session list` update the VS Code session cards from the
  daemon's authoritative persisted/live session inventory instead of only
  rendering rows in the transcript.
- **Where**: VS Code command-result typing and sidebar slash result
  application.
- **Result**: VS Code now accepts the `sessions` array returned by
  daemon-backed `/session list` and reconciles the sidebar's local session
  cards from it. The command still renders the structured transcript rows,
  while the session switcher/menu immediately reflects daemon state.

### E47. Session Save Sidebar Reconcile

- **Status**: landed.
- **Goal**: make `/session save` update the VS Code session card metadata
  from the daemon's structured save result instead of only rendering a
  transcript confirmation.
- **Where**: VS Code sidebar slash result application.
- **Result**: VS Code now maps daemon-backed `session_save` results into
  the same sidebar reconciliation path used by `/session list`, preserving
  transcript compatibility while refreshing the saved session title,
  status, token, cost, and turn metadata when those fields are present.

### E48. Bare Goal Slash Parity

- **Status**: landed.
- **Goal**: make `/goal` with no objective use the same shared slash
  parser and daemon state-delta path as `/plan` and `/execute` instead of
  being a VS Code-local composer toggle.
- **Where**: shared slash parser/state delta, TUI slash application,
  daemon `session.command`, VS Code slash dispatch.
- **Result**: `/goal` now parses as a goal-mode switch, returns a
  structured `setting` result with `state_delta.mode = "goal"` from the
  daemon, and updates TUI state without creating a goal objective. VS Code
  routes the command through the daemon instead of special-casing it
  locally.

### E49. Session Switch Metadata Reconcile

- **Status**: landed.
- **Goal**: make `/session switch <id|title>` carry the same persisted
  session metadata that `/session list` and `/session save` already expose
  so editor clients do not need a follow-up list call after switching.
- **Where**: daemon `session.command`, VS Code sidebar slash result
  application.
- **Result**: daemon-backed `session_switch` results now include summary,
  updated timestamp, total tokens, total cost, and turn count when known.
  VS Code feeds that switch result through the same sidebar reconciliation
  path as list/save before selecting the target session card.

### E50. Session Rename Metadata Reconcile

- **Status**: landed.
- **Goal**: make `/session rename <id|title> <new title>` return the
  updated persisted session metadata so editor clients can refresh the
  existing sidebar card without a follow-up list call.
- **Where**: daemon `session.command`, VS Code sidebar slash result
  application.
- **Result**: daemon-backed `session_rename` results now include summary,
  status, running state, updated timestamp, total tokens, total cost, and
  turn count when the rename succeeds. VS Code reconciles that metadata
  before applying the local title update, preserving older-daemon fallback
  behavior when those additive fields are absent.

### E51. Session New Daemon Materialization

- **Status**: landed.
- **Goal**: make `/session new [task]` create an authoritative daemon
  session id and persisted idle record before editor clients select or
  start the session.
- **Where**: daemon `session.command`, VS Code sidebar slash result
  application.
- **Result**: daemon-backed `session_new` results now include session id,
  title, summary, status, running state, updated timestamp, and zeroed
  usage fields. VS Code reconciles and selects that daemon-backed session
  card before starting an optional task, so the subsequent `session.start`
  continues the same id instead of creating a separate daemon session.

### E52. Session List Prune Reconcile

- **Status**: landed.
- **Goal**: keep VS Code sidebar sessions aligned when daemon-backed
  sessions are deleted or cleared by another client or by a daemon
  `session.list_changed` notification.
- **Where**: VS Code sidebar session reconciliation, daemon session list
  subscription handling.
- **Result**: full daemon session inventories from `/session list`,
  `session.list`, `session.subscribe_list`, and `session.list_changed`
  now prune missing daemon-backed sidebar cards. Partial single-session
  results such as save, switch, rename, and new remain additive so local
  draft sessions are not removed by accident. The pruning rule is covered
  by extension unit tests that run without the VS Code host.

### E53. Local Slash Fallback Cleanup

- **Status**: landed.
- **Goal**: keep the VS Code slash path daemon-owned after the parity
  work, leaving only real editor-local actions in the local fallback.
- **Where**: VS Code sidebar slash dispatch and extension unit tests.
- **Result**: daemon `action: "local"` responses now route only
  `/sidepanel` and `/status` to the sidebar status summary. Daemon-backed
  commands such as `/info`, `/cost`, `/plan show`, and `/session list` no
  longer have stale VS Code re-parser fallbacks, and the local-action
  filter is covered by a pure unit test.

### E54. Status Slash Discoverability

- **Status**: landed.
- **Goal**: remove drift between the parser and discoverability surfaces
  for the `/status` alias.
- **Where**: shared slash catalog, daemon command catalog/help filtering,
  TUI and VS Code autocomplete tests.
- **Result**: `/status` is now advertised by the shared command catalog
  and therefore appears in TUI slash autocomplete, VS Code composer
  autocomplete, and daemon-backed `/help`. `/sidepanel` remains TUI-only,
  while `/status` stays available as a cross-surface local status action.

### E55. TUI Live Skill Autocomplete Refresh

- **Status**: landed.
- **Goal**: bring TUI `/skill-name` autocomplete freshness up to the VS
  Code watcher behavior, so both clients reflect skill inventory changes
  made by another surface during a long-running session.
- **Where**: TUI host persist tick, project memory-store signature helper.
- **Result**: the TUI records the `.peridot/memory.db` / SQLite sidecar
  signature it loaded at startup, checks it on the existing once-per-second
  persistence tick, and reloads active auto-skill suggestions when the
  signature changes. VS Code keeps its file watcher path; both clients now
  refresh autocomplete without waiting for an agent run to finish.

### E56. VS Code Per-Session Composer History

- **Status**: landed.
- **Goal**: bring the TUI's per-session input-history ergonomics to the
  extension composer while preserving VS Code's multiline textarea
  behavior.
- **Where**: VS Code webview composer state and pure history helper tests.
- **Result**: submitted prompts are recorded per sidebar session and can
  be recalled with ArrowUp / ArrowDown when the caret is on the first or
  last textarea line. In-progress drafts are stored per session as well,
  so switching between sidebar sessions no longer carries one session's
  unsent prompt into another. Both history and drafts are serialized into
  VS Code webview state, so they survive webview reloads without involving
  daemon or workspace storage.

### E57. Bounded Shared Input History Semantics

- **Status**: landed.
- **Goal**: keep TUI and VS Code prompt history behavior aligned now that
  both clients expose per-session recall.
- **Where**: TUI `TuiState::record_input_history`, VS Code composer
  history helper.
- **Result**: TUI input history now deduplicates repeated prompts by
  moving them to the newest slot and caps each session at the 50 most
  recent entries, matching the VS Code composer history policy while
  preserving per-session state swaps.

### E58. Skill-Name Argument Autocomplete

- **Status**: landed.
- **Goal**: make skill-management slashes feel as skill-aware as direct
  `/skill-name` invocation in both TUI and VS Code.
- **Where**: shared TUI slash picker argument context and VS Code
  webview slash autocomplete helper.
- **Result**: `/skills show`, `/skills view`, `/skills use`,
  `/skills pin`, `/skills unpin`, and `/skills archive` now offer active
  auto-skill names as argument completions, while `/skills restore`
  offers archived auto-skill names. The suggestions reuse the same
  live-refreshed skill inventory as `/skill-name` autocomplete, trim a
  typed leading slash for matching, keep archived skills out of direct
  `/skill-name` invocation suggestions, and close once the selected skill
  name is exact so Enter submits the command normally.

### E59. Session-Target Argument Autocomplete

- **Status**: landed.
- **Goal**: make session lifecycle slash commands easier to use from both
  TUI and VS Code without copying opaque session ids from a separate list.
- **Where**: TUI slash picker dynamic argument context and VS Code
  webview slash autocomplete helper.
- **Result**: `/session switch`, `/session close`, `/session delete`, and
  `/session rename` now offer session-id completions from the visible
  session list. Typed title prefixes also match, but the accepted value is
  the stable session id. Rename completions leave a trailing space so the
  operator can immediately type the new title.

### E60. Provider Argument Autocomplete

- **Status**: landed.
- **Goal**: make provider switching discoverable from both TUI and VS Code
  without requiring operators to memorize the exact `auth.primary` ids.
- **Where**: shared slash catalog `arg_options`, daemon
  `session.command_catalog`, TUI finite-argument picker, and VS Code
  composer autocomplete.
- **Result**: `/provider` now advertises the supported live provider ids
  (`claude-api`, `openai-api`, `openrouter-api`, `openai-oauth`) as
  structured argument options. TUI and VS Code filter those ids as the
  operator types and close the argument picker once an exact provider id is
  present so Enter submits the command normally.

### E61. Code-Map Subcommand Autocomplete

- **Status**: landed.
- **Goal**: make the code-map workflow discoverable from both TUI and VS
  Code without forcing operators to remember the exact `/codemap`
  subcommands.
- **Where**: shared slash catalog `arg_options`, daemon
  `session.command_catalog`, TUI finite-argument picker, and VS Code
  composer autocomplete.
- **Result**: `/codemap` now advertises `status`, `refresh`, `find`,
  `locate`, `outline`, and `refs` as structured argument options while
  keeping the detailed help hint for subcommands that need a follow-up
  query/path/symbol. TUI and VS Code filter those subcommands as the
  operator types and close the picker once an exact subcommand is present.

### E62. MCP Add Transport Autocomplete

- **Status**: landed.
- **Goal**: make MCP server registration less error-prone from both TUI
  and VS Code by completing the finite transport argument even though the
  server name before it is free-form.
- **Where**: TUI dynamic slash argument context, TUI Tab acceptance path,
  and VS Code webview slash autocomplete helper.
- **Result**: after `/mcp add <name> `, both clients now suggest `stdio`
  and `http`, filter the suggestions as the operator types, and leave a
  trailing space after accepting the transport so the command or URL can
  be entered immediately. The picker stays closed once a complete transport
  is present or a command/URL argument has started.

### E63. MCP Server-Name Autocomplete

- **Status**: landed.
- **Goal**: make MCP maintenance commands safer from both TUI and VS Code
  by completing configured server names for destructive or probing actions.
- **Where**: daemon `peridot.status` MCP summary, extension status
  context, TUI side-panel MCP summaries, TUI dynamic slash argument
  context, and VS Code webview slash autocomplete helper.
- **Result**: `/mcp remove <name>` and `/mcp test <name>` now suggest
  configured MCP server names. TUI reads the existing side-panel MCP
  status list, while VS Code receives the configured MCP names through
  status refresh and passes them into the composer picker. Exact names and
  commands with extra arguments close the picker so Enter submits normally.

### E64. Model-Name Autocomplete

- **Status**: landed.
- **Goal**: make runtime model switching less typo-prone from both TUI and
  VS Code without hard-coding provider catalog assumptions.
- **Where**: daemon `peridot.status` model suggestion summary, TUI startup
  model suggestion state, TUI dynamic slash argument context, extension
  status context, and VS Code webview slash autocomplete helper.
- **Result**: `/model <name>` and `/subagent model <name|reset>` now suggest
  configured main, subagent, and committee role model names. TUI seeds the
  picker from project config and keeps manually selected runtime models in
  the list, while VS Code receives the model suggestions during status
  refresh and appends newly selected runtime models from slash state deltas.
  `/subagent model` also keeps `reset` as a first-class completion.

### E65. Branch Restore Snapshot Autocomplete

- **Status**: landed.
- **Goal**: make branch snapshot restore less error-prone from both TUI
  and VS Code by completing saved snapshot names instead of requiring
  operators to copy them from `/branch list`.
- **Where**: shared branch snapshot discovery, TUI startup/saved-branch
  suggestion state, daemon `peridot.status`, extension status context,
  TUI dynamic slash argument context, and VS Code webview slash
  autocomplete helper.
- **Result**: `/branch restore <name>` now suggests saved
  `.peridot/branches/<name>` snapshot directories. TUI seeds the picker at
  startup and adds newly saved branches immediately after `/branch save`,
  while VS Code receives snapshot names during status refresh. Exact names
  and commands with extra arguments close the picker so Enter submits
  normally.

### E66. Goal And Notes Subcommand Autocomplete

- **Status**: landed.
- **Goal**: make mixed free-form slash commands easier to use from both
  TUI and VS Code without turning every first argument into a rigid
  choice list.
- **Where**: TUI dynamic slash argument context, TUI Tab acceptance path,
  and VS Code webview slash autocomplete helper.
- **Result**: `/goal pause|resume|clear|status` now autocomplete after
  `/goal ` or a matching prefix, while bare `/goal` and nonmatching
  free-form objectives still behave as before. `/notes last` now
  autocompletes with a trailing space so the operator can immediately type
  the count.

### E67. Export Artifact Multi-Argument Autocomplete

- **Status**: landed.
- **Goal**: make `/export` ergonomic when operators want a custom subset
  of session artifact classes instead of the default bundle.
- **Where**: TUI dynamic slash argument context, TUI Tab acceptance path,
  and VS Code webview slash autocomplete helper.
- **Result**: `/export <artifact...>` now autocompletes
  `attachments`, `notes`, `timeline`, and `full` one token at a time.
  Accepting an artifact leaves a trailing space for the next artifact, and
  already-selected artifacts are hidden from the remaining suggestions.

### E68. Think Alias Argument Autocomplete

- **Status**: landed.
- **Goal**: keep reasoning shortcut autocomplete aligned with the parser
  so operators can discover `/think` aliases without memorizing them.
- **Where**: TUI dynamic slash argument context, TUI Tab acceptance path,
  and VS Code webview slash autocomplete helper.
- **Result**: `/think <arg>` now suggests parser-supported aliases and
  canonical tiers in both clients: `hard`, `harder`, `more`, `high`,
  `xhigh`, `medium`, `low`, `off`, `stop`, and `less`. Bare `/think`
  remains runnable as the high-reasoning shortcut.

### E69. Fast And Autofix Alias Autocomplete

- **Status**: landed.
- **Goal**: keep service-tier and autofix shortcut autocomplete aligned
  with parser-supported aliases without making free-form numeric autofix
  limits rigid.
- **Where**: TUI dynamic slash argument context, TUI Tab acceptance path,
  and VS Code webview slash autocomplete helper.
- **Result**: `/fast <arg>` now suggests `on`, `off`, `toggle`, `true`,
  `false`, `1`, `0`, and `standard`; `/autofix <arg>` now suggests
  `on`, `off`, `true`, `false`, `1`, and `0`. Bare commands remain
  runnable, and `/autofix <N>` still submits as a free-form max-attempts
  value.

### E70. Skills Search Continuation Autocomplete

- **Status**: landed.
- **Goal**: make `/skills search` autocomplete stop at a valid editing
  position instead of accepting an invalid bare command.
- **Where**: TUI dynamic slash argument context, TUI Tab acceptance path,
  and VS Code webview slash autocomplete helper.
- **Result**: accepting `/skills se` now fills `/skills search ` with a
  trailing space in both clients, leaving the free-form query slot ready
  for the operator. Once the trailing space is present, autocomplete
  closes so the search query remains unrestricted.

### E71. Branch Subcommand Continuation Autocomplete

- **Status**: landed.
- **Goal**: keep branch DAG and snapshot workflows discoverable without
  leaving operators at placeholder or invalid command text after accepting
  autocomplete.
- **Where**: shared slash catalog, TUI dynamic slash argument context,
  TUI Tab acceptance path, and VS Code webview slash autocomplete helper.
- **Result**: `/branch turn <turn-id>` is now advertised in the shared
  catalog, matching the parser-supported command. Accepting
  `/branch save|restore|turn|switch` completions in TUI or VS Code now
  leaves a trailing argument slot (`/branch turn `, `/branch restore `,
  etc.) so users can immediately type the required name, turn id, or DAG
  limb index. `/branch list` and `/branch tree` remain plain runnable
  commands.

### E72. Skills Management Continuation Autocomplete

- **Status**: landed.
- **Goal**: keep skill inventory management autocomplete from stopping at
  invalid bare subcommands when the selected action still requires a
  skill name.
- **Where**: TUI dynamic slash argument context, TUI Tab acceptance path,
  and VS Code webview slash autocomplete helper.
- **Result**: accepting `/skills show|view|use|pin|unpin|archive|restore`
  subcommand completions now leaves a trailing skill-name slot in both
  TUI and VS Code (`/skills show `, `/skills restore `, etc.). Existing
  dynamic skill-name completion still takes over once the command is exact
  and live skill suggestions are available, while `/skills list` remains a
  runnable inventory command and `/skills search ` keeps its free-form
  query behavior.

### E73. Code-Map Continuation Autocomplete

- **Status**: landed.
- **Goal**: prevent code-map autocomplete from stopping at invalid bare
  subcommands when the selected action still requires a query, path, or
  symbol argument.
- **Where**: TUI dynamic slash argument context, TUI Tab acceptance path,
  and VS Code webview slash autocomplete helper.
- **Result**: accepting `/codemap find|locate|outline|refs` subcommand
  completions now leaves a trailing argument slot in both TUI and VS Code
  (`/codemap locate `, `/codemap outline `, etc.). `/codemap status` and
  `/codemap refresh` stay directly runnable, and ambiguous prefixes such
  as `/codemap r` still show both `refresh` and `refs` without forcing the
  trailing-space behavior too early.

### E74. Context Alias Discoverability

- **Status**: landed.
- **Goal**: keep parser-supported context inspection aliases visible in
  the shared command catalog used by TUI and VS Code.
- **Where**: shared TUI slash catalog, daemon `session.command_catalog`,
  TUI help/autocomplete, and VS Code composer autocomplete/help.
- **Result**: `/context` is now advertised alongside `/context top`,
  matching the parser path that already treats both commands as the same
  `ContextTop` action. Both forms remain available through the same
  daemon-backed slash command handling.

### E75. Session Subcommand Continuation Autocomplete

- **Status**: landed.
- **Goal**: prevent session autocomplete from accepting placeholder text
  such as `<id|title>` when the operator starts from a partial
  subcommand.
- **Where**: TUI dynamic slash argument context, TUI Tab acceptance path,
  and VS Code webview slash autocomplete helper.
- **Result**: accepting `/session new|switch|close|delete|rename`
  completions now leaves a trailing argument slot in both clients
  (`/session switch `, `/session rename `, etc.). Existing dynamic
  session-target autocomplete still takes over after exact
  `/session switch|close|delete|rename` commands when session ids are
  available, while `/session save|list|count` remain directly runnable.

### E76. Free-Form Slash Acceptance Cleanup

- **Status**: landed.
- **Goal**: stop autocomplete acceptance from copying human-readable
  placeholder hints into the actual composer input.
- **Where**: TUI slash picker acceptance path and VS Code webview slash
  autocomplete acceptance helper.
- **Result**: accepting a command with a free-form argument hint now
  leaves an editable trailing space instead of inserting placeholders
  such as `<task>`, `<path>`, or `<objective>`. The picker still displays
  the hint as documentation, finite option commands still open their
  option picker, and no-argument commands still accept to the command name
  exactly.

### E77. Committee Mode Status Parity

- **Status**: landed.
- **Goal**: make committee mode visible in VS Code after the operator
  toggles it, matching the TUI status bar's `committee <mode>` signal.
- **Where**: daemon `peridot.status`, daemon `session.command` state
  delta, VS Code sidebar context state, VS Code webview status strip, and
  shared slash autocomplete tests.
- **Result**: daemon status now includes `committee_mode`, `/committee`
  command results use the lower-case display form (`committee: full`),
  and VS Code applies `state_delta.committee_mode` into the sidebar
  context. The webview renders `committee planner|full` as a mode pill
  and keeps `off` hidden to avoid noise, while TUI and VS Code tests
  cover `/committee off|planner|full` autocomplete choices.

### E78. Committee Event Transcript Parity

- **Status**: landed.
- **Goal**: make VS Code show the same committee planner/reviewer progress
  events that already appear in the TUI transcript.
- **Where**: shared daemon `AgentRunEvent` stream, TUI runtime event
  rendering, VS Code sidebar transcript conversion, and VS Code unit
  tests.
- **Result**: `planner_plan_ready` now renders as a committee planner
  transcript row in VS Code, and `reviewer_verdict` renders turn-scoped
  approve/request/block rows. Reviewer blocks use an error transcript role
  so duplicate-diff and hard-stop review guards are visible instead of
  collapsing to an opaque event-kind label.

### E79. Auto-Fix Attempt Transcript Parity

- **Status**: landed.
- **Goal**: make VS Code render auto-fix verification progress the same
  way the TUI transcript already does.
- **Where**: shared daemon `AgentRunEvent::AutoFixAttempt`, TUI runtime
  event rendering, VS Code sidebar transcript conversion, and VS Code unit
  tests.
- **Result**: `auto_fix_attempt` now renders as
  `autofix: <tool> passed|FAILED (attempt n/max)` in the VS Code
  transcript instead of falling back to the opaque event-kind label.
  Existing TUI rendering stays unchanged.

### E80. Live MCP Status Event Parity

- **Status**: landed.
- **Goal**: make VS Code consume live MCP status events the same way the
  TUI side panel does, instead of showing an opaque event-kind transcript
  row.
- **Where**: shared daemon `AgentRunEvent::McpStatusChanged`, TUI runtime
  side-panel state, VS Code sidebar context state, and VS Code unit tests.
- **Result**: `mcp_status_changed` now refreshes `context.mcpServers` in
  the VS Code sidebar and is suppressed from the transcript. The webview's
  `/mcp remove|test` argument autocomplete therefore sees live server
  names from the daemon event stream without waiting for a later status
  refresh.

### E81. AGENTS.md Hot-Reload Status Parity

- **Status**: landed.
- **Goal**: make VS Code expose AGENTS.md hot-reload events the same way
  the TUI side panel exposes the active instruction summary.
- **Where**: shared daemon `AgentRunEvent::AgentsMdLoaded`, TUI side-panel
  state, VS Code sidebar context state, VS Code context strip, and VS Code
  unit tests.
- **Result**: `agents_md_loaded` now updates a sidebar `agents` summary
  and stays suppressed from the transcript. The webview context strip
  shows an `AGENTS <rule-count>` pill with source paths in the hover title,
  so an operator can see mid-run instruction reloads without reading an
  opaque event row.

### E82. Session Save Event Transcript Parity

- **Status**: landed.
- **Goal**: make VS Code render daemon session persistence events with the
  same operator-facing meaning as the TUI transcript.
- **Where**: shared daemon `AgentRunEvent::SessionSaved` /
  `SessionSaveFailed`, TUI runtime transcript handling, VS Code transcript
  conversion, and VS Code unit tests.
- **Result**: `session_saved` now renders as a resume-ready session line,
  and `session_save_failed` renders as an error row with the failure
  message. Both events avoid the opaque event-kind fallback in VS Code.

### E83. Hook Event Transcript Parity

- **Status**: landed.
- **Goal**: make VS Code render hook activity with the same meaning the
  TUI activity panel exposes instead of falling back to raw event names.
- **Where**: shared daemon `AgentRunEvent::HookFired`, TUI runtime
  activity handling, VS Code transcript conversion, and VS Code unit
  tests.
- **Result**: `hook_fired` now renders as
  `hook:<name> - <category>: <outcome>` in VS Code. Blocking, failing, or
  error-like outcomes use an error transcript row; normal hook outcomes
  use a status row.

### E84. Run Start Status Parity

- **Status**: landed.
- **Goal**: make VS Code react to daemon run-start events the same way the
  TUI marks the active run as running.
- **Where**: shared daemon `AgentRunEvent::RunStarted`, TUI
  `mark_agent_running`, VS Code sidebar runtime state, and extension
  verification.
- **Result**: `run_started` now transitions the VS Code sidebar status and
  context strip from `Starting daemon` to `Running` immediately while
  staying out of the transcript, so users see the live run state before
  the first model/tool event arrives.

### E85. Interrupted Event Lifecycle Parity

- **Status**: landed.
- **Goal**: make VS Code treat external interruption as a terminal run
  lifecycle state, matching the TUI's interrupted status instead of
  leaving the sidebar running.
- **Where**: shared daemon `AgentRunEvent::Interrupted`, TUI
  `AgentRunStatus::Interrupted`, VS Code sidebar lifecycle handling, VS
  Code extension active-run cleanup, and VS Code unit tests.
- **Result**: `interrupted` now stops the active VS Code run, clears the
  running flag, sets the status/context strip to `Interrupted`, refreshes
  daemon status, and drains any queued task using the same terminal-event
  path as `finished` / `error`.

### E86. Cursor File Link And Session Polish

- **Status**: landed.
- **Goal**: address Cursor-observed sidebar friction around relative file
  links, completed run timing, and session rename editing.
- **Where**: VS Code file-open path resolution, webview run footer,
  session menu rename input handling, and read-only shell allowlist.
- **Result**: file links now try normalized project/workspace candidates,
  workspace-name-prefixed variants, and basename search fallback; finished
  durations are appended to the transcript instead of staying pinned above
  the composer; rename inputs select only once when editing starts; and
  `nl -ba` is accepted as a read-only inspection command.

### E87. VS Code Live Usage Budget Dock

- **Status**: landed.
- **Goal**: surface the same live cost/token/budget pressure that the TUI
  status metrics already expose, without requiring `/cost` after a run.
- **Where**: VS Code daemon event HUD state, webview composer controls,
  and webview unit tests.
- **Result**: `usage_updated`, `budget_updated`, and committee role usage
  now render as compact composer metric chips for executor tokens,
  aggregate executor+committee cost, budget percentage, and turn budget.
  Budget and turn chips switch to warning/critical tones near their
  configured limits.

### E88. Cursor Path And Run-Finish Polish

- **Status**: landed.
- **Goal**: close the Cursor-observed gaps left after the first path and
  session polish pass: abbreviated file links, completion timing placement,
  slow session-title editing, and `nl -ba` read-only confidence.
- **Where**: VS Code workspace file resolution, webview transcript/status
  rendering, session menu rename draft handling, and read-only shell tests.
- **Result**: abbreviated file links containing `...` expand to workspace
  globs, then fall back to a narrowed fuzzy match that can resolve
  reordered camel-case Java hints such as `ApiKeyMongo.java` to
  `MongoApiKeyRepository.java` under the same project prefix. Finished /
  failed / interrupted durations now render as transcript completion
  bubbles instead of composer-adjacent status rows. Session rename drafts
  preserve empty in-progress input across refreshes, and `nl -ba` remains
  covered for long nested Java paths.

### E89. Approval Risk-Class Parity

- **Status**: landed.
- **Goal**: keep the tool risk signal visible at the moment an operator
  approves or denies a gated action, not only on the earlier tool card.
- **Where**: shared `AgentRunEvent::ApprovalRequested`, daemon
  `approval_waiting` snapshots, TUI runtime approval panel, VS Code
  sidebar approval state, and VS Code webview risk-chip rendering.
- **Result**: approval-required events now carry an additive optional
  `risk_class` field. Daemon resume snapshots preserve it, the TUI
  approval panel renders it as `Risk: <class>`, and VS Code approval
  prompts show the same compact chip used by tool summaries. Older events
  without the field still deserialize, and unknown future labels render
  with a sanitized fallback chip class.

### E90. Composer Metric Dock Cleanup

- **Status**: landed.
- **Goal**: keep the VS Code composer metric surface aligned with the
  implemented live usage/budget dock and avoid stale sidebar assumptions in
  the webview code.
- **Where**: VS Code webview composer dock rendering and roadmap/changelog
  documentation.
- **Result**: live usage/budget metric chips are still rendered in the
  composer when data exists, but the webview no longer creates an empty
  run-metrics dock for sessions with no HUD values. The stale code comment
  saying token/cost HUD was omitted has been removed.

### E91. Settings Number Draft Normalization

- **Status**: landed.
- **Goal**: make the VS Code settings webview save exactly the numeric
  value the operator sees, especially while correcting or clearing number
  fields.
- **Where**: VS Code settings webview numeric control model, settings
  webview unit tests, and changelog documentation.
- **Result**: empty or invalid numeric drafts no longer mutate the
  settings save payload before blur restores the visible value. Out-of-range
  numbers still clamp to the configured bounds, and integer settings
  (`U32` / `Usize`) normalize decimal input to integer JSON values before
  `settings.save`.

### E92. Additive Agent Event Fallback Cleanup

- **Status**: landed.
- **Goal**: keep the VS Code transcript aligned with the daemon event
  schema contract that additive future `AgentRunEvent` variants are a
  no-op for older clients.
- **Where**: VS Code sidebar event-to-transcript fallback and pure event
  transcript tests.
- **Result**: known daemon events keep their existing structured handling,
  but unknown non-empty event kinds no longer render opaque status rows in
  the chat transcript. Malformed events with no kind still fall back to the
  generic `Event` row.

### E93. VS Code Ask-User Waiting State

- **Status**: landed.
- **Goal**: make daemon `agent_ask_user` pauses as visible and accurate in
  VS Code as approval pauses, without marking a rejected or stale response
  as sent.
- **Where**: VS Code daemon event lifecycle helpers, sidebar ask-user
  response handling, and extension host interaction responses.
- **Result**: `ask_user_requested` now moves the sidebar/context status to
  `Waiting for user response` while keeping the run active. Sending an
  answer restores the local status to `Running` only after the daemon
  accepts the `interaction.respond` call; stale or rejected responses keep
  the prompt visible and surface the existing error message.

### E94. Recovery Debug-Only Transcript Policy

- **Status**: landed.
- **Goal**: keep internal recovery directives out of user chat transcripts
  while preserving enough diagnostic signal for debugging.
- **Where**: TUI runtime recovery handling, VS Code sidebar event handling,
  read-only shell policy errors, and security/playbook docs.
- **Result**: `recovery` daemon events no longer render as TUI or VS Code
  transcript rows. VS Code keeps the raw daemon event in the Output channel,
  TUI keeps recovery context in runtime activity, and `shell_readonly`
  allowlist denials now tell the model to retry with an allowlisted
  read-only command or use the normal `shell_exec` approval path when shell
  semantics are required.

### E95. Recovery Output Formatting

- **Status**: landed.
- **Goal**: make the remaining VS Code debug surface readable after recovery
  events became transcript-suppressed.
- **Where**: VS Code daemon event Output formatting and pure unit tests.
- **Result**: daemon event Output formatting now routes through a testable
  helper. `recovery` events render as `[session] recovery: <message>` in the
  VS Code Output channel, while unknown additive events remain logged with
  JSON payloads for debugging without leaking into chat.

### E96. Session Transcript Search Slash Parity

- **Status**: landed.
- **Goal**: let editor users search persisted session transcripts without
  leaving the chat surface, matching the existing CLI utility.
- **Where**: shared slash parser, TUI session command queue, daemon
  `session.command`, and the persisted transcript search helper.
- **Result**: `/session search <query>` now searches persisted sessions in
  both TUI and VS Code. The daemon returns a structured `session_search`
  result with hit rows, while TUI prints the same capped result set into the
  transcript.

### E97. Session Show Slash Parity

- **Status**: landed.
- **Goal**: let editor and TUI users inspect one persisted session after
  finding it through `/session list`, `/session search`, or autocomplete.
- **Where**: shared slash parser/catalog, TUI session command queue, daemon
  `session.command`, and a reusable persisted session summary helper.
- **Result**: `/session show <id|title>` now returns lifecycle, workspace,
  token/cost/turn usage, worktree branch, last task, and notes summary data
  in both TUI and VS Code. VS Code receives a structured `session_show`
  command result, so the generic command result block can render the details
  without duplicating CLI formatting.

### E98. Session Locate Slash Parity

- **Status**: landed.
- **Goal**: let editor and TUI users jump from a known session id/title to
  the persisted session directory without switching to a terminal.
- **Where**: shared slash parser/catalog, TUI session command queue, daemon
  `session.command`, and the existing CLI locate path helper.
- **Result**: `/session locate <id|title>` now resolves visible or persisted
  session targets and returns the `.peridot/sessions/<id>` directory in both
  clients. VS Code receives a structured `session_locate` command result with
  a path row, so the existing command renderer can expose the directory path.

### E99. Session Resume Slash Parity

- **Status**: landed.
- **Goal**: let editor and TUI users continue from a persisted session
  summary without copying the CLI `peridot session resume` output manually.
- **Where**: shared slash parser/catalog, TUI session command queue, daemon
  `session.command`, and a reusable resume-task helper shared with the CLI.
- **Result**: `/session resume <id|title>` resolves visible or persisted
  session targets and builds the same continuation prompt as
  `peridot session resume <id>`. TUI starts the task in the current
  foreground session; VS Code receives a `start_task` command result and
  dispatches it through the existing automatic task-start path.

### E100. Notes Clear Slash Parity

- **Status**: landed.
- **Goal**: finish the interactive notes lifecycle by letting operators
  clear active-session notes without using the CLI subcommand directly.
- **Where**: shared slash parser/catalog, TUI session command queue, daemon
  `session.command`, and the CLI note-clear helper.
- **Result**: `/notes clear` removes the active session's `notes.ndjson`
  through the same helper as `peridot session note <id> clear`. TUI reports
  whether notes were cleared, and VS Code receives a structured
  `notes_clear` command result rendered in the existing notes block.

### E101. Session List Status Filter Slash Parity

- **Status**: landed.
- **Goal**: make the existing CLI lifecycle filter available from the
  interactive composers so operators can narrow session lists without
  switching surfaces.
- **Where**: shared slash parser/catalog, TUI session command queue, daemon
  `session.command`, and VS Code slash autocomplete.
- **Result**: `/session list --status idle|running|suspended|done|failed`
  filters persisted sessions in both TUI and VS Code. The daemon returns the
  filtered `sessions`, `items`, `total`, and `status_filter` fields, and both
  composers complete the finite lifecycle values.

### E102. Session Prune Slash Parity

- **Status**: landed.
- **Goal**: let operators clean up stale persisted sessions from the same
  TUI and editor surfaces that list, count, and inspect them.
- **Where**: shared slash parser/catalog, reusable session prune helper, TUI
  session command queue, daemon `session.command`, and VS Code slash
  autocomplete.
- **Result**: `/session prune [--status <state>] [--older-than-days N]
  [--dry-run]` now uses the same deletion helper as `peridot session prune`.
  TUI prints a dry-run or removal summary, VS Code receives a structured
  `session_prune` result with `considered`, `removed`, `status_filter`,
  `older_than_days`, and `dry_run`, and both composers complete prune flags
  plus lifecycle status values.

### E103. Session Replay Slash Parity

- **Status**: landed.
- **Goal**: expose the committee-weaved persisted replay timeline from the
  interactive clients, so operators can inspect a past session without
  switching to `peridot session replay`.
- **Where**: shared slash parser/catalog, reusable session replay summary
  helper, TUI session command queue, daemon `session.command`, and VS Code
  slash autocomplete.
- **Result**: `/session replay <id|title> [--last N]` resolves visible or
  persisted session targets, loads the unified transcript + committee
  timeline, and returns the same transcript-compatible `entries` plus
  structured `timeline` rows. TUI prints replay rows into the transcript,
  VS Code renders them through the generic command result list, and both
  composers complete replay targets plus the optional `--last` flag.

### E104. Persisted Session Export Slash Parity

- **Status**: landed.
- **Goal**: let operators export artifacts from any persisted session from
  the same interactive clients that can list, inspect, replay, and prune
  sessions.
- **Where**: shared slash parser/catalog, existing session export artifact
  helper, TUI session command queue, daemon `session.command`, and VS Code
  slash autocomplete.
- **Result**: `/session export <id|title> [attachments|notes|timeline|full]`
  resolves visible or persisted session targets and writes the selected
  portable artifacts to `.peridot/exports/<session>-<timestamp>/`. Bare
  artifact selection keeps the CLI-compatible full-copy default, TUI prints
  the export summary, VS Code receives the existing `session_export`
  structured result, and both composers complete target sessions plus
  remaining artifact classes.

### E105. Persisted Session Import Slash Parity

- **Status**: landed.
- **Goal**: let operators restore portable persisted session directories
  from the same TUI and editor surfaces that can export them.
- **Where**: shared slash parser/catalog, reusable session import helper,
  TUI session command queue, daemon `session.command`, and VS Code slash
  autocomplete.
- **Result**: `/session import <dir> [--id <id>] [--force]` copies a
  portable session directory into `.peridot/sessions`, updates persisted
  session summaries when transcript data is available, reports the copied
  files in TUI, returns a structured `session_import` result in VS Code,
  and completes the optional `--id` / `--force` flags in both composers.

### E106. VS Code Session Import Affordance

- **Status**: landed.
- **Goal**: make restored session artifacts discoverable from the GUI, not
  only from the slash composer.
- **Where**: VS Code command contributions, sidebar title-bar actions,
  session import command construction, daemon import result metadata, and
  command-result rendering.
- **Result**: `Peridot: Import Session Artifacts` and the sidebar import
  button open a folder picker, prompt for an optional imported session id,
  ask whether to overwrite an existing id, run the shared
  `/session import <dir>` daemon path, refresh the session list, and render
  a session import card with source, destination, copied files, and
  destination open/copy actions.

### E107. VS Code Session Export Target Picker

- **Status**: landed.
- **Goal**: make the GUI export affordance match `/session export` parity
  by supporting persisted sessions, not only the currently active sidebar
  session.
- **Where**: VS Code export command construction, session-list fetch,
  command palette/sidebar export flow, README, and changelog docs.
- **Result**: `Peridot: Export Session Artifacts` now fetches the daemon
  session list, combines it with the current live session, prompts for a
  target when more than one session is available, writes the selected
  session's portable artifacts to the chosen destination, and records the
  selected session id in the result card.

### E108. VS Code Session Replay GUI

- **Status**: landed.
- **Goal**: make persisted timeline replay available without typing the
  slash command manually.
- **Where**: VS Code command contributions, sidebar title-bar actions,
  session replay command construction, README, and changelog docs.
- **Result**: `Peridot: Replay Session Timeline` and the sidebar replay
  button fetch persisted daemon sessions, prompt for the replay target,
  optionally limit the output to recent timeline entries, run the shared
  `/session replay <id> [--last N]` daemon path, and render the replay
  result in the existing command-result transcript block.

### E109. VS Code Session Prune GUI

- **Status**: landed.
- **Goal**: make persisted session cleanup discoverable from the editor
  while keeping deletion guarded.
- **Where**: VS Code command contributions, sidebar title-bar actions,
  session prune command construction, README, and changelog docs.
- **Result**: `Peridot: Prune Sessions` and the sidebar prune button prompt
  for status and optional age filters, run `/session prune ... --dry-run`
  first, show the preview in the command-result transcript block, ask for
  explicit confirmation when sessions match, then run the shared
  `/session prune` daemon path and refresh the session list.

### E110. VS Code Session List GUI

- **Status**: landed.
- **Goal**: make persisted session inventory and lifecycle filtering
  discoverable from the editor, not only the slash composer.
- **Where**: VS Code command contributions, sidebar title-bar actions,
  session list command construction, sidebar session reconciliation,
  README, and changelog docs.
- **Result**: `Peridot: Show Sessions` and the sidebar sessions button
  prompt for all sessions or a lifecycle filter, run the shared
  `/session list [--status <state>]` daemon path, render the result in the
  transcript, refresh local session cards for full inventories, and avoid
  pruning unrelated local cards for filtered inventories.

### E111. VS Code Session Search GUI

- **Status**: landed.
- **Goal**: make persisted transcript search discoverable from the editor,
  not only from slash autocomplete.
- **Where**: VS Code command contributions, sidebar title-bar actions,
  session search command construction, README, and changelog docs.
- **Result**: `Peridot: Search Sessions` and the sidebar search button
  prompt for a query, run the shared `/session search <query>` daemon path,
  and render persisted transcript matches in the existing command-result
  transcript block.

### E112. VS Code Session Inspect GUI

- **Status**: landed.
- **Goal**: expose the remaining read-only persisted session inspection
  utilities from the editor without requiring manual slash commands.
- **Where**: VS Code command contributions, sidebar title-bar actions,
  session target command construction, README, and changelog docs.
- **Result**: `Peridot: Show Session Count`, `Peridot: Show Session
  Details`, and `Peridot: Locate Session Directory` run the shared
  `/session count`, `/session show <id>`, and `/session locate <id>` daemon
  paths. Targeted commands fetch persisted sessions, prompt for the target,
  and render the existing structured session result blocks.

### E113. VS Code Session Resume GUI

- **Status**: landed.
- **Goal**: let editor users continue persisted session work without typing
  `/session resume <id>` manually.
- **Where**: VS Code command contributions, sidebar title-bar actions,
  session resume command construction, README, and changelog docs.
- **Result**: `Peridot: Resume Session` fetches persisted sessions, prompts
  for the target, runs the shared `/session resume <id>` daemon path, shows
  the resume summary, and starts the returned continuation task through the
  normal session runner.

### E114. VS Code Session Rename/Delete GUI

- **Status**: landed.
- **Goal**: make individual persisted session lifecycle edits available
  from the editor without typing `/session rename` or `/session delete`.
- **Where**: VS Code command contributions, sidebar title-bar actions,
  session target command construction, README, and changelog docs.
- **Result**: `Peridot: Rename Session` fetches persisted sessions, prompts
  for the target and new title, runs `/session rename <id> <title>`, and
  refreshes the session list. `Peridot: Delete Session` fetches persisted
  sessions, asks for explicit confirmation, runs `/session delete <id>`,
  finishes any cancelled live run, and refreshes the session list.

### E115. VS Code Session New/Switch/Close GUI

- **Status**: landed.
- **Goal**: expose the remaining persisted session lifecycle controls from
  the editor without requiring manual slash commands.
- **Where**: VS Code command contributions, sidebar title-bar actions,
  session lifecycle command construction, README, and changelog docs.
- **Result**: `Peridot: New Session` runs `/session new [task]`, selects the
  daemon-created persisted session, and starts the optional initial task.
  `Peridot: Switch Session` prompts for a persisted session and runs
  `/session switch <id>`, selecting the resolved sidebar session.
  `Peridot: Close Session` prompts for a persisted session, asks for
  explicit confirmation, runs `/session close <id>`, finishes any cancelled
  live run, and refreshes the session list.

### E116. VS Code Session Notes GUI

- **Status**: landed.
- **Goal**: make active-session operator notes discoverable from the editor
  without requiring manual `/note` or `/notes` commands.
- **Where**: VS Code command contributions, sidebar title-bar actions,
  session notes command construction, README, and changelog docs.
- **Result**: `Peridot: Add Session Note` prompts for note text and runs
  `/note <text>`. `Peridot: Show Session Notes` optionally limits the list
  with `/notes last N`, and `Peridot: Clear Session Notes` asks for
  confirmation before running `/notes clear`. All three render through the
  existing structured notes command-result block.

### E117. VS Code Workspace TODO GUI

- **Status**: landed.
- **Goal**: expose the shared `/todos` scanner from the editor without
  requiring manual slash commands.
- **Where**: VS Code command contributions, sidebar title-bar actions,
  README, and changelog docs.
- **Result**: `Peridot: Show Workspace TODOs` runs `/todos` through the
  daemon command path, renders TODO/FIXME/HACK/XXX/BUG hits in the existing
  command-result rows, and preserves file-open affordances for each marker.

### E118. VS Code Context/Diff GUI

- **Status**: landed.
- **Goal**: expose read-only context and working-tree inspection utilities
  from the editor without requiring manual slash commands.
- **Where**: VS Code command contributions, sidebar title-bar actions,
  README, and changelog docs.
- **Result**: `Peridot: Show Context Top` runs `/context top` for the
  active daemon session and renders source token totals plus the largest
  context entries. `Peridot: Show Working Tree Diff` runs `/diff` and
  renders the current Git working tree diff in the sidebar transcript.

### E119. VS Code MCP Server List GUI

- **Status**: landed.
- **Goal**: expose configured MCP server inventory from the editor without
  requiring manual `/mcp list`.
- **Where**: VS Code command contributions, sidebar title-bar action, README,
  and changelog docs.
- **Result**: `Peridot: Show MCP Servers` runs `/mcp list` through the daemon
  command path and renders configured server names, transports, and details
  in the existing structured command-result rows.

### E120. VS Code MCP Server Test GUI

- **Status**: landed.
- **Goal**: expose MCP connectivity checks from the editor without requiring
  manual `/mcp test <name>`.
- **Where**: VS Code command contributions, sidebar title-bar action,
  configured-server picker, README, and changelog docs.
- **Result**: `Peridot: Test MCP Server` uses the workspace MCP server
  inventory to prompt for a target, runs `/mcp test <name>` through the daemon
  command path, and renders the reachable/tool-count result in the existing
  structured command-result block.

### E121. VS Code MCP Server Remove GUI

- **Status**: landed.
- **Goal**: expose MCP server removal from the editor without requiring manual
  `/mcp remove <name>`.
- **Where**: VS Code command contributions, sidebar title-bar action,
  configured-server picker, confirmation prompt, README, and changelog docs.
- **Result**: `Peridot: Remove MCP Server` uses the workspace MCP server
  inventory to prompt for a target, asks for explicit confirmation, runs
  `/mcp remove <name>` through the daemon command path, renders the daemon's
  restart-required note, and refreshes status so server-name autocomplete
  matches the updated config.

### E122. VS Code MCP Server Add GUI

- **Status**: landed.
- **Goal**: expose MCP server registration from the editor without requiring
  manual `/mcp add <name> <transport> <target>`.
- **Where**: VS Code command contributions, sidebar title-bar action, guided
  name/transport/target prompts, README, and changelog docs.
- **Result**: `Peridot: Add MCP Server` validates a unique whitespace-free
  server name, limits transport to `stdio` or `http`, validates the command or
  URL target, runs `/mcp add` through the daemon command path, renders the
  daemon's restart-required note, and refreshes status so server-name
  autocomplete sees the new config entry.

### E123. MCP Inventory Refresh After Config Mutation

- **Status**: landed.
- **Goal**: keep MCP server inventory surfaces current immediately after
  `/mcp add` or `/mcp remove`, instead of relying on a later status poll or
  manual `/mcp list`.
- **Where**: TUI MCP slash handlers, daemon `session.command` MCP results,
  VS Code slash-status refresh trigger, and command-result tests.
- **Result**: TUI `/mcp list`, `/mcp add`, and `/mcp remove` refresh the
  side-panel MCP inventory from `.peridot/config.toml`. Daemon-backed
  `/mcp add` and `/mcp remove` return refreshed `items` rows alongside the
  restart-required message, so VS Code renders the current configured
  inventory in the command result. VS Code composer slashes that mutate MCP
  config now also force a status refresh, keeping `/mcp test|remove`
  autocomplete aligned with the latest config.

### E124. MCP Test Connectivity Metadata

- **Status**: landed.
- **Goal**: preserve the useful connectivity signal from `/mcp test <name>`
  instead of reducing it to a transient text-only success message.
- **Where**: TUI MCP test handler, daemon `session.command` MCP test result,
  VS Code command-result row typing/rendering, and command-result tests.
- **Result**: TUI `/mcp test <name>` refreshes the side-panel MCP inventory
  from config, then marks the tested server connected with its exposed tool
  count on success or disconnected on probe failure. Daemon-backed
  `/mcp test <name>` returns a structured row for the tested server carrying
  `connected` and `tool_count`, and VS Code command rows render those values
  alongside transport metadata.

### E125. MCP Status Panel Visibility

- **Status**: landed.
- **Goal**: make the MCP inventory snapshot visible after it is refreshed,
  not only available as hidden autocomplete context.
- **Where**: TUI side-panel rendering, localized side-panel labels, VS Code
  MCP command-result reconciliation, and MCP command helper tests.
- **Result**: TUI status panel now renders configured MCP servers with
  transport, tool count, and connected/disconnected state. VS Code applies
  structured MCP command-result rows back into sidebar context, so after
  `/mcp list`, `/mcp add`, `/mcp remove`, or `/mcp test`, later MCP pickers
  show the latest transport and connectivity metadata.

### E126. VS Code MCP Context Pill

- **Status**: landed.
- **Goal**: make refreshed MCP inventory visible in the editor chrome, not
  only in command results and slash argument pickers.
- **Where**: VS Code webview context strip, MCP context summary helper, and
  webview unit tests.
- **Result**: the VS Code/Cursor sidebar context strip now shows an `MCP`
  pill with configured/up counts and total tool count. Disconnected servers
  switch the pill to a warning tone, and the tooltip lists server transport,
  tool count, and connection state.

### E127. VS Code Branch Workflow GUI

- **Status**: landed.
- **Goal**: expose the existing branch snapshot and branch-DAG workflow from
  VS Code/Cursor without requiring users to memorize slash commands.
- **Where**: VS Code command contributions, sidebar title-bar actions,
  branch command builders, and branch command unit tests.
- **Result**: editor users can show turn pickers, list saved snapshots, save
  and restore snapshots, fork at a turn id, show the branch tree, and switch
  to a branch limb through command-palette/title-bar actions. Each action
  still routes through the shared `/branch ...` daemon command path used by
  the TUI.

### E128. VS Code Session Utility GUI

- **Status**: landed.
- **Goal**: expose the high-value session recovery/control slashes from the
  editor GUI so operators do not need to remember composer commands.
- **Where**: VS Code command contributions, sidebar title-bar actions, and
  shared slash execution in the sidebar provider.
- **Result**: `Peridot: Compact Context`, `Peridot: Rewind Last Exchange`,
  and `Peridot: Undo Last Change` run `/compact`, `/rewind`, and `/undo`
  through the same daemon slash path used by the composer. Rewind keeps using
  the extension's transcript removal and prompt-restore reconciliation, and
  undo asks for confirmation before restoring the latest file checkpoint.

### E129. VS Code Runtime Control GUI

- **Status**: landed.
- **Goal**: make session runtime controls available from the editor command
  palette instead of requiring users to remember slash commands or focus the
  composer controls.
- **Where**: VS Code command contributions, runtime slash command builders,
  shared slash execution in the sidebar provider, README, and changelog docs.
- **Result**: `Peridot: Set Execution Mode`, `Peridot: Set Permission Mode`,
  `Peridot: Set Reasoning Effort`, `Peridot: Switch Runtime Provider`,
  `Peridot: Set Runtime Model`, and `Peridot: Set Committee Mode` prompt for
  the desired value and run `/execute|/plan|/goal`, `/auto|/safe|/yolo`,
  `/reasoning`, `/provider`, `/model`, and `/committee` through the same
  daemon slash path as the composer. The resulting state delta updates the
  sidebar context and future run options exactly like manual slash input.

### E130. VS Code `@file` Composer Mentions

- **Status**: landed.
- **Goal**: bring the TUI's `@file` auto-mention ergonomics to the VS
  Code/Cursor composer so editor users can point the model at files without
  typing long paths manually.
- **Where**: VS Code workspace file indexing, sidebar context state, webview
  composer picker handling, and file-mention unit tests.
- **Result**: status refreshes now pass a capped workspace-relative file
  index to the webview. While the composer cursor is inside a word-boundary
  `@token`, the picker suggests file paths using the same basename-first
  fuzzy priorities as the TUI. Tab or click replaces the active token with
  `@path/to/file `, and Enter still submits normally. Mentions stay as
  literal navigation hints; file contents are not inlined automatically.

### E131. Fresh `@file` Mention Indexes

- **Status**: landed.
- **Goal**: keep `@file` autocomplete useful during long-running TUI and
  VS Code/Cursor sessions after files are created or deleted.
- **Where**: TUI host persistence tick and VS Code workspace file watcher.
- **Result**: the TUI can force-refresh its cached picker index and rebuilds
  it while the `@file` picker is open. The VS Code extension listens for
  workspace create/delete events, debounces them, and updates only the
  `workspaceFiles` sidebar context so the composer sees fresh paths without a
  full status refresh.

### E132. VS Code Image Attachment Previews

- **Status**: landed.
- **Goal**: make image attachments visually inspectable in the editor
  surface without changing the model-context contract.
- **Where**: VS Code command-result decoration, sidebar webview resource
  roots, attachment card renderer, and attachment-preview unit tests.
- **Result**: workspace-local image attachments receive a bounded webview
  preview URI when the host handles `/attach`, `/attachments`, or `/detach`
  results. The webview renders non-SVG image previews in attachment cards and
  inventory rows, while the daemon still stores image attachments as
  placeholder metadata rather than inlining binary contents into context.

### E133. VS Code Composer Image Paste/Drop Attachments

- **Status**: landed.
- **Goal**: let Cursor/VS Code users attach screenshots without leaving the
  composer.
- **Where**: webview composer paste/drop handlers, inline image payload
  helpers, extension-host attachment persistence, and unit tests.
- **Result**: PNG, JPEG, GIF, WebP, and BMP files pasted into or dropped on
  the composer are encoded in the webview, decoded by the extension host,
  saved under `.peridot/attachments/<session>/`, and attached through the
  existing `/attach` daemon command. The same size and media guardrails are
  enforced on both sides of the webview boundary.

### E134. Workspace File Slash Argument Autocomplete

- **Status**: landed.
- **Goal**: let users complete file-path arguments inside the slash composer
  instead of manually typing fragile relative paths.
- **Where**: TUI slash picker, VS Code webview slash autocomplete, shared
  workspace file indexes, and unit tests.
- **Result**: `/attach <path>` and `/codemap outline <path>` now reuse the
  same fuzzy workspace-relative file index as `@file` mentions. Exact completed
  paths close the argument picker so the command remains directly runnable.

### E135. Attached File Detach Autocomplete

- **Status**: landed.
- **Goal**: make attachment removal use the same path-completion ergonomics
  as attachment creation.
- **Where**: TUI session state, TUI slash picker, VS Code sidebar context,
  VS Code webview slash autocomplete, and unit tests.
- **Result**: `/detach <path>` now suggests paths currently known to be
  attached to the active session. The cache updates from `/attach`,
  `/attachments`, and `/detach` results, and exact completed paths remain
  directly runnable.

### E136. Session-Local Attachment Path Cache

- **Status**: landed.
- **Goal**: keep `/detach` autocomplete scoped to the active VS Code/Cursor
  chat session after session switches and window reloads.
- **Where**: VS Code persisted sidebar session state and attachment-path
  normalization helpers.
- **Result**: attached paths are saved on each stored chat session, restored
  when that session becomes active, reset for draft/cleared sessions, and
  sanitized while loading older persisted snapshots.

### E137. Shared TODO Marker Index

- **Status**: landed.
- **Goal**: stop `/todos` from maintaining a separate workspace walk path when
  the code-map index already contains TODO/FIXME/HACK markers.
- **Where**: shared Rust code-map index metadata, TUI `/todos`, daemon
  `session.command` `/todos`, and VS Code TODO command documentation.
- **Result**: `/todos` now loads `.peridot/codemap.json`, refreshes it when
  source files changed or when a previous index was built with too small a
  TODO cap, and renders the same indexed marker data in TUI and VS Code.

### E138. VS Code Code-Map Freshness Context

- **Status**: landed.
- **Goal**: make the editor surface show whether the persisted code-map/TODO
  index is fresh without requiring a separate status command.
- **Where**: VS Code command-result context folding, workspace file watcher,
  sidebar context strip, and webview unit tests.
- **Result**: `/codemap`, `/codemap status`, and `/todos` results update a
  code-map freshness pill with symbol/TODO counts. Workspace file
  create/change/delete events mark the pill stale until the next indexed
  command refreshes `.peridot/codemap.json`.

### E139. TUI Code-Map Freshness Panel

- **Status**: landed.
- **Goal**: keep the terminal side panel aligned with the editor code-map
  freshness surface so operators can see whether the persisted index is fresh
  without re-reading transcript output.
- **Where**: TUI side-panel state, localized side-panel labels, code-map slash
  handlers, and TUI render tests.
- **Result**: `/codemap`, `/codemap status`, `/codemap find|locate|outline|refs`,
  and `/todos` update a persisted code-map summary in the side panel with
  fresh/stale/missing state, symbol/TODO counts, indexed/source file counts,
  and refresh timestamp metadata.

### E140. Status Code-Map Snapshot

- **Status**: landed.
- **Goal**: make code-map freshness visible during normal editor status
  refreshes, not only after a `/codemap` command result.
- **Where**: daemon `peridot.status`, VS Code status normalization, and
  code-map context unit tests.
- **Result**: `peridot.status` now includes an additive `code_map` snapshot
  with fresh/stale/missing state and counts. VS Code folds that snapshot into
  the sidebar context strip so the code-map pill can appear on startup or
  manual refresh before any codemap command is run.

### E141. Attachment Context Status

- **Status**: landed.
- **Goal**: keep attached files visible after `/attach` or `/attachments`
  output scrolls away, using the session-local attachment cache already shared
  by `/detach` autocomplete.
- **Where**: TUI side-panel rendering, localized attachment labels, VS Code
  context strip helper, README, changelogs, and unit tests.
- **Result**: TUI side panels now show an Attachments block with the current
  session's attached paths. VS Code/Cursor shows an `Attachments N` context
  pill with attached paths in the tooltip, and both surfaces stay in sync with
  `/attach`, `/attachments`, `/detach`, and session switches.

### E142. Operator Notes Context Status

- **Status**: landed.
- **Goal**: keep active-session operator notes visible after `/note` or
  `/notes` output scrolls away.
- **Where**: TUI side-panel rendering, localized note labels, VS Code
  note context normalization, context-strip pill, persisted sidebar session
  state, README, changelogs, and unit tests.
- **Result**: TUI side panels now show a Notes block with the active
  session's note count and latest note text. VS Code/Cursor folds `/note`,
  `/notes`, and `/notes clear` results into a session-local `Notes N`
  context pill and persists that summary per chat session.

### E143. Session Notes Summary Hydration

- **Status**: landed.
- **Goal**: keep note status accurate after session list refreshes, reloads,
  and foreground swaps instead of requiring another `/notes` command.
- **Where**: persisted-session hydration, TUI foreground swap state,
  daemon `session.list`, VS Code session normalization/reconciliation, and
  unit tests.
- **Result**: session directory rows now carry additive `notes_count` /
  `last_note` metadata. TUI foreground swaps hydrate the active note side-panel
  summary from that directory metadata, and VS Code/Cursor folds daemon
  `session.list` snapshots into the per-session `Notes N` context pill.

### E144. Session Attachment Summary Hydration

- **Status**: landed.
- **Goal**: keep attachment status accurate after session list refreshes,
  reloads, and foreground swaps instead of requiring another `/attachments`
  command.
- **Where**: persisted-session hydration, TUI foreground swap state,
  daemon `session.list`, VS Code session normalization/reconciliation, and
  unit tests.
- **Result**: session directory rows now carry additive `attachment_count` /
  `attachment_paths` metadata reconstructed from persisted context snapshots.
  TUI foreground swaps hydrate the active attachment side-panel and `/detach`
  autocomplete from that directory metadata, and VS Code/Cursor folds daemon
  `session.list` snapshots into the per-session `Attachments N` context pill.

### E145. Session Context Change Broadcasts

- **Status**: landed.
- **Goal**: keep subscribed editor session lists current when session context
  metadata changes from another client or window.
- **Where**: daemon `session.command` result handling, `session.list_changed`
  emission, and daemon subscription tests.
- **Result**: successful `/note`, `/attach`, and mutating `/detach` commands
  now emit `session.list_changed` for subscribed clients. VS Code/Cursor
  already reconciles those notifications, so note and attachment context pills
  update across windows without a manual session-list refresh.

### E146. Session Show Attachment Summary

- **Status**: landed.
- **Goal**: make targeted session inspection show the same attachment context
  now available in session lists and side panels.
- **Where**: `peridot session show`, TUI `/session show`, daemon
  `/session show`, VS Code command-result typing, and daemon tests.
- **Result**: session show summaries now include additive
  `attachment_count` / `attachment_paths` metadata. CLI text prints attached
  paths under the session detail block, TUI `/session show` includes the same
  rows in the transcript, and VS Code/Cursor renders the attachment count in
  `Peridot: Show Session Details` through the existing command-result table.

### E147. Session Show Attachment Path Rows

- **Status**: landed.
- **Goal**: make the VS Code/Cursor session detail block expose attached file
  paths directly, not only the aggregate attachment count.
- **Where**: daemon `/session show` command-result rows and daemon tests.
- **Result**: `/session show <id|title>` now appends one `source:
  "attachment"` item per attached path. The existing VS Code/Cursor generic
  command-result renderer turns those `path` fields into clickable file rows
  without changing the backwards-compatible top-level
  `attachment_count` / `attachment_paths` payload.

## Notes

- Keep attachment state session-local. Do not introduce hosted state.
- Prefer reconstructing artifact metadata from existing session context
  entries until there is a stronger reason to add a separate attachment
  registry file.
- Continue routing through daemon slash/RPC paths so TUI and extension
  behavior stays shared.
