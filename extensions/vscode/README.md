# Peridot Agent

VS Code panel for [Peridot Agent](https://github.com/dlsxj101/peridot-agent) —
a Rust CLI/TUI autonomous coding agent with multi-LLM committee mode,
native tool calling, and 2-Tier context management.

> **Status**: v0.5.0 ships a bundled `peridot` binary for six targets
> (linux-x64/arm64, darwin-x64/arm64, win32-x64/arm64) so installing the
> extension from the Marketplace / Open VSX runs out of the box — no
> separate `cargo build` required. It also splits the sidebar webview
> into its own esbuild bundle and adds a HUD panel for usage / budget /
> 4-Tier context, an inline plan panel for `plan_updated`, inline
> unified-diff rendering for `file_diff` events, a pre-approval diff
> preview for `file_write` / `file_patch`, and a cached / reused-daemon
> status reader.

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
| `peridot.binaryPath` | (empty) | Absolute path to a `peridot` binary. Leave empty to use the bundled binary in the `.vsix` (default for Marketplace / Open VSX installs) or fall back to the system PATH (for sideloaded dev builds without a bundled binary). |

## Local development

Sideloading a `.vsix` you packaged yourself? `npm run package` produces a
universal build with **no** bundled binary — Peridot then falls back to
`peridot` on your PATH. To exercise the bundled-binary path locally:

```bash
cargo build --release -p peridot-cli
cd extensions/vscode
npm run bundle-binary       # copies target/release/peridot into resources/
npm run package             # .vsix now contains the binary
```

`resources/peridot` and `resources/peridot.exe` are gitignored so the
local copy never lands on `main`. The release pipeline drops a
platform-specific binary into the same location before
`vsce package --target`.

## Roadmap

- **v0.5.0** — ✅ Bundled `peridot` binary for six targets, sidebar
  webview split into its own esbuild bundle, HUD panel for usage /
  budget / context, inline plan panel, inline unified-diff cards,
  pre-approval diff preview for `file_write` / `file_patch`, and
  cached / reused-daemon status reads.
- **v0.6.0** — Skill picker, slash commands, branch picker, multi-session
  tab bar. (Requires new daemon RPCs — designed alongside the daemon.)

## Source

Extension source lives at
[github.com/dlsxj101/peridot-agent/tree/main/extensions/vscode](https://github.com/dlsxj101/peridot-agent/tree/main/extensions/vscode).
The Rust agent core is in the same repository under `crates/`.
