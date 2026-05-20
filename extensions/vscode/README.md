# Peridot Agent

VS Code panel for [Peridot Agent](https://github.com/dlsxj101/peridot-agent) —
a Rust CLI/TUI autonomous coding agent with multi-LLM committee mode,
native tool calling, and 2-Tier context management.

> **Status**: v0.4.0 adds the first approval-resume control layer. The extension
> installs, registers its commands reliably in VS Code and Cursor, spawns
> `peridot daemon` over JSON-RPC, round-trips a `version` request, and can
> run a task while streaming daemon events into the Peridot sidebar. The
> sidebar now shows the active workspace, provider, model, auth state, and
> login/refresh actions, plus run options, inline `agent_ask_user` response
> cards, and approve/deny controls for approval-gated tool calls.

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
the active workspace/provider context, renders common daemon events as compact
transcript entries, can answer live `agent_ask_user` prompts, and can resume
approval-paused tool calls from the sidebar.

## Configuration

| Setting | Default | Description |
|---|---|---|
| `peridot.binaryPath` | (empty) | Absolute path to a `peridot` binary. Leave empty to use a bundled binary when present or fall back to the system PATH. |

## Roadmap

- **v0.5.0** — FileDiff event → Monaco diff editor; richer approval diff preview.
- **v0.6.0** — Skill picker, slash commands, branch picker, multi-session tab bar.

## Source

Extension source lives at
[github.com/dlsxj101/peridot-agent/tree/main/extensions/vscode](https://github.com/dlsxj101/peridot-agent/tree/main/extensions/vscode).
The Rust agent core is in the same repository under `crates/`.
