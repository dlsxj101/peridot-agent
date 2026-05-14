#!/usr/bin/env sh
set -eu

stage="${1:-unknown-stage}"
output="${2:-}"
log_dir="${PERIDOT_PROJECT_ROOT:-$(pwd)}/.peridot/logs"
log_file="$log_dir/verification-failures.log"

mkdir -p "$log_dir"
{
  printf '%s\n' "## $(date -u '+%Y-%m-%dT%H:%M:%SZ') stage=$stage"
  printf '%s\n' "$output" | sed -n '1,80p'
  printf '\n'
} >> "$log_file"

echo "on-verification-failed: wrote $log_file"
