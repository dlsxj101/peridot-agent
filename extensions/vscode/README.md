# Peridot Agent

VS Code panel for [Peridot Agent](https://github.com/dlsxj101/peridot-agent) —
a Rust CLI/TUI autonomous coding agent with multi-LLM committee mode,
native tool calling, and 2-Tier context management.

> **Status**: v0.5.0 splits the sidebar webview into its own bundle
> (esbuild) and adds a HUD panel for usage / budget / 4-Tier context, an
> inline plan panel for `plan_updated`, inline unified-diff rendering for
> `file_diff` events, and a pre-approval diff preview for
> `file_write` / `file_patch`. Daemon status reads are now cached and
> reuse the active session's RPC channel instead of respawning the
> daemon. The extension still installs cleanly in VS Code and Cursor,
> spawns `peridot daemon` over JSON-RPC, runs a task end-to-end, and
> renders `agent_ask_user` / approval cards inline.

## Commands

| Command | Description |
|---|---|
| `Peridot: Hello` | Pops a "extension is alive" toast. |
| `Peridot: Check Daemon Version` | Spawns the bundled daemon, asks `peridot.version`, displays both the daemon and extension versions side-by-side. |
| `Peridot: Run Task` | Prompts for a task, calls `session.start`, and streams daemon events into the Peridot sidebar. |
| `Peridot: Cancel Current Task` | Sends `session.cancel` for the active daemon session. |
| `Peridot: Login with ChatGPT` | Runs `peridot login openai-oauth` from the active workspace. |
| `Peridot: Refresh Status` | Refreshes daemon workspace/provider/model/auth status. |

## Sidebar

Open the Peridot Activity Bar item, choose mode/permission/model options, type
a task, and submit it to start a daemon-backed agent session. The sidebar shows
the active workspace/provider context, a live HUD with token usage, cost /
turn budget, and 4-Tier context utilization, an inline plan panel that follows
`plan_updated`, a compact transcript with collapsed tool cards, answerable
`agent_ask_user` cards, and approve/deny controls for approval-gated tool
calls — including an inline unified diff preview for `file_write` /
`file_patch` so you can see the change before deciding.

## Configuration

| Setting | Default | Description |
|---|---|---|
| `peridot.binaryPath` | (empty) | Absolute path to a `peridot` binary. Leave empty to use a bundled binary when present or fall back to the system PATH. |

## Roadmap

- **v0.5.0** — ✅ Sidebar webview split into its own esbuild bundle, HUD
  panel for usage / budget / context, inline plan panel, inline
  unified-diff cards, pre-approval diff preview for `file_write` /
  `file_patch`, and cached / reused-daemon status reads.
- **v0.6.0** — Skill picker, slash commands, branch picker, multi-session
  tab bar. (Requires new daemon RPCs — designed alongside the daemon.)

## Source

Extension source lives at
[github.com/dlsxj101/peridot-agent/tree/main/extensions/vscode](https://github.com/dlsxj101/peridot-agent/tree/main/extensions/vscode).
The Rust agent core is in the same repository under `crates/`.
