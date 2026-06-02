# Peridot Agent — User Guide

Peridot is a Rust CLI/TUI autonomous coding agent. It pairs a Manus-style
agent harness (deterministic loop, budget guardrails, verification,
memory) with a Claude Code / Codex-style interface (interactive TUI, slash
commands, session resume, and a VS Code extension).

This guide is task-oriented. For architecture and design rationale, read
[`PERIDOT_SPEC_v1.md`](../PERIDOT_SPEC_v1.md). For contributing, see
[`CONTRIBUTING.md`](../CONTRIBUTING.md).

---

## 1. Install

```bash
# One-line install (prebuilt binary, no Rust required)
curl -fsSL https://raw.githubusercontent.com/dlsxj101/peridot-agent/main/install.sh | bash

# Or build from source (contributors; needs Rust >= 1.95)
git clone https://github.com/dlsxj101/peridot-agent
cd peridot-agent
cargo build --release -p peridot-cli
```

The binary is `peridot`; the installer also creates a `peri` alias. Verify:

```bash
peridot version            # short
peridot version --detailed # + target triple / rustc fingerprint
```

Keep it current:

```bash
peridot update --check   # report without installing
peridot update           # install latest release
peridot update --force   # reinstall even if up to date
```

## 2. First run

Authenticate a provider, then start a task. Peridot supports Anthropic
(Claude), OpenAI (API key or ChatGPT OAuth), and OpenRouter.

```bash
# Anthropic (recommended): export the key, then it's auto-detected
export ANTHROPIC_API_KEY=sk-ant-...

# OpenAI API key / OpenRouter / ChatGPT OAuth
peridot login openai-api
peridot login openrouter-api
peridot login openai-oauth     # PKCE browser flow for a ChatGPT subscription

peridot logout <provider>      # remove stored credentials
```

Confirm everything is wired up:

```bash
peridot doctor   # validates config, provider auth, MCP servers,
                 # AGENTS metadata, and permissions. Exit 0 = healthy.
```

Run your first task (launches the interactive TUI by default):

```bash
peridot run "add a --json flag to the export command"
```

## 3. Execution modes

Peridot has three modes (spec §2):

| Mode | Command | Behavior |
|------|---------|----------|
| **Plan** | `peridot plan "<task>"` | Read-only. Analyzes and proposes a plan; never edits files. |
| **Execute** | `peridot run "<task>"` | Interactive editing with approvals (default). |
| **Goal** | `peridot goal "<objective>"` | Durable, long-running objective with a Goal Checker loop. |

Inside the TUI you can switch modes and behavior live with slash commands
(`/plan`, `/execute`, `/goal`).

## 4. The interactive TUI

The TUI streams the agent's thinking, tool calls, results, plan/goal
state, token usage, and cost while the run continues. Key surfaces:

- **Transcript** — the running conversation and tool activity.
- **Side panel** — plan/goal steps, MCP status, AGENTS rules, codemap
  freshness, usage metrics.
- **Approval panel** — gated tool calls; for diffs, use ←/→ to move
  between hunks and Tab/Space to accept/reject individual hunks.
- **Ask-user panel** — the agent's clarifying questions.
- **Tab bar** — the current foreground session (multi-session aware).

UI vocabulary uses five markers only — `▸ ◆ ❯ ✔/✘ ⚠` — and supports
English and Korean (`/lang`).

### Headless / scripting

For CI or piping, run non-interactively:

```bash
peridot run "<task>" --headless --output json
```

Combine with `--mock-response-file <file.jsonl>` to exercise the full
parse-and-execute loop deterministically without any API calls.

## 5. Slash commands (TUI / VS Code composer)

Slash commands route through the shared daemon, so TUI and the VS Code
extension behave identically. Type `/help` for the full, surface-filtered
list. Highlights:

- **Session**: `/session list|count|save|switch|rename|delete|close|new`,
  `/clear`, `/rewind`.
- **Plan & goal**: `/plan show`, `/goal pause|resume|clear|status`,
  `/execute`.
- **Codemap**: `/codemap [status|refresh|find|locate|outline|refs]`,
  `/todos`.
- **Skills**: `/skills [list|search|show|use|pin|unpin|archive|restore|archived]`,
  or invoke a stored skill directly with `/skill-name`.
- **Attachments**: `/attach <path>`, `/attachments`, `/detach <path>`,
  `@file` mentions in the composer.
- **Branching**: `/branch save|restore|list|tree|turn|switch` (file-based
  and turn-level DAG branching).
- **Inspection**: `/info`, `/cost`, `/context`, `/status`.
- **Runtime**: `/provider`, `/model`, `/reasoning`, `/think`, `/fast`,
  `/autofix`, `/committee off|planner|full`, `/subagent`.
- **Export & notes**: `/export [attachments|notes|timeline|full]`,
  `/note <text>`, `/notes [last N]`.

Most commands offer argument autocomplete (provider ids, model names,
session ids, skill names, snapshot names, codemap subcommands, etc.).

## 6. Sessions

Sessions persist under `.peridot/sessions/<id>/` and survive restarts and
crashes (an unclean shutdown downgrades a `Running` session to
`Suspended`).

```bash
peridot session save <name> "<note>"   # snapshot the current state
peridot session list                    # list persisted sessions
peridot session resume <name>           # continue a saved/suspended session
```

Multi-session: background runs get isolated git worktrees; spawn
sub-agents with `/fork`, `/teammate`, `/worktree`. Foreground swap is
`Ctrl+T` / `Ctrl+W`.

## 7. Configuration

Config is TOML at `~/.peridot/config.toml` (global) or
`.peridot/config.toml` (project; takes precedence).

```bash
peridot config init     # scaffold a project config
peridot setting         # interactive settings screen (saves on `s`)
peridot setup           # scaffold project-local Peridot files
peridot env <sub>        # manage Peridot's user-local env var store
```

Key knobs (spec §16): provider/auth, single `model` knob, permission
level (`safe`/`auto`), reasoning tier, service tier, budget caps,
`[auto_fix].max_attempts`, MCP servers, and hooks.

Multimodal vision (`[vision]`): `/attach`-ed images are sent to
vision-capable models as image blocks and automatically stripped to a
text placeholder for text-only models. `[vision].enabled` (default `true`)
turns image sending off entirely; `[vision].max_image_bytes` (default
`5242880`, i.e. 5 MiB) caps how large an image may be before it stays
placeholder-only.

## 8. Permissions & safety

Peridot enforces a layered safety model (spec §19):

- **Permission levels** — `safe` requires approval for risky actions;
  `auto` widens what runs without prompting.
- **Path sandboxing** — file tools are bounded by `AGENTS.md` rules.
- **Command blocklists** — dangerous shell commands are gated.
- **Prompt-injection defense** — external/untrusted content is tagged.
- Dangerous commands, dependency installs, publishing, force pushes, and
  destructive git operations always require explicit approval.

## 9. MCP servers

Peridot is an MCP client (stdio and HTTP):

```bash
peridot mcp list
peridot mcp add <name> <stdio|http> <command-or-url>
peridot mcp test <name>
peridot mcp remove <name>
```

Live MCP status appears in the TUI side panel and the VS Code context
strip.

## 10. Git & GitHub workflow

Per-step git is available through the agent's `git_*` tools. To go from
local changes to an open PR in one shot:

```bash
peridot ship --dry-run   # preview branch → commit → push → PR
peridot ship             # execute (PR creation gated by permission level)
```

## 11. Verification & auto-fix

```bash
peridot verify --output json
```

The agent runs a deterministic verification pipeline (fmt/clippy/test or
the project's configured checks). On failure it can enter an auto-fix loop
(verify → fix → re-verify) bounded by a circuit breaker; toggle with
`/autofix on|off|<N>`.

## 12. Memory, skills, and AGENTS.md

- **Memory** — a SQLite store keeps session summaries and a curated skill
  library; an LLM Curator auto-archives stale entries (30/90-day rules).
- **Skills** — reusable instruction snippets in `.peridot/skills`,
  discoverable via `/skills` and invocable via `/skill-name`.
- **AGENTS.md** — the project instruction file. It hot-reloads mid-run;
  inspect the active rule count in the TUI side panel / VS Code context
  strip. Manage with `peridot agents show`.

## 13. VS Code extension

The extension (`extensions/vscode`) speaks JSON-RPC to `peridot daemon`
and mirrors the TUI: sidebar transcript, composer with slash autocomplete
and `@file` mentions, image paste/drop attachments, approval and ask-user
prompts, a live usage/budget dock, and command-palette entries for
sessions, skills, codemap, MCP, branches, and ship.

---

## Troubleshooting

- **`rustc 1.94.x is not supported` when building from source** — the
  workspace MSRV is 1.95. Install it:
  `rustup toolchain install 1.95.0` (or newer) and ensure your default
  `stable` is ≥ 1.95.
- **`peridot doctor` reports an auth failure** — re-run `peridot login
  <provider>` or export the provider's API key env var.
- **A run stalls waiting for approval** — answer the approval/ask-user
  panel in the TUI, or `interaction.respond` from the editor client.
