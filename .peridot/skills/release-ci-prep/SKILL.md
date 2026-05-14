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
- Build six targets from the spec.
- Package Unix artifacts as tarballs and Windows artifacts as zips.
- Generate and verify SHA256 checksums.
- Preserve `peridot` binary and `peri` alias behavior.
- Keep install/update scripts safe and explicit.

## Versioning
- Use tags shaped like `v0.1.0`.
- Ensure `peridot version` and package metadata agree.
- Document manual release steps only when automation cannot own them yet.

## Maintenance
- If CI time grows too much, split quick checks from heavy release/E2E checks rather than weakening required local verification.
