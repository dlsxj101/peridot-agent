# Peridot Agent

VS Code panel for [Peridot Agent](https://github.com/dlsxj101/peridot-agent) —
a Rust CLI/TUI autonomous coding agent with multi-LLM committee mode,
native tool calling, and 2-Tier context management.

> **Status**: v0.0.1 is a scaffolding release. The extension installs,
> spawns the bundled `peridot` daemon over JSON-RPC, and round-trips a
> `version` request. The real chat panel, diff viewer, and approval
> UI ship in v0.1.0 once this foundation is verified end-to-end on
> the Marketplace.

## Commands

| Command | Description |
|---|---|
| `Peridot: Hello` | Pops a "extension is alive" toast. |
| `Peridot: Check Daemon Version` | Spawns the bundled daemon, asks `peridot.version`, displays both the daemon and extension versions side-by-side. |

## Configuration

| Setting | Default | Description |
|---|---|---|
| `peridot.binaryPath` | (empty) | Absolute path to a `peridot` binary. Leave empty to use the bundled one (preferred) or fall back to the system PATH. |

## Roadmap

- **v0.1.0** — Chat panel (WebView), `Peridot: Run Task` command, ChatGPT OAuth login.
- **v0.2.0** — FileDiff event → Monaco diff editor; ApprovalRequested → inline approve/deny.
- **v0.3.0** — Skill picker, slash commands, branch picker, multi-session tab bar.

## Source

Extension source lives at
[github.com/dlsxj101/peridot-agent/tree/main/extensions/vscode](https://github.com/dlsxj101/peridot-agent/tree/main/extensions/vscode).
The Rust agent core is in the same repository under `crates/`.
