#!/usr/bin/env sh
set -eu

command_text="${*:-}"
log_dir="${PERIDOT_PROJECT_ROOT:-$(pwd)}/.peridot/logs"
audit_file="$log_dir/dangerous-shell.log"

mkdir -p "$log_dir"

case "$command_text" in
  *"rm -rf /"*|*"mkfs."*|*"dd if=/dev/zero"*|*":(){ :|:& };:"*|*"chmod -R 777 /"*|*"curl "*"| sh"*|*"wget "*"| bash"*)
    severity="hard-block-candidate"
    ;;
  *"git push --force"*|*"git reset --hard"*|*"sudo "*|*" chmod "*|*" chown "*|*"npm publish"*|*"cargo publish"*|*"DROP TABLE"*|*"DROP DATABASE"*|*"TRUNCATE "*)
    severity="confirmation-required-candidate"
    ;;
  *)
    exit 0
    ;;
esac

{
  printf '%s\t%s\t%s\n' "$(date -u '+%Y-%m-%dT%H:%M:%SZ')" "$severity" "$command_text"
} >> "$audit_file"

echo "pre-shell-dangerous-command: recorded $severity. Runtime policy should decide whether to block."
