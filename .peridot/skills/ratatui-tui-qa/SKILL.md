---
name: ratatui-tui-qa
description: Design or verify Peridot Ratatui terminal UI behavior. Use for layout modes, header/status bars, side panels, streaming rendering, ask_user screens, keybindings, slash commands, Peridot Night theme, or terminal snapshot QA.
---

# Ratatui TUI QA

## Checklist
1. Test full, compact, and minimal layout thresholds.
2. Keep header status fields stable: mode, permission, model, tokens, cost, cache.
3. Ensure main panel entries distinguish thinking, tools, diffs, success, failure, warning, and ask_user.
4. Ensure side panel can collapse and never blocks core interaction.
5. Keep keybindings consistent with the spec.
6. Check theme contrast for Peridot Night.
7. Keep input editing terminal-native: Backspace and Ctrl-H delete before the cursor, Delete removes at the cursor, arrows/Home/End move predictably, and submitted text clears without exiting the TUI.
8. Treat the agent run as a background event stream. Submitting a task should update run status, transcript, tool activity, approvals, usage, and session continuity inside the TUI instead of dropping to an external summary.
9. Approval flows must be visible and reversible at the UI boundary: show the blocked tool/action, accept approve/deny keys, and keep the current run context available after the decision.

## Testing
- Prefer deterministic render tests for layout state.
- Use terminal snapshot tests where practical.
- Test long text, narrow terminals, and streaming updates.
- Add regression tests for Backspace/Ctrl-H, cursor-position editing, slash-command handling, runtime events, tool-start previews, tool-result previews, and approval panels when touching input or event handling.
- Render snapshots should assert stable user-visible surfaces rather than every decorative character.
- Avoid decorative UI that obscures repeated coding workflows.

## Maintenance
- If UI tests become brittle, narrow snapshots to stable regions and assert layout invariants separately.
- When adding a new `AgentRunEvent` or `TuiRuntimeEvent`, update conversion, state application, render coverage, and tests in the same change.
