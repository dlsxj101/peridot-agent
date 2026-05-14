# Skill And Hook Maintenance

Skills and hooks exist to reduce repeated work. They should be edited when they stop doing that.

## Change Triggers
- A skill repeats generic advice that the base agent already knows.
- A skill is too broad and triggers for unrelated tasks.
- A hook slows normal iteration without catching real problems.
- A hook frequently warns on harmless actions.
- A hook blocks work that the permission model should handle.
- A script lacks a no-op path for early project states.

## Principles
- Measure before expanding.
- Prefer narrower triggers over longer instructions.
- Keep block hooks rare.
- Add no-op fallbacks for missing `Cargo.toml`, missing commands, empty parameters, and early skeleton phases.
- Fold duplicated skills together.
- Delete stale skills rather than preserving them for sentiment.

## Hook Tuning
- Use `PERIDOT_SKIP_SLOW_HOOKS=1` for expensive advisory hooks.
- Use `on_failure = "warn"` while a check is young or noisy.
- Promote to `block` only after the check is fast, deterministic, and protects repository integrity.
- Keep hook output short enough to be useful in agent context.

## Skill Tuning
- Keep `SKILL.md` concise.
- Move detailed references into separate files only when they are frequently useful and too long for the main skill.
- Prefer procedural checklists over architecture essays.
- Update the skill immediately after discovering a repeated miss.

## Review Cadence
- Review skills/hooks at the end of each Peridot implementation phase.
- Keep a note in the phase handoff when a skill or hook should be revised next.
- Treat ineffective automation as technical debt, not harmless clutter.
