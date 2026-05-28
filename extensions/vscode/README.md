# Peridot Agent

VS Code panel for [Peridot Agent](https://github.com/dlsxj101/peridot-agent) —
a Rust CLI/TUI autonomous coding agent with multi-LLM committee mode,
native tool calling, and 2-Tier context management.

> **Status**: v0.5.20 adds a bottom run-timing footer with a live
> Peridot gem loader, moves assistant copy actions into message
> footers, and restores the `agent_ask_user` `Other` free-form answer
> path in the sidebar. The sidebar
> includes onboarding, queued prompts, persistent chat sessions,
> Markdown answers, single-line tool activity (with risk-class chip
> colouring), approval/diff cards, usage/budget HUD, an inline plan
> panel, a compact context donut, daemon-backed slash commands, and a
> structured `/branch` picker.

## Commands

| Command | Description |
|---|---|
| `Peridot: Hello` | Pops a "extension is alive" toast. |
| `Peridot: Check Daemon Version` | Spawns the bundled daemon, asks `peridot.version`, displays both the daemon and extension versions side-by-side. |
| `Peridot: Run Task` | Prompts for a task, calls `session.start`, and streams daemon events into the Peridot sidebar. |
| `Peridot: Cancel Current Task` | Sends `session.cancel` for the active daemon session. |
| `Peridot: Login with ChatGPT` | Runs `peridot login openai-oauth` from the active workspace. |
| `Peridot: Refresh Status` | Refreshes daemon workspace/provider/model/auth status. |
| `Peridot: Set Execution Mode` | Picks Execute, Plan, or Goal and runs the matching shared slash command. |
| `Peridot: Set Permission Mode` | Picks Auto, Safe, or Yolo and runs the matching shared slash command. |
| `Peridot: Set Reasoning Effort` | Picks a reasoning tier and runs `/reasoning <tier>`. |
| `Peridot: Switch Runtime Provider` | Picks a provider id and runs `/provider <id>` for this session. |
| `Peridot: Set Runtime Model` | Prompts for a model override and runs `/model <name>`. |
| `Peridot: Set Committee Mode` | Picks Off, Planner, or Full and runs `/committee <mode>`. |
| `Peridot: Show Workspace Code Map` | Runs the shared `/codemap` scan and appends public symbols plus TODO markers to the sidebar transcript, refreshing stale indexes first. |
| `Peridot: Show Workspace Code Map Status` | Runs `/codemap status` to show whether the persisted code map index is missing, fresh, or stale. |
| `Peridot: Refresh Workspace Code Map Index` | Rebuilds `.peridot/codemap.json` through `/codemap refresh`. |
| `Peridot: Search Workspace Code Map` | Prompts for a query and runs `/codemap find <query>` against the persisted index. |
| `Peridot: Locate Workspace Symbol` | Prompts for a symbol, runs `/codemap locate <symbol>`, and opens the first indexed definition. |
| `Peridot: Outline Current File` | Runs `/codemap outline <path>` for the active editor file and renders indexed symbols. |
| `Peridot: Find Workspace Symbol References` | Prompts for a symbol and runs `/codemap refs <symbol>` to render matching reference lines. |
| `Peridot: Show Skills` | Runs `/skills` and renders active stored skills with copy/use/pin/archive actions. |
| `Peridot: Show Archived Skills` | Runs `/skills archived` and renders archived skills with show/restore actions. |
| `Peridot: Search Skills` | Prompts for a query and runs `/skills search <query>` against active stored skills. |
| `Peridot: Search Archived Skills` | Prompts for a query and runs `/skills archived <query>` against archived stored skills. |
| `Peridot: Attach File to Session` | Picks a workspace file, runs `/attach <path>`, and renders a compact attachment block with open/copy actions. |
| `Peridot: Show Session Attachments` | Runs `/attachments` and renders files already loaded into the current session context, with open/copy/detach actions. |
| `Peridot: Show Workspace TODOs` | Runs `/todos` to scan the workspace for TODO, FIXME, HACK, XXX, and BUG markers. |
| `Peridot: Show Context Top` | Runs `/context top` for the active session and renders the largest context entries plus source token totals. |
| `Peridot: Show Working Tree Diff` | Runs `/diff` and renders the current working tree diff in the sidebar transcript. |
| `Peridot: Compact Context` | Runs `/compact` to queue context compaction for the active daemon session's next turn. |
| `Peridot: Rewind Last Exchange` | Runs `/rewind`, removes the visible last exchange, and restores the last prompt in the composer when available. |
| `Peridot: Undo Last Change` | Confirms, then runs `/undo` to restore the latest Peridot file checkpoint. |
| `Peridot: Show Branch Turns` | Runs `/branch` and renders the current session turn picker for forking from an earlier context turn. |
| `Peridot: Show Branch Snapshots` | Runs `/branch list` and renders saved context snapshots from `.peridot/branches`. |
| `Peridot: Save Branch Snapshot` | Prompts for a snapshot name and runs `/branch save <name>` against the active session context. |
| `Peridot: Restore Branch Snapshot` | Picks a saved snapshot and runs `/branch restore <name>` against the active session. |
| `Peridot: Fork Branch at Turn` | Prompts for a context turn id and runs `/branch turn <id>`. |
| `Peridot: Show Branch Tree` | Runs `/branch tree` to show abandoned conversation limbs from the active session journal. |
| `Peridot: Switch Branch Limb` | Prompts for a branch limb index from `/branch tree` and runs `/branch switch <index>`. |
| `Peridot: Show MCP Servers` | Runs `/mcp list` and renders configured MCP server names, transports, and details. |
| `Peridot: Add MCP Server` | Prompts for name, transport, and command/URL, then runs `/mcp add <name> <transport> <target>`. |
| `Peridot: Test MCP Server` | Picks a configured MCP server, runs `/mcp test <name>`, and renders the connectivity result. |
| `Peridot: Remove MCP Server` | Picks a configured MCP server, asks for confirmation, and runs `/mcp remove <name>`. |
| `Peridot: Add Session Note` | Prompts for an operator note and runs `/note <text>` against the active session. |
| `Peridot: Show Session Notes` | Runs `/notes`, optionally limited to recent entries, and renders the active session's note list. |
| `Peridot: Clear Session Notes` | Asks for confirmation and runs `/notes clear` for the active session. |
| `Peridot: New Session` | Runs `/session new [task]` to create an authoritative persisted session and optionally start its first task. |
| `Peridot: Switch Session` | Picks a persisted session, runs `/session switch <id>`, and makes it active in the sidebar. |
| `Peridot: Close Session` | Picks a persisted session, asks for confirmation, runs `/session close <id>`, and refreshes the session list. |
| `Peridot: Show Session Count` | Runs `/session count` and renders the persisted lifecycle totals. |
| `Peridot: Show Session Details` | Picks a persisted session and runs `/session show <id>` to render lifecycle, workspace, usage, and notes metadata. |
| `Peridot: Locate Session Directory` | Picks a persisted session and runs `/session locate <id>` to render its `.peridot/sessions/<id>` directory. |
| `Peridot: Resume Session` | Picks a persisted session, runs `/session resume <id>`, and starts the generated continuation task. |
| `Peridot: Rename Session` | Picks a persisted session and runs `/session rename <id> <title>`, then refreshes the session list. |
| `Peridot: Delete Session` | Picks a persisted session, asks for confirmation, runs `/session delete <id>`, and refreshes the session list. |
| `Peridot: Show Sessions` | Runs `/session list`, optionally filtered by lifecycle status, and refreshes local session cards only for full inventory results. |
| `Peridot: Search Sessions` | Prompts for a query, runs `/session search <query>`, and renders persisted transcript hits. |
| `Peridot: Prune Sessions` | Previews `/session prune` with status and age filters, then asks for confirmation before removing matching persisted sessions. |
| `Peridot: Replay Session Timeline` | Picks a persisted session and runs `/session replay`, optionally limited to recent timeline entries. |
| `Peridot: Export Session Artifacts` | Picks an active or persisted session, exports its attachments, notes, and replay timeline to a portable directory, then opens it. |
| `Peridot: Import Session Artifacts` | Picks a portable session directory and runs `/session import <dir>` with optional id and overwrite controls. |
| `Peridot: Show GitHub PR Status` | Runs `gh pr status` from the workspace and appends the result to the sidebar transcript. |
| `Peridot: Ship Changes to PR` | Previews `peridot ship --dry-run`, asks for confirmation, then commits, pushes, and optionally opens a PR. |
| `Peridot: Merge GitHub PR` | Prompts for PR/merge strategy, asks for confirmation, then runs `gh pr merge`. |
| `Peridot: Open Settings` | Opens an editor-area form for `.peridot/config.toml`. Toggle autonomy loops, defaults, committee mode, security, git automation, language, and updates. New sessions started after a save pick up the values automatically; running sessions keep their boot snapshot. Also reachable from the gear icon in the Peridot sidebar title bar. |

## First run

The extension contributes a **Get Started with Peridot** walkthrough in
VS Code's welcome flow. It links the same surfaces used day to day:
open the Peridot sidebar, connect a provider, review workspace settings,
then run a first task.

When you open the Peridot Activity Bar with no provider configured, you
land on a three-button onboarding screen:

- **Sign in with ChatGPT** — OAuth via your ChatGPT account
  (`peridot login openai-oauth` under the hood).
- **OpenRouter API key** — one key, 75+ models. Stored in Peridot's
  local env store, never your shell rc.
- **Local LLM endpoint** — point at any OpenAI-compatible HTTP API
  (Ollama, LM Studio, vLLM, …).

Already configured? The session view opens directly. Use the "Switch
provider" button in the session header to come back.

## Sidebar

Once a provider is live, the sidebar shows workspace / provider /
model / auth context, a session dropdown, a HUD for token usage and
cost / turn budget, an inline plan panel that follows `plan_updated`,
a compact context donut in the composer options row, a chat-style
Markdown transcript with single-line tool activity and inline unified
diffs, plus approve/deny controls with a diff preview for `file_write` /
`file_patch`. The slash picker loads the same command catalog exposed by
the TUI through the daemon `session.command_catalog` RPC, and supported
session-control slashes run through `session.command` so mode, permission,
model, provider, committee, note, compact, branch, MCP, TODO, codemap,
info, cost, plan show, goal control, session save, session count,
session rename/delete, rewind, export, diff, undo, and context results stay
aligned with the daemon.
Typing `/branch` opens a picker backed by the current context turns;
selecting a row runs `/branch turn <id>`. The session header also exposes
artifact export for the active session's attachments, notes, and replay
timeline.

Type at the composer — Enter sends, Shift+Enter inserts a newline.
Sending while a task is in flight queues the message; the queue UI lets
you edit, remove, or clear individual entries before they auto-run.
Follow-up prompts continue the active session until you run `/clear` or
open a new session from the dropdown.

## Configuration

| Setting | Default | Description |
|---|---|---|
| `peridot.binaryPath` | (empty) | Absolute path to a `peridot` binary. Leave empty to use the bundled binary in the `.vsix` (default for Marketplace / Open VSX installs) or fall back to the system PATH (for sideloaded dev builds without a bundled binary). |

## Cursor remote install workaround

Some Cursor remote-server builds fail while updating Marketplace
extensions whose VSIX response is transported with
`Content-Encoding: gzip`. The Marketplace package is valid after HTTP
decoding, but Cursor may cache the gzip transport body and then try to
open it as a ZIP, reporting:

```text
End of central directory record signature not found. Either not a zip file, or file is truncated.
```

Install the decoded VSIX directly on the remote host:

```bash
cd extensions/vscode
bash scripts/install-cursor-remote.sh 0.5.20
```

Or run the same workaround without a checkout:

```bash
curl -fsSL https://raw.githubusercontent.com/dlsxj101/peridot-agent/main/extensions/vscode/scripts/install-cursor-remote.sh \
  | bash -s -- 0.5.20
```

The script downloads the Marketplace VSIX with `curl --compressed`,
validates that the saved file is a decoded ZIP/VSIX, and installs it via
the newest `~/.cursor-server/.../cursor-server` binary. Reload Cursor
after it prints the successful install message.

## Local development

Sideloading a `.vsix` you packaged yourself? `npm run package` produces a
universal build and runs the extension unit tests through VSCE's
`vscode:prepublish` hook. Unless you stage binaries under
`resources/bin/<target>/` first, Peridot falls back to `peridot` on your
PATH. To exercise a single-platform bundled-binary path locally:

```bash
cargo build --release -p peridot-cli
cd extensions/vscode
npm run bundle-binary       # copies target/release/peridot into resources/
npm run package             # .vsix now contains the binary
```

For WSL/Cursor extension development, install extension dependencies first:

```bash
cd extensions/vscode
npm install
npm test
```

Then use the VS Code/Cursor launch configuration **Peridot: Run Extension
with bundled CLI**. Its prelaunch task typechecks the extension, builds the
release `peridot-cli`, copies the binary into `extensions/vscode/resources/`,
and smoke-checks the bundled CLI with `resources/peridot --version` before
opening the Extension Development Host.

`resources/peridot`, `resources/peridot.exe`, and `resources/bin/` are
gitignored so local binary copies never land on `main`. The release
pipeline drops platform-specific binaries into `resources/peridot[.exe]`
for target packages and into `resources/bin/<target>/` for the universal
fallback package.

## Release

Extension releases use `vsce/v<version>` tags so they do not collide with
Rust CLI `v*` releases. Before publishing, update
`extensions/vscode/package.json`, then push a matching tag:

```bash
git tag vsce/v0.5.20
git push origin vsce/v0.5.20
```

The release workflow verifies that the tag matches the extension package
version, builds six bundled CLI binaries, publishes platform-specific
VSIX packages to VS Code Marketplace and Open VSX, publishes the universal
fallback VSIX, and attaches all VSIX assets to the GitHub Release. The
workflow requires `VSCE_PAT` and `OVSX_PAT` repository secrets.

## Roadmap

- **v0.5.0** — ✅ Bundled `peridot` binary for six targets, sidebar
  webview split into its own esbuild bundle, HUD panel for usage /
  budget / context, inline plan panel, inline unified-diff cards,
  pre-approval diff preview for `file_write` / `file_patch`, and
  cached / reused-daemon status reads.
- **v0.5.20** — ✅ `agent_ask_user` single-select and multi-select
  prompts include an `Other` free-form answer in the sidebar; long
  tasks show live and final elapsed time in a bottom footer.
- **v0.5.19** — ✅ Request-context donut breakdown matching the next
  daemon/provider request, routine phase events hidden from the
  transcript, user-facing `checking` phase wording, and tool-result
  mutation markers.
- **v0.5.18** — ✅ Editor-area settings webview (form for
  `.peridot/config.toml`), Hermes-style `/skill-name` slash skill
  invocation, daemon `peridot.handshake` schema-version check,
  routine phase-transition filtering for a quieter transcript,
  risk-class chip colours on tool rows, and an LLM-authored session
  title with `"No title"` fallback.
- **v0.6.x+** — ✅ Settings webview polish, Hermes-style skill
  directories, skill pin/archive/restore/detail/search surfaces,
  daemon-backed session/slash parity, attachment inventory/lifecycle,
  session artifact export, stale worktree/session reconciliation, and
  shared autocomplete/help metadata across TUI and VS Code.

## Source

Extension source lives at
[github.com/dlsxj101/peridot-agent/tree/main/extensions/vscode](https://github.com/dlsxj101/peridot-agent/tree/main/extensions/vscode).
The Rust agent core is in the same repository under `crates/`.
