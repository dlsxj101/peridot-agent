# Peridot Agent

VS Code panel for [Peridot Agent](https://github.com/dlsxj101/peridot-agent) —
a Rust CLI/TUI autonomous coding agent with multi-LLM committee mode,
native tool calling, and 2-Tier context management.

> **Status**: v0.5.11 ships platform-specific `.vsix` packages plus a
> universal fallback package for Cursor, ChatGPT OAuth defaults to a
> Codex-compatible GPT model, and the sidebar includes onboarding,
> queued prompts, persistent chat sessions, Markdown answers, single-line
> tool activity, approval/diff cards, usage/budget HUD, an inline plan
> panel, and a compact context donut in the bottom composer.

## Commands

| Command | Description |
|---|---|
| `Peridot: Hello` | Pops a "extension is alive" toast. |
| `Peridot: Check Daemon Version` | Spawns the bundled daemon, asks `peridot.version`, displays both the daemon and extension versions side-by-side. |
| `Peridot: Run Task` | Prompts for a task, calls `session.start`, and streams daemon events into the Peridot sidebar. |
| `Peridot: Cancel Current Task` | Sends `session.cancel` for the active daemon session. |
| `Peridot: Login with ChatGPT` | Runs `peridot login openai-oauth` from the active workspace. |
| `Peridot: Refresh Status` | Refreshes daemon workspace/provider/model/auth status. |

## First run

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
`file_patch`. Type at the composer — Enter sends, Shift+Enter inserts
a newline. Sending while a task is in flight queues the message; the
queue UI lets you edit, remove, or clear individual entries before
they auto-run. Follow-up prompts continue the active session until you
run `/clear` or open a new session from the dropdown.

## Configuration

| Setting | Default | Description |
|---|---|---|
| `peridot.binaryPath` | (empty) | Absolute path to a `peridot` binary. Leave empty to use the bundled binary in the `.vsix` (default for Marketplace / Open VSX installs) or fall back to the system PATH (for sideloaded dev builds without a bundled binary). |

## Local development

Sideloading a `.vsix` you packaged yourself? `npm run package` produces a
universal build. Unless you stage binaries under `resources/bin/<target>/`
first, Peridot falls back to `peridot` on your PATH. To exercise a
single-platform bundled-binary path locally:

```bash
cargo build --release -p peridot-cli
cd extensions/vscode
npm run bundle-binary       # copies target/release/peridot into resources/
npm run package             # .vsix now contains the binary
```

`resources/peridot`, `resources/peridot.exe`, and `resources/bin/` are
gitignored so local binary copies never land on `main`. The release
pipeline drops platform-specific binaries into `resources/peridot[.exe]`
for target packages and into `resources/bin/<target>/` for the universal
fallback package.

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
