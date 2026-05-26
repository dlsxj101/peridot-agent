# Peridot Agent

VS Code panel for [Peridot Agent](https://github.com/dlsxj101/peridot-agent) —
a Rust CLI/TUI autonomous coding agent with multi-LLM committee mode,
native tool calling, and 2-Tier context management.

> **Status**: v0.5.18 adds an editor-area settings page (`Peridot: Open
> Settings` + sidebar gear icon) backed by daemon `settings.list /
> save` RPC, Hermes-style auto-skill invocation via `/skill-name`
> slashes, and a daemon-handshake schema-version check. The sidebar
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
| `Peridot: Open Settings` | Opens an editor-area form for `.peridot/config.toml`. Toggle autonomy loops, defaults, committee mode, security, git automation, language, and updates. New sessions started after a save pick up the values automatically; running sessions keep their boot snapshot. Also reachable from the gear icon in the Peridot sidebar title bar. |

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
`file_patch`. The slash picker loads the same command catalog exposed by
the TUI through the daemon `session.command_catalog` RPC, and supported
session-control slashes run through `session.command` so mode, permission,
model, provider, committee, goal control, note, compact, branch, MCP,
TODO, diff, undo, and context results stay aligned with the daemon.
Typing `/branch` opens a picker backed by the current context turns;
selecting a row runs `/branch turn <id>`.

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
bash scripts/install-cursor-remote.sh 0.5.18
```

Or run the same workaround without a checkout:

```bash
curl -fsSL https://raw.githubusercontent.com/dlsxj101/peridot-agent/main/extensions/vscode/scripts/install-cursor-remote.sh \
  | bash -s -- 0.5.18
```

The script downloads the Marketplace VSIX with `curl --compressed`,
validates that the saved file is a decoded ZIP/VSIX, and installs it via
the newest `~/.cursor-server/.../cursor-server` binary. Reload Cursor
after it prints the successful install message.

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

For WSL/Cursor extension development, install extension dependencies first:

```bash
cd extensions/vscode
npm install
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

## Roadmap

- **v0.5.0** — ✅ Bundled `peridot` binary for six targets, sidebar
  webview split into its own esbuild bundle, HUD panel for usage /
  budget / context, inline plan panel, inline unified-diff cards,
  pre-approval diff preview for `file_write` / `file_patch`, and
  cached / reused-daemon status reads.
- **v0.5.18** — ✅ Editor-area settings webview (form for
  `.peridot/config.toml`), Hermes-style `/skill-name` slash skill
  invocation, daemon `peridot.handshake` schema-version check,
  routine phase-transition filtering for a quieter transcript,
  risk-class chip colours on tool rows, and an LLM-authored session
  title with `"No title"` fallback.
- **v0.6.0** — Settings webview polish (in-flight save guard, aria-
  live flash, focus-visible outline on toggles, responsive layout
  below 480px viewports, webview-side i18n for Save / Reload /
  flash strings); per-skill description shown in `skill_list` L0
  disclosure; L2 reference-file tier under
  `.peridot/skills/auto/<name>/references/`; operator-facing
  `peridot skill pin <name>` / `unpin <name>` subcommands. Multi-
  session tab bar and remaining editor parity polish.

## Source

Extension source lives at
[github.com/dlsxj101/peridot-agent/tree/main/extensions/vscode](https://github.com/dlsxj101/peridot-agent/tree/main/extensions/vscode).
The Rust agent core is in the same repository under `crates/`.
