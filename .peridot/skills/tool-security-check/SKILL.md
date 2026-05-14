---
name: tool-security-check
description: Check Peridot tool security and permission behavior. Use when adding or modifying tools, shell execution, file writes, git operations, hooks, command blocklists, path sandboxing, destructive command classification, or permission mode behavior.
---

# Tool Security Check

## Checklist
1. Declare tool group, permission level, read-only status, concurrency safety, and state mutation.
2. Apply hard command blocklists before user hooks.
3. Resolve paths before sandbox and boundary checks.
4. Keep Plan Mode read-only.
5. Require confirmation for destructive/system operations unless policy explicitly allows automation.
6. Log blocked or suspicious actions in audit paths that are ignored by git.

## Tests
- Add deterministic tests for command classification.
- Add symlink/path traversal tests for file tools.
- Add permission mode matrix tests for safe, auto, and yolo.
- Add hook ordering tests: built-in pre-hook, user pre-hook, tool, built-in post-hook, user post-hook, event.

## Maintenance
- If a rule produces frequent false positives, narrow the pattern and document the example.
- Do not move security checks into LLM prompts when deterministic code can enforce them.
