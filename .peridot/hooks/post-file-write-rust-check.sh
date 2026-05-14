#!/usr/bin/env sh
set -eu

changed_path="${1:-}"
cd "${PERIDOT_PROJECT_ROOT:-$(pwd)}"

case "$changed_path" in
  *.rs) ;;
  *)
    echo "post-file-write-rust-check: non-Rust path; skipping."
    exit 0
    ;;
esac

if [ "${PERIDOT_SKIP_SLOW_HOOKS:-0}" = "1" ]; then
  echo "post-file-write-rust-check: PERIDOT_SKIP_SLOW_HOOKS=1; skipping."
  exit 0
fi

if [ ! -f Cargo.toml ]; then
  echo "post-file-write-rust-check: no Cargo.toml found; skipping."
  exit 0
fi

if ! command -v cargo >/dev/null 2>&1; then
  echo "post-file-write-rust-check: cargo is not installed; skipping."
  exit 0
fi

echo "post-file-write-rust-check: cargo check --workspace"
cargo check --workspace
