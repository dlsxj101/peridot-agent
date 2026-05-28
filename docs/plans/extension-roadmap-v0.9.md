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

## Notes

- Keep attachment state session-local. Do not introduce hosted state.
- Prefer reconstructing artifact metadata from existing session context
  entries until there is a stronger reason to add a separate attachment
  registry file.
- Continue routing through daemon slash/RPC paths so TUI and extension
  behavior stays shared.
