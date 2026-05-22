# Multi-LLM Committee Runbook

This runbook tracks Peridot's three-role committee mode — **Planner**,
**Reviewer**, **Executor** — on top of the normal `HarnessAgent` loop. The
intent is to lift quality by making long tasks reason in stages and by
reviewing mutating turns before the run continues.

## Why now

- Goal Checker proved the harness can call a secondary model, but it only
  applies in goal mode. Committee mode generalizes that pattern to ordinary
  coding sessions.
- Per-session `/provider` and model overrides already give the executor a live
  runtime identity. Committee mode adds planner and reviewer model slots around
  that executor path without changing the default single-agent flow.
- The hardest bugs are often diffs the executor thinks are done. A reviewer
  role catches those issues while the session still has context to repair them.

## High-level shape

```
┌────────────────────────────────────────────────────────────────┐
│  Operator types a task                                          │
└────────────────────────────────────────────────────────────────┘
                          │
                          ▼
┌────────────────────────────────────────────────────────────────┐
│  Planner agent (plan-mode constrained, models.planner)          │
│  - read-only tools                                              │
│  - one-shot turn: produce a structured plan                     │
└────────────────────────────────────────────────────────────────┘
                          │  plan_text
                          ▼
┌────────────────────────────────────────────────────────────────┐
│  Executor agent (execute mode, models.main or per-session model) │
│  - sees task + planner plan as PlanReminder context             │
│  - normal Peridot turn loop                                     │
└────────────────────────────────────────────────────────────────┘
                          │ after each turn that touched files
                          ▼
┌────────────────────────────────────────────────────────────────┐
│  Reviewer agent (single-turn, models.reviewer)                  │
│  - sees the diff (git diff) + the relevant transcript slice     │
│  - returns Verdict { approve | request_changes | block }        │
└────────────────────────────────────────────────────────────────┘
                          │
                          ├─ approve         → executor continues
                          ├─ request_changes → comments injected
                          │                    into executor context
                          │                    next turn
                          └─ block           → loop pauses, asks user
```

Three modes of activation:

- **off** (default): committee is bypassed, single-agent loop runs exactly like today.
- **planner-only**: pre-flight planner pass, then single executor loop (no per-turn reviewer).
- **full**: planner + executor + reviewer-in-loop.

Switchable per session via `/committee <mode>` slash (and by `[committee]
mode = "planner"` or `"full"` in project config).

## Surfaces in play

| Concern | Today | After |
|---|---|---|
| Active models | `models.main` plus goal checker | `models.planner` and `models.reviewer` wrap the executor; executor still uses `models.main` or per-session `/model` |
| AgentRole | default executor role | explicit `AgentRole::{Planner, Reviewer, Executor}` prompt suffixes |
| AgentRunEvent | single-agent stream | `PlannerPlanReady`, `ReviewerVerdict`, and `CommitteeRoleUsage` |
| TUI side panel | mascot · plan · subagents · budget · MCP | committee status, verdict counts, and per-role usage |
| Slash commands | TUI slash catalog | `/committee off|planner|full` in the shared catalog |
| ContextSource | User / Assistant / Tool / PlanReminder / External | `ReviewerComment` for requested changes |
| CommitteeConfig | base project config | `[committee] mode`, `planner_model`, `reviewer_model`, `executor_model`, `max_review_passes` |

## Phase / milestone breakdown

### Phase 1 — Foundation: AgentRole + CommitteeConfig (M-COM1)
- `peridot_common::CommitteeConfig` enum and struct (mode, planner_model, reviewer_model, executor_model, max_review_passes); default `mode = Off`.
- `peridot_core::AgentRole { Planner, Reviewer, Executor }` (default `Executor`).
- `HarnessAgent` carries `role: AgentRole`; system prompt switches per role (`system_prompt_for_role` joins `system_prompt_for_mode` + role guidance).
- `[committee]` TOML section parses cleanly; `cargo test --workspace` green; no behaviour change yet (mode defaults to Off).

### Phase 2 — Planner pre-flight (M-COM2)
- New `run_planner_preflight(planner_agent, provider, task)`: one-turn read-only run that emits a plan markdown.
- Wire into `peridot-cli::run_loop` so `committee.mode != Off` triggers it before the executor loop starts.
- New `AgentRunEvent::PlannerPlanReady { plan_text }` + TUI consumes it as a `TranscriptKind::System` line and primes `state.side_panel.plan` from the parsed plan.
- Plan text is injected into the executor's context as a trusted `ContextSource::PlanReminder` before turn 1.
- Test: mock provider returns a canned plan; executor sees it as the first context message.

### Phase 3 — Reviewer in-loop (M-COM3)
- New `Verdict { Approve, RequestChanges { comments }, Block { reason } }` and `AgentRunEvent::ReviewerVerdict { turn, verdict }`.
- After every executor turn that produced a tool result touching the workspace (file_write, file_patch, shell_exec with mutation), call `run_reviewer_pass(reviewer_agent, provider, diff, transcript_window)`.
- `RequestChanges` injects the reviewer's comments as `ContextSource::ReviewerComment` into the executor's context before the next turn (capped at `max_review_passes` to prevent loops).
- `Block` interrupts the executor (re-uses M9 CancelToken machinery) and emits an `AskUser` panel so the operator can override.
- Test: mock executor turn produces a diff, mock reviewer requests changes, next executor turn sees the comment in context.

### Phase 4 — `/committee` slash + status surface (M-COM4)
- New `SlashCommand::Committee(CommitteeMode)`; parses `off`, `planner`, `full`.
- TUI status bar appends `committee <mode>` when not Off, and side panel grows a "Committee" mini-section showing the last planner/reviewer event timestamps.
- `slash_command_catalog` advertises `/committee`.
- Tests: slash command toggles `state.config.committee.mode`; render snapshot surfaces `committee <mode>`.

### Phase 5 — Resilience & guardrails (M-COM5)
- Cost accounting: planner/reviewer turns add to `header.cost_usd` with their own provider; `/cost` and `/info` surface a per-role breakdown (`planner $0.01 · reviewer $0.02 · executor $0.05`).
- Max review passes guard: if reviewer keeps returning `RequestChanges` for the same diff signature N times, escalate to `Block` automatically with an explicit reason.
- Reviewer-friendly diff truncation (M9 / replay_step uses similar slicing): large diffs are summarised before being sent so they fit within the reviewer's context.
- Tests: cost split per role; runaway-loop guard fires after N rounds.

### Phase 6 — Persistence + replay (M-COM6)
- Plan text and reviewer verdicts persist to `<sessions>/<id>/committee.ndjson` (one JSON line per event: planner / reviewer / verdict). M3 throttled save path picks them up.
- `peridot session show --committee-tail N` prints the last N committee events (re-uses the M30 pattern).
- `peridot session replay` interleaves committee events with transcript entries (chronological).
- Tests: round-trip planner plan + reviewer verdicts through `session show` and `session replay`.

### Phase 7 — Documentation & rollout (M-COM7)
- `AGENTS.md` Multi-Session Notes section grows a "Committee" subsection: when to enable, which models pair well, cost/latency notes.
- `.peridot/skills/multisession-orchestration/SKILL.md` cross-links to the committee runbook.
- Two ready-made config snippets in the README: "minimal committee" (planner-only with `claude-haiku-4-5` planner over `claude-opus-4-7` executor) and "full committee" (claude planner + openai reviewer + claude executor).

## Cross-cutting checklist (every committee PR)

- [ ] `cargo fmt --all --check`
- [ ] `cargo clippy --workspace --all-targets -- -D warnings`
- [ ] `cargo test --workspace`
- [ ] `cargo build --release -p peridot-cli`
- [ ] New `AgentRunEvent` variants flow through `peridot-cli/src/main.rs` adapter into a `TuiRuntimeEvent` arm
- [ ] New `PhraseKey` arms in both English and Korean for any visible string
- [ ] New TOML fields carry `#[serde(default)]` so existing config files still parse
- [ ] No new unbounded channels; reviewer runs are still bounded by `max_review_passes`

## Risks & mitigations

| Risk | Mitigation |
|---|---|
| Cost blows up (3× LLM calls per turn) | `committee.mode` defaults to Off; explicit slash to enable; per-role cost surfaced in `/cost` so the operator can see the impact |
| Latency triples | Planner is one-shot pre-flight; reviewer runs only on turns that mutate files, not on read-only thinking turns |
| Reviewer + Executor lock loop ("review never approves") | `max_review_passes` cap, then auto-Block with a clear reason |
| Provider mismatch (reviewer model can't see file diff size) | Diff is sliced before send (re-use M9 sizing); fallback to summary when over budget |
| Existing single-agent users break | Phase 1 is a no-op behaviour change (mode=Off); existing sessions / configs / tests all still pass |
| `models.planner` etc. set to a model the active provider doesn't support | Validated at agent construction; falls back to `models.main` with a transcript notice |

## Out of scope (this design pass)

- More than three roles (no `Critic`, no `Memory Curator`, no `Tester` yet)
- Multi-executor (peer review by *two* executors) — orthogonal feature
- Real-time visualization of role conversation in the TUI (a transcript line per verdict is enough)
- Cross-provider streaming sync — each role gets one shot at a time, sequential
- Plug-in / external role registration

## PR layout

1. **M-COM1** — Foundation: enums + config + role-aware system prompt (behaviour off by default) ✓ landed
2. **M-COM2** — Planner pre-flight + `PlannerPlanReady` event + context inject ✓ landed
3. **M-COM3** — Reviewer verdict types + `ContextSource::ReviewerComment` + transcript surface ✓ landed (part 1; part 2 absorbed into M-COM4b)
4. **M-COM4a** — `/committee` slash + status bar surface ✓ landed
5. **M-COM4b** — Turn-by-turn committee loop + in-loop reviewer pass + verdict apply + max_review_passes guard ✓ landed
6. **M-COM5** — Per-role cost split (`CommitteeRoleUsage` event + `/cost` / `/info` surface) ✓ landed
7. **M-COM6** — Persistence (`committee.ndjson`) + `session show --committee-tail` ✓ landed
8. **M-COM7** — Docs (this runbook), skill cross-links (`committee-orchestration/SKILL.md`), `AGENTS.md` reference, ready-made config snippets ✓ landed

Each PR is reviewable in isolation and leaves the workspace fmt/clippy/test green.

## Outstanding (post-M-COM7)

These items were called out during plan but deferred so the seven core PRs stay sized:

- **Replay weaving** — `peridot session replay` interleaves committee events with transcript chronologically. The data is on disk (`committee.ndjson`, `transcript.ndjson`); the CLI just needs to merge-sort by `ts` before printing.
- **Diff-signature duplicate guard** — orthogonal to consecutive `RequestChanges`: detect when reviewer rejects the *same* diff signature N times even with gaps between, and auto-Block. Hash-based.
- **Block prompt** — when `Block` fires, drop an `AskUser` panel so the operator can override and continue. Today the run halts cleanly via cancel token; the operator restarts manually.
- **`AgentRole` for `models.executor`** — the planner / reviewer respect their own `models.*` keys, but the executor still uses `models.main` / per-session `/model`. Adding `models.executor` would let one project's committee run on a different executor model than `models.main` without slash.
