#!/usr/bin/env sh
set -eu

cd "${PERIDOT_PROJECT_ROOT:-$(pwd)}"

if [ ! -f Cargo.toml ]; then
  echo "pre-git-commit-quality: no Cargo.toml found; skipping cargo quality gate."
  exit 0
fi

if ! command -v cargo >/dev/null 2>&1; then
  echo "pre-git-commit-quality: cargo is not installed; skipping."
  exit 0
fi

echo "pre-git-commit-quality: cargo fmt --all --check"
cargo fmt --all --check

echo "pre-git-commit-quality: cargo clippy --workspace -- -D warnings"
cargo clippy --workspace -- -D warnings

echo "pre-git-commit-quality: cargo test --workspace"
cargo test --workspace
