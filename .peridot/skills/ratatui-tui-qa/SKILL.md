---
name: ratatui-tui-qa
description: Design or verify Peridot Ratatui terminal UI behavior. Use for layout modes, header/status bars, side panels, streaming rendering, ask_user screens, keybindings, slash commands, Peridot Night theme, mascot rendering, multi-session tab bar, i18n, approval scope, or terminal snapshot QA.
---

# Ratatui TUI QA

## Checklist
1. Test full, compact, and minimal layout thresholds (`select_layout`).
2. Keep header minimal: `PERIDOT  <model>` only; mode/permission/tokens/cost/cache live in the status bar (`render_status_metrics`).
3. Markers reduced to five: `▸` user / `◆` assistant / `❯` tool start / `✔`·`✘` tool result / `⚠` notice. Never reintroduce sub-line icons.
4. Apply `Wrap { trim: false }` to transcript and side panel; long lines must wrap, not horizontally clip.
5. Keybindings:
   - `Esc` — interrupt while agent is busy; clear input if non-empty; otherwise open menu.
   - `Shift+Enter` — newline in the input buffer; `Enter` submits.
   - `Tab` — autocomplete slash command prefix when picker is open.
   - `Ctrl+P` — menu; `Ctrl+]` — toggle side panel; `Ctrl+T`/`Ctrl+W` — cycle foreground session.
   - `PgUp/PgDn` — scrollback when transcripts exceed the viewport.
6. Approval panel offers four scopes: Approve once / Approve for session / Approve always / Deny — return `(ApprovalDecision, ApprovalScope)` together.
7. Slash command picker (`slash_picker`) activates on `/`-prefixed input and never collides with menu/ask_user/approval panels.
8. i18n: every user-facing string flows through `tr(PhraseKey, Locale)` — never embed raw Korean/English literals in render code. Default locale is `Locale::En`.
9. Cross-crate event plumbing: `PlanUpdated`, `BudgetUpdated`, `ContextUtilizationChanged`, `McpStatusChanged`, `AgentsMdLoaded`, `HookFired`, `TurnEnded`, and `Interrupted` must round-trip from `AgentRunEvent` through the CLI adapter into `TuiRuntimeEvent` and update side panel state.
10. Cooperative interrupt: `CancelToken` checked at the top of every loop iteration; `AgentRunStatus::Interrupted` survives subsequent `Finished` events (interrupted wins).
11. Subagent monitor renders depth-indented tree with `id`, `parent_id`, `depth`, `tokens`. Foreground vs background sessions show distinct attention badges.
12. Mascot (`mascot/` module) maps `TuiState` → `MascotState` (8 moods), renders 8×8 pixel art via `▀` half-block + truecolor in the side panel top-left. Hidden in `LayoutMode::Minimal` or when `show_mascot=false`.
13. Multi-session tab bar (`session_directory`) compacts when narrow; pending-attention sessions surface a `!` suffix.
14. Theme contrast: Peridot Night fg/bg pairs and mascot palette (`peridot_palette()`) must remain accessible.
15. Approval flows must be visible and reversible at the UI boundary: show the blocked tool/action, accept approve/deny keys, and keep the current run context available after the decision.

## Testing
- Prefer deterministic render tests for layout state. Use `peridot-tui/src/fixtures.rs` `TestScenario` enum to seed states.
- Drive every scenario through `ratatui::backend::TestBackend` to assert the cell buffer, not just text snapshots.
- Test long text, narrow terminals (width 72 / minimal), streaming updates, and locale swaps.
- Add regression tests for: Esc-interrupt vs Esc-menu disambiguation, Shift+Enter newline, slash autocomplete via Tab, cross-crate event arms, thinking-log persistence across `debug_view` toggle, mascot frame cycling vs `spinner_tick`, subagent tree depth indent.
- Render snapshots should assert stable user-visible surfaces rather than every decorative character.
- For mascot tests, validate the 8×4 cell window contains `▀` glyphs with expected fg/bg colors; verify ASCII fallback when truecolor is unavailable.
- Avoid decorative UI that obscures repeated coding workflows.

## Maintenance
- If UI tests become brittle, narrow snapshots to stable regions and assert layout invariants separately.
- When adding a new `AgentRunEvent` or `TuiRuntimeEvent`, update conversion, state application, render coverage, and tests in the same change.
- When adding a new `PhraseKey`, populate both `Locale::En` and `Locale::Ko` arms — the `match` is exhaustive and a missing arm fails compile, never let `unreachable!` plug it.
- When adding a new slash command, register a `SlashCommandSpec` in `slash_command_catalog()` so the picker and `/help` discover it automatically.
- When changing `TuiState` schema, give every new field `#[serde(default)]` so saved sessions stay resumable.
