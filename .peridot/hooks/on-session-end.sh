#!/usr/bin/env sh
set -eu

session_id="${1:-unknown-session}"
status="${2:-unknown-status}"
summary="${3:-}"
session_dir="${PERIDOT_PROJECT_ROOT:-$(pwd)}/.peridot/sessions"
handoff="$session_dir/latest-handoff.md"

mkdir -p "$session_dir"
{
  printf '# Latest Peridot Handoff\n\n'
  printf -- '- Session: %s\n' "$session_id"
  printf -- '- Status: %s\n' "$status"
  printf -- '- Updated: %s\n\n' "$(date -u '+%Y-%m-%dT%H:%M:%SZ')"
  printf '## Summary\n\n%s\n' "$summary"
} > "$handoff"

echo "on-session-end: wrote $handoff"
