---
name: context-cache-invariants
description: Preserve Peridot context and prompt-cache invariants. Use when modifying context history, compaction, offloading, system prompt construction, tool definition ordering, structured variation, or cache breakpoint behavior.
---

# Context Cache Invariants

## Invariants
- Append conversation history; do not rewrite prior messages except through explicit compaction markers.
- Keep tool definitions and system prompt sections deterministic.
- Keep volatile values out of stable cached sections.
- Preserve `todo.md` or plan reminder injection near the current turn.
- Offload large outputs and retain file references.
- Try cheaper compaction tiers before expensive ones.

## Review Steps
1. Identify which cache breakpoint the change touches.
2. Check whether serialization order is stable.
3. Check whether mode changes only invalidate the intended section.
4. Verify external content is tagged and does not become authoritative.
5. Add tests for token thresholds, offload thresholds, and compaction behavior.

## Warning Signs
- `HashMap` output appears in prompt/tool serialization.
- Current time enters the system prompt.
- Failed tool output disappears from context.
- Compaction drops `MEMORY.md`, `todo.md`, or active plan state.
