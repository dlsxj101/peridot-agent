# F4 — Web UI / Browser Client

Design doc for roadmap item **F4** (spec §21.5.10). Implementation plan,
not landed code. Largest effort of the feature track (4–8 weeks). The good
news: the daemon RPC contract built for the VS Code extension is the
foundation — F4 reuses it over a web transport.

## Goal

A browser front-end that drives Peridot sessions: transcript, composer
with slash autocomplete, approvals, ask-user prompts, and the live
usage/budget HUD — the same surfaces the TUI and VS Code already expose,
served to a browser.

## Current state

- ✅ `peridot daemon` speaks line-delimited JSON-RPC 2.0 over stdio:
  `session.start`, `session.command`, `session.command_catalog`,
  `interaction.respond`, `approval.respond`, streamed `AgentRunEvent`s,
  `peridot.status`, etc. VS Code already consumes all of this.
- ❌ No network transport, no auth, no multi-user isolation, no browser
  client.

## Architecture

Two layers: a transport bridge in Rust, and a separate front-end app.

### 1. Transport bridge — `peridot daemon --serve` (or `peridot serve`)

- Add an HTTP + WebSocket server (e.g. `axum` + `tokio-tungstenite`) that
  bridges to the **existing** JSON-RPC dispatch. The browser sends the
  same `{jsonrpc, id, method, params}` envelopes over a WebSocket; the
  bridge forwards them into `dispatch_request` and streams
  `AgentRunEvent`/notifications back over the socket.
- Reuse, do not fork, the daemon handlers — the bridge is an adapter from
  WS frames to the line-delimited protocol the daemon already implements.
- One WebSocket connection ≈ one stdio client today; multi-session
  routing already exists in `SessionRouter`.

### 2. Auth + isolation (new, security-sensitive)

- **Auth**: local-only by default (bind `127.0.0.1`, single-user token in
  `.peridot`/env). Remote/multi-user is a separate hardening milestone
  (TLS, per-user tokens, rate limits).
- **Workspace isolation**: each authenticated session maps to a project
  root + git worktree, reusing the existing `WorkspaceIsolation`. Never
  let one web session read another's filesystem scope.
- **Honor the security model**: path sandboxing, command blocklists,
  AGENTS boundaries, prompt-injection tagging all still apply — the web
  client must not be a bypass. Approvals route through the same
  `approval.respond` path.

### 3. Front-end app (`web/`, separate package)

- React + TypeScript + a WS client. **Reuse the VS Code webview
  components** where possible — the webview is already a TS app that
  renders transcript/composer/approvals from daemon events; factor shared
  rendering into a package consumed by both.
- Surfaces: transcript, composer (slash autocomplete from
  `session.command_catalog?surface=web`, `@file` mentions), approval +
  ask-user modals, usage/budget HUD, session switcher.

### 4. Surface metadata

- The slash catalog already carries `surfaces` metadata (E39–E41,
  E44/E54). Add a `web` surface so the catalog filters web-appropriate
  commands, exactly as `vscode` does today.

## Integration points

- `peridot-cli`: new `serve`/`--serve` subcommand, axum server, WS↔RPC
  bridge, auth middleware.
- Daemon: a `web` surface value in `command_catalog` filtering (additive).
- `web/`: new front-end workspace (separate from the Rust workspace),
  ideally sharing webview rendering with `extensions/vscode`.

## Milestones

1. WS↔JSON-RPC bridge for a single local session (echo `peridot.version`,
   then `session.start` + event streaming) — Rust integration test with a
   WS client.
2. Local-only auth (loopback + token).
3. Minimal browser client: connect, start a task, render streamed events.
4. Composer slash autocomplete via the `web`-surface catalog.
5. Approvals + ask-user modals over `approval.respond` /
   `interaction.respond`.
6. Usage/budget HUD; session switcher; `@file` mentions.
7. (Separate hardening) multi-user auth, TLS, rate limiting, remote bind.

## Risks / decisions

- **Security blast radius**: a network listener is the highest-risk
  surface in the project. Default to loopback + token; gate remote/
  multi-user behind explicit config and a dedicated hardening pass.
- **Scope**: ship local-single-user first; treat multi-user as a distinct
  project.
- **Duplication**: factor shared TS rendering out of the VS Code webview
  rather than reimplementing it, or the two clients drift.

## Testing

- Rust: WS bridge integration tests (connect, RPC round-trip, event
  stream, auth rejection).
- Front-end: component tests on the shared rendering package (the VS Code
  extension already has webview unit tests to model these on).
- An end-to-end smoke test: spawn `peridot serve`, drive a mock-provider
  session from a headless WS client, assert the event stream.
