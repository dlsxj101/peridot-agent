# Peridot Agent

VS Code panel for [Peridot Agent](https://github.com/dlsxj101/peridot-agent) —
a Rust CLI/TUI autonomous coding agent with multi-LLM committee mode,
native tool calling, and 2-Tier context management.

> **Status**: v0.1.x adds the first sidebar chat panel. The extension
> installs, registers its commands reliably in VS Code and Cursor, spawns
> `peridot daemon` over JSON-RPC, round-trips a `version` request, and can
> run a task while streaming daemon events into the Peridot sidebar.

## Commands

| Command | Description |
|---|---|
| `Peridot: Hello` | Pops a "extension is alive" toast. |
| `Peridot: Check Daemon Version` | Spawns the bundled daemon, asks `peridot.version`, displays both the daemon and extension versions side-by-side. |
| `Peridot: Run Task` | Prompts for a task, calls `session.start`, and streams daemon events into the Peridot sidebar. |
| `Peridot: Cancel Current Task` | Sends `session.cancel` for the active daemon session. |

## Sidebar

Open the Peridot Activity Bar item, type a task, and submit it to start a
daemon-backed agent session. Events from the daemon appear in the transcript
as the run progresses.

## Configuration

| Setting | Default | Description |
|---|---|---|
| `peridot.binaryPath` | (empty) | Absolute path to a `peridot` binary. Leave empty to use a bundled binary when present or fall back to the system PATH. |

## Roadmap

- **v0.2.0** — FileDiff event → Monaco diff editor; ApprovalRequested → inline approve/deny.
- **v0.3.0** — ChatGPT OAuth login, skill picker, slash commands, branch picker, multi-session tab bar.

## Source

Extension source lives at
[github.com/dlsxj101/peridot-agent/tree/main/extensions/vscode](https://github.com/dlsxj101/peridot-agent/tree/main/extensions/vscode).
The Rust agent core is in the same repository under `crates/`.
