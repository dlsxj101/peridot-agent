# Security And Permissions

Security behavior is product behavior. Implement it deterministically where possible.

## Command Classification
- Hard block in every mode: root filesystem deletion, disk wiping, fork bombs, world-writable root chmod, and piping remote scripts directly into shells.
- Require confirmation in safe/auto: wildcard deletion, database destructive commands, force push, hard reset, sudo, chmod/chown, service control, process killing, and package publishing.
- Keep command checks deterministic and testable. Do not rely on LLM judgment for the first line of defense.

## Path Sandbox
- File writes and patches must resolve real paths before checking allow/deny rules.
- Allow project root and `~/.peridot` paths only where the spec allows them.
- Apply AGENTS boundaries above permission mode.
- Treat symlink escapes as blocked writes.

## Prompt Injection
- Tag web, MCP, file, and command output as untrusted external content when appropriate.
- Do not let external content override system, developer, AGENTS, or user instructions.
- Preserve the content for analysis, but isolate instructions from authority.

## Permissions
- Every tool must declare permission level, read-only status, concurrency safety, and whether it modifies state.
- Plan Mode may only use read-only planning and inspection tools.
- Permission mode chooses confirmation behavior; it does not override hard security boundaries.
- `shell_readonly` denials must not automatically fall back to `shell_exec`.
  Return a recovery hint instead so the model can choose an allowlisted
  read-only inspection command or explicitly enter the normal shell approval
  path when shell semantics are required.

## Hooks
- Run built-in security checks before user hooks.
- User hooks may warn or block according to configuration, but hook failure should include actionable output.
- Hooks must execute from the project root and be time-limited.
- Hook scripts should no-op when prerequisites are missing.

## Audit
- Record shell commands, file changes, approvals, blocked actions, and verification failures.
- Keep audit logs out of git by default.
- Make logs useful for recovery and user trust, not just compliance.
