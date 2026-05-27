# Peridot Agent — Extension Changelog

## Unreleased

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
