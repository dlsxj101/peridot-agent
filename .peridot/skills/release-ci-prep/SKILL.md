---
name: release-ci-prep
description: Prepare Peridot CI, release, install, and distribution workflows. Use for GitHub Actions CI, release.yml, cross-compilation targets, install.sh, Homebrew notes, versioning, cargo release checks, or packaging readiness.
---

# Release CI Prep

## CI Checklist
- Run format, clippy, check, and tests on pull requests.
- Test on Linux, macOS, and Windows.
- Keep real API E2E tests gated, budgeted, serialized, and out of ordinary PR paths.
- Upload coverage only from trusted main-branch contexts.

## Release Checklist
- Before committing release changes or creating any `v*` tag, bump package metadata to the exact release version.
- Check root `Cargo.toml` `[workspace.package].version`, internal path dependency versions such as `peridot-cli/Cargo.toml`, and `Cargo.lock`.
- Never push a release tag until the version bump commit is on `main` and CI has passed for that commit.
- Build six targets from the spec.
- Package Unix artifacts as tarballs and Windows artifacts as zips.
- Generate and verify SHA256 checksums.
- Preserve `peridot` binary and `peri` alias behavior.
- Keep install/update scripts safe and explicit.

## Versioning
- Use tags shaped like `v0.1.0`.
- Ensure `peridot version`, Cargo package metadata, release artifact names, and the release tag agree.
- If releasing `vX.Y.Z`, the workspace version must be `X.Y.Z` before the release commit is pushed.
- Run `cargo check --workspace` or a stronger Cargo command after a version bump so `Cargo.lock` is refreshed.
- If the user asks to release without a version bump, pause and make the version bump first.
- Document manual release steps only when automation cannot own them yet.

## Maintenance
- If CI time grows too much, split quick checks from heavy release/E2E checks rather than weakening required local verification.
