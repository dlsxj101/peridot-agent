# Peridot Agent — Extension Changelog

## Unreleased

### Added — surface-aware slash autocomplete

- Slash catalog entries may now carry client surface metadata. The VS
  Code composer still accepts older daemon catalogs, but filters out
  TUI-only suggestions such as `/collapse` and `/lang` when the metadata
  is present.
- Slash catalog entries may also carry structured `arg_options`; the
  composer uses those choices before falling back to legacy `arg_hint`
  parsing.
- The extension now requests `session.command_catalog` with
  `surface: "vscode"`, so modern daemons filter editor-inapplicable
  commands before the catalog reaches the webview.
- `/help` now runs through daemon `session.command` and renders the same
  structured, surface-filtered catalog rows as other command results.
- Skill-aware slash autocomplete now refreshes when `.peridot/memory.db`
  changes and immediately after `/skills archive` or `/skills restore`, so
  `/skill-name` suggestions stay current without a manual status refresh.
- Skill-aware argument autocomplete now fills active skill names for
  commands such as `/skills show <name>`, `/skills use <name>`, and
  `/skills archive <name>`, and fills archived skill names for
  `/skills restore <name>` without showing archived skills as direct
  `/skill-name` suggestions.
- Session-target argument autocomplete now fills stable session ids for
  `/session switch|close|delete|rename|show|locate|resume`, including
  matches from visible session titles.
- Session subcommand continuation autocomplete now accepts partial
  subcommands such as `/session sw` into `/session switch ` so users can
  immediately type or autocomplete the target session.
- `/session search <query>` now runs through the daemon command path and
  renders structured cross-session transcript hits in the same command
  result block as other session utilities.
- `/session resume <id|title>` now runs through the daemon command path and
  starts a continuation task from the selected persisted session summary.
- `/session list --status <state>` now runs through the daemon command path
  and filters persisted sessions by lifecycle state, with autocomplete for
  the allowed status values.
- `Peridot: Show Sessions` now exposes the same all/filtered persisted
  session inventory from the command palette and sidebar title bar. Filtered
  results no longer prune unrelated local session cards.
- `Peridot: Search Sessions` now exposes persisted transcript search from
  the command palette and sidebar title bar, using the shared
  `/session search <query>` daemon command.
- `Peridot: Show Session Count`, `Peridot: Show Session Details`, and
  `Peridot: Locate Session Directory` now expose `/session count`,
  `/session show <id>`, and `/session locate <id>` from the command palette
  and sidebar title bar, with persisted-session picking for target commands.
- `Peridot: Resume Session` now exposes `/session resume <id>` from the
  command palette and sidebar title bar, then starts the returned
  continuation task through the normal session runner.
- `Peridot: Rename Session` and `Peridot: Delete Session` now expose
  individual persisted session lifecycle edits from the command palette and
  sidebar title bar. Delete asks for explicit confirmation before running
  `/session delete <id>`.
- `Peridot: New Session`, `Peridot: Switch Session`, and `Peridot: Close
  Session` now expose the remaining persisted session lifecycle commands
  from the command palette and sidebar title bar, with daemon-backed
  selection and session-list reconciliation.
- `Peridot: Add Session Note`, `Peridot: Show Session Notes`, and
  `Peridot: Clear Session Notes` now expose active-session notes from the
  command palette and sidebar title bar through the shared daemon slash
  paths.
- `Peridot: Show Workspace TODOs` now exposes `/todos` from the command
  palette and sidebar title bar, rendering marker hits with existing
  file-open rows.
- `Peridot: Show Context Top` and `Peridot: Show Working Tree Diff` now
  expose `/context top` and `/diff` from the command palette and sidebar
  title bar.
- `Peridot: Show MCP Servers` now exposes `/mcp list` from the command
  palette and sidebar title bar, rendering configured server names,
  transports, and details through the existing command-result block.
- `Peridot: Add MCP Server` now exposes `/mcp add <name> <transport>
  <target>` through guided name, transport, and command/URL prompts.
- `Peridot: Test MCP Server` now exposes `/mcp test <name>` from the
  command palette and sidebar title bar with a configured-server picker.
- `Peridot: Remove MCP Server` now exposes `/mcp remove <name>` with a
  configured-server picker, confirmation prompt, and post-removal status
  refresh.
- MCP config mutation commands now refresh editor state more directly:
  daemon-backed `/mcp add` and `/mcp remove` return the refreshed server
  inventory rows, and VS Code composer slashes force a status refresh so
  `/mcp test|remove` autocomplete follows the latest `.peridot/config.toml`.
- `/mcp test <name>` command results can now carry and render structured
  connectivity metadata, including `connected` and `tool_count`, alongside
  the server transport.
- `/session prune [--status <state>] [--older-than-days N] [--dry-run]`
  now runs through the daemon command path and returns a structured prune
  result so editor users can preview or remove stale persisted sessions.
- `Peridot: Prune Sessions` now exposes session cleanup from the command
  palette and sidebar title bar with status and age filters, a mandatory
  dry-run preview, and a final confirmation before deletion.
- `/session replay <id|title> [--last N]` now runs through the daemon
  command path and renders persisted replay timeline rows, including
  committee-weaved events, in the existing command result block.
- `Peridot: Replay Session Timeline` now exposes replay from the command
  palette and sidebar title bar with persisted-session picking and an
  optional recent timeline-entry limit.
- `/session export <id|title> [attachments|notes|timeline|full]` now runs
  through the daemon command path, so editor users can export artifacts
  from any persisted session instead of only the active session.
- `Peridot: Export Session Artifacts` now lets editor users choose an
  active or persisted session before picking the destination folder.
- `/session import <dir> [--id <id>] [--force]` now runs through the
  daemon command path, so editor users can restore portable persisted
  session directories without switching to the terminal.
- Session import is now available from the command palette and sidebar title
  bar with a folder picker, optional imported id, and explicit overwrite
  choice.
- `/notes clear` now runs through the daemon command path and clears
  operator notes for the active session, with autocomplete alongside
  `/notes last`.
- `/session show <id|title>` now runs through the daemon command path and
  renders structured persisted session details without requiring a separate
  terminal invocation.
- `/session locate <id|title>` now runs through the daemon command path and
  returns the persisted session directory as a structured path row.
- Accepting free-form slash commands now leaves an editable argument slot
  instead of copying placeholder text such as `<task>` or `<objective>`
  into the composer.
- `/committee planner|full` now updates VS Code session status context
  and renders a committee mode pill, matching the TUI status surface.
- Committee planner and reviewer daemon events now render in the VS Code
  transcript: planner plans appear as committee progress rows, reviewer
  verdicts include the executor turn and comments, and reviewer blocks use
  error styling.
- Auto-fix attempt daemon events now render in the VS Code transcript with
  the checked tool, pass/fail status, and attempt count, matching the TUI
  transcript wording.
- Live MCP status daemon events now update VS Code's MCP server snapshot
  for `/mcp remove|test` autocomplete and no longer appear as opaque
  transcript event-kind rows.
- AGENTS.md hot-reload daemon events now update a VS Code context-strip
  `AGENTS <rule-count>` pill with source paths, matching the TUI
  instruction-summary side panel.
- Session save daemon events now render as resume-ready VS Code transcript
  rows, and session save failures render as error rows instead of opaque
  event-kind labels.
- Hook activity daemon events now render as named VS Code transcript rows,
  with blocking or failing outcomes promoted to error rows.
- Run-start daemon events now move the VS Code sidebar from
  `Starting daemon` to `Running` immediately while remaining out of the
  transcript.
- Interrupted daemon events now stop the active VS Code run, set the
  sidebar status to `Interrupted`, and use the same active-run cleanup path
  as other terminal events.
- File links now try workspace-name-prefixed and normalized relative path
  candidates before falling back to basename search, which improves Cursor
  workspaces opened one level above or below the daemon project root.
- Completed run duration now lands in the transcript instead of staying pinned
  above the composer, and session title rename inputs no longer re-select text
  on every sidebar refresh.
- Abbreviated file links containing `...` now expand to workspace globs, and
  Java-like reordered camel-case hints such as `ApiKeyMongo.java` can resolve to
  the best matching file under the same project prefix.
- Completed run duration now renders as a transcript completion bubble, so the
  final timing reads as part of the conversation rather than a composer-pinned
  status line.
- Session title rename drafts now preserve empty input across sidebar refreshes,
  avoiding title text being restored over slow edits or deletions.
- Approval prompts now show the same risk-class chip as tool cards, and
  unknown future risk labels are sanitized into a stable fallback CSS class.
- Live usage and budget updates now render as compact composer chips for
  executor tokens, aggregate executor+committee cost, budget percentage, and
  turn pressure.
- Composer metric docks now skip empty run-metrics containers when a session
  has no live HUD values, matching the actual usage/budget chip behavior.
- Settings number fields now keep empty or invalid drafts out of the save
  payload until blur restores the visible value, and integer-backed settings
  normalize decimal input before sending `settings.save`.
- Unknown future daemon event kinds are now treated as no-op transcript
  entries in the VS Code sidebar instead of leaking opaque status rows.
- `agent_ask_user` pauses now move VS Code status to `Waiting for user
  response`, and stale or rejected responses no longer replace the prompt
  with `User response sent`.
- Recovery daemon events now stay in the VS Code Output channel instead of
  rendering as sidebar transcript rows.
- Recovery daemon events now format as readable Output channel lines while
  unknown additive events stay logged with JSON payloads for debugging.
- Provider argument autocomplete now fills supported provider ids for
  `/provider <claude-api|openai-api|openrouter-api|openai-oauth>` from the
  shared daemon slash catalog.
- Code-map subcommand autocomplete now fills `/codemap status|refresh|find|locate|outline|refs`
  from the shared daemon slash catalog.
- Code-map continuation autocomplete now accepts partial subcommands such
  as `/codemap loc` into `/codemap locate ` so users can immediately type
  the symbol, path, or query.
- `/context` is now advertised alongside `/context top`, matching the
  daemon parser and shared TUI catalog.
- MCP add transport autocomplete now fills `stdio` or `http` after
  `/mcp add <name> ` and leaves the composer ready for the command or URL.
- MCP server-name autocomplete now fills configured server names for
  `/mcp remove <name>` and `/mcp test <name>` after status refresh.
- Model-name autocomplete now fills configured main, subagent, and committee
  role models for `/model <name>` and `/subagent model <name|reset>`.
- Branch snapshot autocomplete now fills saved `.peridot/branches` snapshot
  names for `/branch restore <name>` after status refresh.
- Branch subcommand autocomplete now fills parser-supported
  `/branch save|restore|turn|switch` forms with a trailing argument slot,
  and `/branch turn <turn-id>` is now present in the shared catalog.
- Goal and notes subcommand autocomplete now fills
  `/goal pause|resume|clear|status` and `/notes last` while preserving
  free-form goal objectives and bare `/notes`.
- Export artifact autocomplete now supports multi-artifact
  `/export attachments notes timeline` composition and suggests only
  remaining artifact classes after each accepted token.
- Think alias autocomplete now suggests the parser-supported `/think`
  arguments, including `hard`, `harder`, `more`, `stop`, and `less`.
- Fast and autofix autocomplete now suggests parser-supported aliases such
  as `/fast standard` and `/autofix false` while keeping numeric autofix
  limits free-form.
- Skills search autocomplete now accepts `/skills se` into
  `/skills search ` so users can immediately type the free-form query.
- Skills management autocomplete now accepts partial subcommands such as
  `/skills sh` into `/skills show ` so users can immediately type or
  autocomplete the skill name.
- The composer now keeps submitted prompt history per sidebar session.
  ArrowUp / ArrowDown recall previous prompts when the caret is on the
  first or last textarea line, and unsent drafts no longer leak across
  session switches. The history and drafts are restored from VS Code
  webview state after a webview reload.
- `/sidepanel` is now filtered out of VS Code slash autocomplete and
  `/help` because it only toggles the TUI status panel.
- `/session new [task]` now applies the daemon's structured
  `session_new` result, so the extension no longer has to re-parse that
  slash command on modern daemons.
- `/session list` now reconciles the sidebar's session cards from the
  daemon's returned session inventory in addition to rendering the
  transcript result.
- `/session save` now refreshes the saved session's VS Code sidebar
  metadata from the daemon's structured result, including status and
  usage fields when present.
- Bare `/goal` now routes through daemon `session.command` like `/plan`
  and `/execute`, applying the shared goal-mode state delta instead of a
  VS Code-local parser branch.
- `/session switch` now reconciles the selected sidebar session from the
  daemon result, including persisted usage metadata when present.
- `/session rename` now reconciles the renamed sidebar session from the
  daemon result, keeping persisted status and usage metadata fresh.
- `/session new [task]` now materializes a daemon-backed idle session id
  before VS Code selects the new card or starts the optional task.
- Full daemon session inventories now prune missing daemon-backed VS Code
  sidebar cards, so `session.list_changed` cannot leave deleted sessions
  stale in the editor.
- Extension unit tests now cover daemon-backed session pruning, including
  empty authoritative inventories and local draft preservation.
- Slash autocomplete filtering and finite-argument picker behavior now live
  in a tested pure helper shared by the VS Code webview.
- VS Code CI and release packaging now run `npm test` before building or
  publishing the extension, so webview/sidebar unit regressions block the
  pipeline.
- Local VSIX packaging now runs `npm test` through the `vscode:prepublish`
  hook before creating a package.
- VSIX packaging now excludes TypeScript test outputs and source-only
  development files so `npm test` does not leak `out-test/` into packages.
- VS Code CI/release packaging now relies on the same `vscode:prepublish`
  test gate instead of running duplicate test/build steps before `vsce package`.
- VS Code local slash handling now only accepts real editor-local actions
  after a daemon `action: "local"` result. Daemon-backed commands such as
  `/info`, `/cost`, `/plan show`, and `/session list` no longer have stale
  sidebar re-parser fallbacks.
- `/status` is now part of the shared slash catalog, so the VS Code
  composer and `/help` can discover the status alias that the daemon
  already accepts.

### Added — compaction snapshot details

- `Context compacted` transcript rows now expand into a structured
  snapshot with retained decisions, files read/changed, verification
  records, approvals, todos, and untrusted inputs.
- Slash autocomplete now includes active auto-skills from the daemon's
  `skills.list` RPC, so stored skills appear as `/skill-name`
  suggestions alongside built-in commands.
- The bundled CLI now understands Hermes-style skill directories with
  `SKILL.md`, `references/`, and `templates/`, including
  `skill_view_ref` access for reference files.
- `/codemap` is available through the shared slash catalog and renders
  workspace public symbols plus TODO markers as structured command rows.
- `Peridot: Show Workspace Code Map` and the sidebar header code-map
  button run that shared `/codemap` scan without typing the slash command.
- Code map results now render as a grouped symbol/TODO panel with an
  inline filter instead of a generic command-row list.
- `Peridot: Locate Workspace Symbol` runs `/codemap locate <symbol>`,
  appends the ranked symbol-location result, and opens the first matching
  indexed definition in the editor.
- `Peridot: Outline Current File` and the sidebar outline button run
  `/codemap outline <path>` for the active editor file and render indexed
  symbols in the existing code-map panel.
- `Peridot: Find Workspace Symbol References` and the sidebar references
  button run `/codemap refs <symbol>` and render matching source lines as
  a dedicated code-map references group.
- `Peridot: Show Workspace Code Map Status` and the sidebar status button
  run `/codemap status`, showing whether `.peridot/codemap.json` is
  missing, fresh, or stale before a refresh.
- Workspace code-map commands now auto-refresh stale `.peridot/codemap.json`
  indexes before returning overview, search, locate, outline, or reference
  rows, and the sidebar chip reflects the refresh.
- Stale detection also catches added or deleted source files, so removed
  symbols do not linger in VS Code code-map results.
- Source fingerprints now make stale detection robust for rapid same-second
  edits and same-file-count content changes.
- The composer now supports `/skills` and `/skills list`, rendering active
  stored skills in a VS Code card with copyable slash invocations.
- Added `Peridot: Show Skills` and a sidebar header button that route
  through the same `/skills` daemon path.
- Skill inventory rows now include pin/unpin buttons backed by
  `/skills pin <name>` and `/skills unpin <name>`.
- Skill inventory rows now include a detail button backed by `/skills
  show <name>`, rendering the stored skill body in a dedicated card.
- `Peridot: Search Skills` and a sidebar header search button run
  `/skills search <query>` and render filtered skill inventory results.
- Skill inventory and detail cards now include a Use button backed by
  `/skills use <name>`, loading the stored skill into session context.
- Skill inventory and detail cards now include a confirm-before-archive
  action backed by `/skills archive <name>`.
- Archived skill inventory is available from the sidebar and command
  palette, with Restore actions backed by `/skills restore <name>`.
- Archived skill search is available from the sidebar and command
  palette, and archived skill detail cards show Restore instead of Use.
- `/note <text>` now persists through the daemon for VS Code sessions,
  and `/notes [last N]` renders a structured session notes card.
- GitHub PR workflow commands are available from VS Code: PR status,
  preview-and-confirm `peridot ship`, and confirm-before-merge
  `gh pr merge`.
- File attachments are available through `/attach <path>` and the VS
  Code file picker. UTF-8 files are inlined into session context; image
  files are represented with a metadata placeholder.
- Attachment results now render as compact attachment blocks with open
  and copy actions, including image-placeholder metadata.
- `Peridot: Show Session Attachments` runs `/attachments` and renders
  the current session's attachment inventory with open/copy actions.
- Attachment cards now include a confirm-before-detach action backed by
  `/detach <path>`, so stale session context can be removed from the
  sidebar.
- `Peridot: Export Session Artifacts` exports the active session's
  attachments, notes, and replay timeline into a portable directory and
  reveals it from VS Code.
- `/export [attachments|notes|timeline|full]` is available through the
  shared slash catalog; composer results render as an export card with
  open/copy actions for the generated directory.
- Daemon status now reports stale Peridot worktree reconciliation. Clean
  orphaned worktrees are removed automatically; dirty preserved
  worktrees surface as a sidebar warning instead of staying invisible.
- `/session count` is available through the shared slash catalog and
  returns the persisted lifecycle breakdown as a structured command
  card.
- `/info` now runs through the daemon-backed slash path and returns a
  structured session info card with session id, workspace, provider,
  model, mode, permission, turn, token, and cost context.
- `/cost` now runs through the daemon-backed slash path and returns
  current-session plus aggregate executor/committee token and cost
  totals, including the active budget limit when available.
- `/plan show` now runs through the daemon-backed slash path and returns
  the live plan snapshot as structured command rows.
- `/session save` now runs through the daemon-backed slash path and
  persists the active daemon session record immediately, including live
  token, cost, and turn totals.
- `/goal pause`, `/goal resume`, `/goal clear`, and `/goal status` now
  run through the daemon-backed slash path and return structured goal
  state cards.
- `/session rename` and `/session delete` now run through the
  daemon-backed slash path, mutate persisted session records/blobs, and
  update matching sidebar sessions by daemon id.
- `/rewind` now runs through the daemon-backed slash path, removes the
  last user turn from the session context snapshot, and refills the
  composer draft with the restored prompt.
- `/clear` now uses the daemon-backed slash path to cancel the active
  daemon session and delete matching persisted session records/blobs
  before the sidebar opens a fresh local transcript.
- `/session close <id|title>` now uses the daemon-backed slash path,
  matching TUI close semantics and cleaning up any active VS Code run
  bookkeeping after the daemon cancels the session.
- `/session switch <id|title>` now uses daemon target resolution, so
  persisted sessions can be selected from the composer even when the
  local sidebar had not materialized the card yet.
- `/goal <objective>` now uses the daemon-backed slash path to return a
  goal-mode `start_task` result, keeping mode changes and task launch on
  the shared command contract.
- `/codemap` now uses a persisted `.peridot/codemap.json` index, and
  `Peridot: Refresh Workspace Code Map Index` rebuilds it explicitly.
- `Peridot: Search Workspace Code Map` runs `/codemap find <query>`
  against the persisted index and renders matches in the code-map panel.
- The sidebar subscribes to the daemon's session list and reconciles
  `.peridot/memory.db` session records, so sessions started or finished
  from another VS Code window can appear in the local session menu after
  the shared store changes.
- A VS Code Get Started walkthrough now walks users through opening the
  sidebar, connecting a provider, reviewing settings, and running a
  first task.
- The `vsce/v*` release workflow now fails before publishing if the tag
  version does not match `extensions/vscode/package.json`, preventing
  accidental Marketplace/Open VSX version skew.

## [0.5.20] — 2026-05-26

### Added — run timing footer

- Long-running sidebar tasks now show a Peridot gem loader with live
  elapsed time while the daemon is working.
- Completed tasks keep the final elapsed time in the bottom footer so
  slow provider responses are visible after the run finishes.
- Assistant copy actions moved into the message footer instead of
  floating over the response body.

### Fixed — ask-user fallback input

- `agent_ask_user` single-select and multi-select prompts in the
  sidebar now always include an `Other` option with a free-form text
  input, matching the TUI behavior and the daemon contract.
- Choosing `Other` sends a text answer back to the daemon so the agent
  can continue with the operator's custom response instead of being
  limited to model-proposed choices.
- Extension package 0.5.19 → 0.5.20.

## [0.5.19] — 2026-05-26

### Added — request-context details and mutation markers

- The context donut now reflects the daemon's estimated next request
  footprint, with tooltip lines for persisted context, messages,
  system prompt, tool schemas, and request overhead.
- Tool-result summaries include `mutated=true|false` when the daemon
  reports worktree mutation status.

### Changed — quieter phase display

- Routine `phase_changed` events update the sidebar status chip without
  adding transcript rows.
- The user-facing label for the daemon's verifying phase is now
  `checking`, matching what the harness is actually doing between tool
  calls.
- Extension package 0.5.18 → 0.5.19.

## [0.5.18] — 2026-05-26

### Added — Cursor remote install workaround and settings polish

- Added `scripts/install-cursor-remote.sh`, a remote-host installer for
  Cursor builds that fail to update Marketplace VSIX packages served
  with `Content-Encoding: gzip`. The script downloads the decoded VSIX
  with `curl --compressed`, validates the ZIP header, and installs it
  through the newest `~/.cursor-server/.../cursor-server` binary.
- Documented the Cursor remote installer workaround in the Marketplace
  README so affected users can recover from
  `End of central directory record signature not found` without waiting
  for a Cursor remote updater fix.
- Localized the settings webview shell strings through VS Code `l10n`
  and added Korean translations.

### Changed — release metadata

- Extension package 0.5.17 → 0.5.18.
- Settings save/reload controls now expose in-flight save state,
  unsaved-change confirmation, focus-visible toggle outlines, aria-live
  save feedback, and a narrower mobile layout.

## [0.5.17] — 2026-05-25

### Added — settings webview, slash skills, schema-version handshake

- **`Peridot: Open Settings`** opens a form-style editor in the editor
  area (also reachable via a gear icon in the sidebar title bar).
  The webview talks to the daemon's `settings.list` / `settings.save`
  RPC and renders the same curated registry the TUI's `peridot
  setting` screen uses, with checkbox / dropdown / number inputs and
  a flash status for save outcome. New sessions started after a save
  pick up the new values automatically.
- **Skill slash commands**. The webview now recognises
  `/auto-fix-parser-tests`, `/ship-daily`, and any other kebab-case
  skill name registered in the project's auto-skill store. The daemon
  looks the name up, returns the SKILL body, and the extension
  surfaces it as a status entry so the model picks it up on the next
  turn.
- **Daemon handshake parity**. The extension now reads the daemon's
  `peridot.handshake` notification and shows an explicit
  "extension/daemon schema version mismatch" warning if the daemon's
  `AGENT_RUN_EVENT_SCHEMA_VERSION` doesn't match the bundled
  expectation. Prevents silent breakage when shipping a new extension
  against an older daemon.
- **`ContextCompacted` + `PhaseChanged` events**. Compaction now
  surfaces as a single status row showing `Context compacted (N files
  read, M untrusted)`. Phase transitions still appear when something
  notable happens (entering recovery, delegating, hitting done).
- **LLM session titles**. The extension now requests an LLM-authored
  session title from the daemon (`session.generate_title`) and
  replaces the placeholder once it lands. Failed generations fall
  back to `"No title"` rather than the raw truncated task. Sessions
  the operator renames manually are preserved via a `userRenamed`
  flag.

### Changed — quieter transcript, surface-aware settings filter

- **Routine phase transitions hidden**. `Executing ↔ Verifying ↔
  Planning` ping-pong no longer floods the chat. Only `Recovering`,
  `Delegating`, or `Done` phase changes appear as status rows. The
  underlying ndjson event stream is unchanged so debugging /
  automation hooks still see every transition.
- **TUI-only settings hidden in the webview**. `tui.show_thinking`,
  `tui.show_token_count`, `tui.show_cost`, `tui.show_mascot` only
  affect the terminal UI; the webview filters them out by reading
  the `surfaces` field on each `SettingItem`. The full item list is
  still shipped to the daemon on save so the TUI sees the same
  values.
- **Tool chip risk colouring**. `tool_started` chips carry the
  Rust-side `risk_class` (read-only / local-write / build-or-test /
  external-network / destructive / secret-adjacent) and the webview
  renders them with matching colours so an operator can spot
  destructive shell commands at a glance.

### Migration notes

- Extension package 0.5.16 → 0.5.17.
- Daemon must be v0.8.11+ for the handshake check to pass without a
  warning. Older daemons still work — the version mismatch is a
  warning, not a fatal error.

## [0.5.16] — 2026-05-23

### Added — multi-run parity and canonical slash state

- Running daemon tasks are tracked per chat session instead of through one
  global extension-host active run, so the sidebar's multi-session UI matches
  daemon execution state more closely.
- Slash commands with finite options now receive canonical state deltas from
  the daemon/core parser instead of being re-parsed locally in the sidebar.
- Daemon-backed `/reasoning`, `/think`, `/fast`, `/committee`, `/lang`,
  `/autofix`, provider, mode, permission, and subagent model changes keep the
  UI run options in sync with daemon session specs.

### Fixed — transcript and tool-call polish

- Tool-call rows keep stable keys and collapsed one-line tool lists animate
  when the visible tool summary changes, without forcing a full transcript
  replacement.
- Tool names render the animated gradient on the glyphs themselves, with a
  seamless repeat and reduced-motion fallback.
- The context indicator shows only the donut ring, without the extra circular
  wrapper.

## [0.5.14] — 2026-05-22

### Added — daemon slash command parity and persistent sessions

- Branch, MCP, compact, TODO, diff, undo, and context slash commands now call
  the daemon `session.command` RPC instead of using local placeholder text.
- The extension asks the daemon for `session.command_catalog`, so the webview
  slash picker and help text use the same catalog as the TUI instead of a
  duplicated local list.
- Mode, permission, model, provider, reasoning, fast tier, committee, note,
  autofix, collapse, clear, and goal control slashes now flow through daemon
  command results before the sidebar applies matching local UI state.
- `/branch` without arguments now renders a picker UI from the daemon's
  structured `branch_picker` result; selecting a row runs `/branch turn <id>`.
- Open chat sessions, transcripts, daemon session ids, queued prompts, and run
  options are restored from workspace storage after Extension Host reloads.
- Branch/MCP/TODO/context/diff command results render as structured rows in the
  sidebar, and ChatGPT OAuth shows a manual login link when browser handoff is
  attempted.
- The local development launch path typechecks the extension, builds and
  bundles the release CLI, and smoke-checks the bundled binary before opening
  an Extension Development Host.
- The VSIX package now includes the MIT license file.

## [0.5.11] — 2026-05-21

### Added — persistent chat sessions and cleaner transcript UX

- Follow-up prompts now continue the active Peridot daemon session until the
  user explicitly runs `/clear` or opens a new session. The sidebar keeps the
  transcript visible across tasks, and a header dropdown lists open sessions
  with a button for starting a fresh one.
- The sidebar webview is retained while hidden, so returning to Peridot from
  another Cursor view restores the chat without requiring a full window reload.
- Tool activity now renders as a single-line row with status on the same line,
  and repeated tool calls replace the active row while a disclosure toggle keeps
  the full tool history available.
- Assistant messages render Markdown, show a copy button on hover, and no
  longer display `Finished · undefined` after completion.
- Assistant/tool message boxes were removed, while user prompt bubbles remain
  boxed for contrast.
- The chat title now appears as `Peridot Agent`, the top-left icon is larger,
  the context display is reduced to the donut only, and the empty ready screen
  keeps the composer pinned to the bottom.

## [0.5.10] — 2026-05-21

### Fixed — marketplace publish retries

- VS Code Marketplace and Open VSX publish steps now retry transient timeouts
  and treat duplicate/already-published responses as success. This handles
  cases where a registry accepts the upload but the client times out before
  receiving the final response.

## [0.5.9] — 2026-05-21

### Fixed — universal Open VSX fallback handling

- Universal VSIX publishing to VS Code Marketplace and GitHub Release remains
  required, while the large universal Open VSX upload is now non-blocking if
  the registry disconnects. Platform-specific Open VSX publishes still fail
  the release when they fail.

## [0.5.8] — 2026-05-21

### Fixed — release asset publishing

- GitHub Release asset upload now runs once after all VSIX packages are
  produced, avoiding concurrent release creation conflicts between platform
  publish jobs.
- VS Code extension workflows suppress noisy Node deprecation warnings from
  action internals while using Node 24-compatible artifact actions.

## [0.5.7] — 2026-05-21

### Fixed — release workflow resilience

- VS Code extension release jobs now use Node 24-compatible artifact and
  release actions, removing the GitHub Actions Node 20 deprecation warnings.
- Open VSX publishing now retries transient registry failures before failing,
  and GitHub Release asset upload runs before the Open VSX step so `.vsix`
  files remain attached even if the registry is temporarily unavailable.

## [0.5.6] — 2026-05-21

### Added — universal VSIX fallback and ChatGPT model UX

- Release workflow now publishes a universal `.vsix` in addition to the six
  platform-specific packages. The universal package carries all bundled
  `peridot` binaries under `resources/bin/<target>/`, and the extension
  resolver picks the current host target at runtime. This gives Cursor a
  stable fallback when its Marketplace updater chooses the universal package.
- ChatGPT OAuth setup now resets `auth.primary`, `api.base_url`, and
  `models.main` to a Codex-compatible GPT default (`gpt-5.5`) so an old
  Claude model selection cannot produce a Codex `400 Bad Request`.
- Composer model override becomes a ChatGPT model dropdown when the active
  provider is `openai-oauth`; OpenRouter and other providers keep free-form
  model input.
- Context utilization now renders as a compact donut immediately above the
  composer with exact token counts in the hover tooltip.
- Session layout pins the composer to the bottom of the sidebar.

### Fixed — recovery loop guard

- Error-driven recovery now waits 3 seconds before retrying and aborts after
  3 recoverable errors in one run, preventing provider/configuration failures
  from spinning through the full max-turn budget.

## [0.5.5] — 2026-05-21

### Fixed — Windows login environment

- Peridot child processes spawned by the extension now receive a `HOME`
  fallback from `USERPROFILE` when Cursor's Windows extension host does
  not provide one, fixing ChatGPT login failures with `HOME is required`.
- The bundled CLI now also treats `USERPROFILE` as the user home on
  Windows for auth, config, update-check, cache, and memory paths.

## [0.5.4] — 2026-05-21

### Changed — marketplace icon

- Replaced the extension marketplace and onboarding mascot PNG with the
  updated Peridot pixel mascot.

## [0.5.3] — 2026-05-21

### Fixed — bundled binary and mascot resources

- The sidebar webview now allows bundled `resources/` assets, so the
  onboarding mascot image loads instead of rendering as a broken image.
- `peridot.binaryPath` overrides are validated before use. Stale paths
  from another extension host, such as a Windows Cursor window seeing a
  WSL-only `/home/.../peridot`, are ignored so packaged installs fall
  back to the bundled `resources/peridot.exe`.
- Changing `peridot.binaryPath` now clears the cached binary lookup and
  refreshes status without requiring a Cursor restart.

## [0.5.2] — 2026-05-20

### Added — UI polish pass

- Composer mode / permission / scope selects are normalized across
  platforms with `appearance: none` and a CSS-drawn chevron so the
  native OS chrome stops bleeding into the design.
- `<details>` plan panel uses a custom triangle that rotates 90° when
  open, replacing the missing native arrow.
- Tool / approval / diff code blocks use a lightweight JSON-ish
  tokenizer for subtle key / string / number / boolean coloring.
- Empty state shows the mascot plus an inline `kbd`-styled keyboard
  hint (Enter sends, Shift+Enter newline) so new users see the
  composer convention at a glance.
- Auth flow has a spinner indicator — primary-button gains a busy
  state, and onboarding option cards render a top-right spinner
  while a provider is being configured.
- Send → Stop button now animates the icon swap with a scale-pop
  keyframe and the button presses with a subtle scale-down.
- Queue items display a placeholder when emptied, strip paste
  formatting via `document.execCommand('insertText', …)`, treat
  Escape as cancel-and-restore, and bypass the Enter-saves shortcut
  during IME composition.
- Composer textarea draft, mode / permission / model select picks,
  and focus position now survive a re-render so streaming events
  never reset the operator's pending choice or steal focus.

### Added — onboarding landing + message queue + tool cards

- New onboarding landing screen with three entry points: **Sign in with
  ChatGPT** (OAuth), **OpenRouter API key** (single-key, 75+ models),
  and **Local LLM endpoint** (Ollama / LM Studio / vLLM via the OpenAI
  HTTP API). Each form drives the underlying `peridot env set` /
  `peridot config set` commands so the daemon picks up the new
  provider without a restart. The landing screen appears whenever the
  workspace has no configured auth; once a provider is configured the
  sidebar drops straight into the session view. A "Switch provider"
  button in the session header brings the landing back on demand.
- Composer queues messages typed while the agent is busy. Each queued
  prompt is inline-editable, can be removed, and runs automatically
  with the operator's last-used run options as soon as the current
  turn finishes. A "Clear" link drops the entire queue.
- Tool calls now render as a single collapsing card with a pulsing
  status dot during execution; the matching `tool_finished` event
  fills in the summary in place rather than appending a second card.
- File diffs and approval cards share a unified-diff renderer; the
  file path doubles as an "open in editor" link.
- Pixel-art deer mascot lands as the marketplace icon
  (`resources/peridot-icon.png`, 256×256) and as the activity bar SVG
  (`resources/peridot.svg`, 32×32 vector). Palette mirrors the TUI
  mascot in `crates/peridot-tui/src/mascot/frames.rs`.

### Changed — composer keybindings, layout, errors

- Composer keys swap to the standard chat convention: **Enter sends**,
  **Shift+Enter inserts a newline**. IME composition events bypass the
  send guard so Korean / Japanese candidate selection still works.
- Send button morphs into a stop button while the agent is running; the
  user can keep typing in the textarea (queued messages stack above).
- Sidebar layout caps the content column at 720px and centers it so
  wide sidebars no longer stretch elements awkwardly. Header / context
  / HUD / transcript / queue / composer stack with consistent padding.
- Visual refresh — quieter pills, restrained color accents (peridot
  green for live state, warning yellow for missing auth, error red for
  failures), tighter typography. Tool cards, status lines, error lines
  and chat bubbles each get distinct treatment without resorting to
  emoji icons.
- The Output channel no longer auto-shows when a task starts or fails;
  it's still written to for full diagnostics but stays hidden by
  default. Failure surfaces ship the last few stderr lines into the
  sidebar error block so the operator gets the cause without opening
  the Output panel.

### Fixed — CI chmod path duplication

- The vsce/v0.5.1 release pipeline ran every non-Windows publish job
  inside `extensions/vscode/`, so the previous
  `chmod +x extensions/vscode/resources/peridot` resolved to
  `extensions/vscode/extensions/vscode/...` and aborted. v0.5.2's
  workflow already targets `resources/peridot` (relative).

## [0.5.1] — 2026-05-20

### Fixed — release pipeline retry build

No runtime behaviour changes from v0.5.0. This release exists to recover
from a CI bug in the `vsce/v0.5.0` workflow where `chmod +x` resolved
its target relative to the publish job's `working-directory` and
double-prefixed the path (`extensions/vscode/extensions/vscode/...`),
aborting every non-Windows publish before `npm ci`. The Windows runs
skipped the step entirely (the executable-bit guard was false) and
published v0.5.0 to win32-x64 / win32-arm64 alone. v0.5.1 publishes the
same .vsix payload to all six platforms in one pass; v0.5.0 stays on
the Marketplace as a Windows-only artefact.

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
