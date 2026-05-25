# Extension Roadmap — v0.6.x

Where the VS Code extension goes next, ordered by impact × effort.
Drawn from three signals in v0.8.11 / extension-0.5.17:

- **UX audit** (settings panel review, 12 findings)
- **Hermes comparison** (skill system gaps vs the reference impl)
- **E2E live test** (Java + Vue scaffold with real LLM)

Every item links back to a concrete file:line, an estimated effort,
and an "if we skip it" risk so the team can drop something without
losing context.

---

## Tier S — settings page UX fixes (ship within v0.5.18)

These are accessibility / data-loss regressions introduced by the
new settings webview. Cheap fixes; mostly within
`extensions/vscode/webview/settings.{ts,css}`.

| # | Finding | Effort | Risk if skipped |
|---|---|---|---|
| S1 | Toggle has no visible focus indicator (`settings.css:118-152`) | 15 min | Keyboard users can't tell where they are |
| S2 | Toggle is a nested `<label>` anti-pattern, no `role="switch" / aria-checked` (`settings.ts:199-213`) | 30 min | Screen readers announce inconsistently |
| S3 | Flash region missing `aria-live`; save errors auto-dismiss in 3.5s (`settings.ts:275-286`) | 15 min | SR users + glance-away sighted users miss failures |
| S4 | No in-flight guard on Save — rapid double-click sends two RPCs (`settings.ts:150-152`) | 20 min | Last-write-wins races; not corrupting, but confusing |
| S5 | Reload silently discards unsaved edits (`settings.ts:156-158`) | 30 min | Data loss for users who don't know |
| S6 | Empty number-input snaps to min without showing the new value (`settings.ts:259-261`) | 15 min | `1` typed in a min=10 field silently becomes 10 on save |

**Bundle as one PR**. ~2 hours total. Test via `qa-test/webview-jsdom.mjs`
(already exists at `/tmp/peridot-qa-*/harness/`) by:

1. Adding `dispatchEvent(new KeyboardEvent('keydown', {key:'Tab'}))`
   to simulate focus traversal and `assert(document.activeElement)`.
2. Asserting `flash.getAttribute('aria-live') === 'polite'`.
3. Dispatching two `click` events on Save in quick succession and
   confirming exactly one `postMessage` fires.

---

## Tier A — Hermes parity (ship within v0.6.0)

Gaps the comparison agent flagged between Peridot's auto-skill
system and Hermes. We covered the foundations (description /
pinned, LLM body, slash invocation, snapshot) in v0.8.11; these are
the polish items that close out the comparison.

### A1. L2 reference-file tier
Currently `.peridot/skills/auto/<name>.md` is a single file. Hermes
allows `<name>/SKILL.md + references/*.md + templates/*` and exposes
the reference list at `skill_view` time.

- **Where**: `crates/peridot-tools/src/tools/skill.rs:46`
  (`skill_view`), `crates/peridot-cli/src/run_state.rs:save_auto_skill`
- **Effort**: ~3 hours
- **What**: detect a `<name>/SKILL.md` layout in addition to
  `<name>.md`, add a `skill_view_ref(name, ref_path)` tool, surface
  reference filenames in the L1 `skill_view` response

### A2. Description column shown in `skill_list`
The column exists (v0.8.11 schema migration) but `skill_list` still
returns just `name + first body line + idle days`. Hermes shows the
frontmatter `description` so the model picks better.

- **Where**: `crates/peridot-tools/src/tools/skill.rs::skill_list`
- **Effort**: ~30 min
- **What**: include `stored.description` in the L0 response;
  truncate to 80 chars

### A3. `peridot skill pin <name>` / `unpin <name>` subcommands
The `set_skill_pinned` API exists but there's no operator entry
point. CLI subcommand is the missing surface.

- **Where**: `crates/peridot-cli/src/commands/skills.rs` (new file)
- **Effort**: ~1 hour
- **What**: `peridot skill {list,pin,unpin,view,delete}` mirroring
  the existing `peridot session ...` family; expose via `gh` /
  `peridot --help`

### A4. 2-hour idle window for Curator
Hermes only fires the Curator after "7+ days since last run AND 2+
hours of agent idle." Peridot only checks the 7-day floor, so the
Curator can spawn mid-session if the user reopens a project after a
quiet week.

- **Where**: `crates/peridot-cli/src/main.rs:maybe_run_idle_curator`
- **Effort**: ~30 min
- **What**: read `last_activity_unix` and require `now - last_activity
  >= 2 * 3600` in addition to the existing 7-day gap; better: hook
  the spawn to `session_end` instead of pre-command

### A5. N-gram reflection filter for noise
N-gram promotion (Peridot-only extension) can promote `file_read |
file_read | file_read` as a "skill." The LLM reflection gate filters
most, but a deterministic pre-filter would catch repeating-pattern
noise before the LLM call.

- **Where**: `crates/peridot-cli/src/curator.rs:run_ngram_reflection`
- **Effort**: ~1 hour
- **What**: drop n-grams where all tools are identical or where the
  distinct-tool count is 1 — these are never genuinely reusable
  workflows

---

## Tier B — Editor surface polish (v0.6.x)

Things that aren't broken but make the extension feel rough next to
Cursor / Continue.

### B1. Settings page i18n (webview strings)
The Rust-side settings labels are bilingual but the webview's own
chrome (`"Save"`, `"Reload from disk"`, `"Saved to ..."`, `"Couldn't
load settings"`, `"Loading settings…"`) is hardcoded English.

- **Where**: `extensions/vscode/webview/settings.ts` (and
  `extension.ts:openSettings` for the panel title)
- **Effort**: ~1 hour
- **What**: pass the active locale into the `load` postMessage,
  switch chrome strings via a small lookup. Or use
  `vscode.l10n.t(...)` on the host side and inject translated
  strings into the webview's HTML template.

### B2. Settings page responsive layout
Below ~480px viewports (sidebar-launched panel) the 220px control
column collapses the label cell. The panel currently opens in the
editor area where this rarely happens, but a future "settings in
sidebar" toggle would hit it.

- **Where**: `extensions/vscode/webview/settings.css:60-67`
- **Effort**: ~15 min
- **What**: `@media (max-width: 480px)` rule stacking label / control
  vertically

### B3. Sticky footer separator
The Save/Reload bar visually merges with the last row on scroll.

- **Where**: `extensions/vscode/webview/settings.css:154-162`
- **Effort**: ~5 min
- **What**: top border + small shadow

### B4. Phase / context summary as header chip
Instead of (or in addition to) the transcript status rows, render
the current `AgentPhase` as a chip in the sidebar header. Once
that exists, `phase_changed` transitions never need to enter the
transcript at all.

- **Where**: `extensions/vscode/src/sidebar.ts:resolveWebviewView`
  + `webview/index.ts` header renderer
- **Effort**: ~1.5 hours
- **What**: small chip pill, colour-coded by phase (gray = Planning
  / Executing / Verifying, amber = Recovering, blue = Delegating,
  green = Done)

### B5. Sidebar settings entry-point discoverability
The gear icon is a `view/title` menu item — many VS Code users
never look at the title bar. A secondary entry from the composer's
overflow menu or onboarding screen would help.

- **Where**: `extensions/vscode/webview/index.ts` composer options
- **Effort**: ~30 min
- **What**: small "⚙ Settings" link in the session header, calling
  `vscode.commands.executeCommand('peridot.openSettings')`

---

## Tier C — Bigger bets (v0.7.x)

Cost more, deliver less per hour, but worth tracking so the team
knows where to invest when polish budget is exhausted.

### C1. Marketplace + Open VSX release pipeline
The repo bundles platform-specific binaries but the actual `vsce
publish` flow isn't automated yet. Manual release ≈ 30 min per
target; missing this isn't blocking but adds friction.

- **Effort**: ~4 hours
- **What**: GitHub Actions workflow on tag push that builds 6
  platform binaries, runs `npm run bundle-binary`, packages each as
  `.vsix`, signs and publishes

### C2. Multi-window session sync
Open Peridot in two VS Code windows on the same project; right now
each window has its own session list. A shared daemon means a
shared session list — needs broadcast of `session.list_changed` and
each window subscribing.

- **Effort**: ~6 hours
- **What**: new `session.subscribe_list` RPC, `session_list_changed`
  notification, sidebar reconcile-on-event

### C3. Marketplace "Try it" walkthrough
First-time users hit the auth selector and have to know what
`openai-oauth` vs `claude-api` means. A 3-step VS Code Walkthrough
that picks the right one based on what's already stored would
significantly lower the activation bar.

- **Effort**: ~3 hours
- **What**: `contributes.walkthroughs` in `package.json`, three
  steps tied to commands

### C4. Skill-aware autocomplete in the composer
The slash command parser now recognises `/skill-name` as a Skill
variant; the composer's autocomplete picker should pull the active
auto-skill list from the daemon and offer them inline.

- **Effort**: ~3 hours
- **What**: new `skills.list` daemon RPC (filtered to non-archived,
  `scope=auto`), composer popup that merges built-in slashes with
  skill names

### C5. Compaction visualization
`AgentRunEvent::ContextCompacted` ships the structured snapshot
(files_read, files_changed, decisions, …) but the sidebar only
shows a one-line summary. A click-to-expand panel that renders the
file list and decision bullets would help operators audit what the
agent thinks it knows.

- **Effort**: ~2 hours
- **What**: collapsible panel triggered by clicking the
  "Context compacted" row, rendering the `compacted` payload as a
  small tree

---

## What we explicitly DON'T do next

- **Server-side state for sessions** — sessions live in the
  workspace's `.peridot/sessions/`. Don't pivot to a hosted state
  store; the file-system model is part of the product.
- **Bundle additional language models** — Peridot is a thin shell
  around whatever provider the user has authenticated. Curated
  model lists belong in `peridot login`'s onboarding wizard, not
  in the extension.
- **Replace the daemon RPC with HTTP** — line-delimited JSON-RPC on
  stdio is what makes the daemon embeddable everywhere (TUI, VS
  Code, Cursor, future GUI shells). HTTP would force a port-
  allocation problem and break the "spawn-then-pipe" model.

---

## Tracking

This file lives in `docs/plans/`. When an item lands, add the
matching CHANGELOG entry and strike the row here. When a tier
empties, move the file to `docs/plans/archive/` so the next-roadmap
slot stays clean.
