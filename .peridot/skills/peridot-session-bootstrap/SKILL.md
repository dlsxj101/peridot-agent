---
name: peridot-session-bootstrap
description: Start or resume Peridot Agent implementation work. Use when beginning a new coding session, selecting the current implementation phase, checking repository state, reading PERIDOT_SPEC_v1.md, or preparing a handoff-aware task plan.
---

# Peridot Session Bootstrap

## Workflow
1. Read `PERIDOT_SPEC_v1.md` sections relevant to the requested work.
2. Identify which of the seven implementation sessions the task belongs to.
3. Inspect current repository state and existing files before editing.
4. If `Cargo.toml` exists, check the current workspace build/test status or explain why not.
5. State the narrow implementation unit and expected verification before editing.
6. At the end, record incomplete work, verification results, and any skill/hook inefficiency noticed.

## Guardrails
- Do not skip spec reading because the repo is small.
- Do not advance phase completion without the spec's done criteria.
- If a hook or skill slows this workflow without adding signal, update it or leave a clear maintenance note.
