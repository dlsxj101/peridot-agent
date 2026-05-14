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

## Testing
- Prefer deterministic render tests for layout state.
- Use terminal snapshot tests where practical.
- Test long text, narrow terminals, and streaming updates.
- Avoid decorative UI that obscures repeated coding workflows.

## Maintenance
- If UI tests become brittle, narrow snapshots to stable regions and assert layout invariants separately.
