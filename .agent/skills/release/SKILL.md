---
name: release
description: Prepare or execute a CC Session release. Use when the user asks to plan a release, bump a version, update release notes, prepare CHANGELOG.md, inspect changes since the last tag, run scripts/release.sh, or publish a version tag.
---

# Release

Use this skill to choose the release version, prepare `CHANGELOG.md`, and run `scripts/release.sh`.

## Changelog

- Write `CHANGELOG.md` in English.
- Keep entries user-facing and concise.
- Prefer sections in this order: `### Added`, `### Changed`, `### Fixed`, `### Removed`.
- Use `### Removed` for user-visible removals, dropped support, or deleted workflows.
- Append a PR number `(#N)` only if it already appears in commit history; otherwise use a short commit hash `(abc1234)` only when it adds traceability.
- Exclude pure CI churn, dependency-only maintenance, test-only changes, formatting-only changes, and dead-code cleanup unless they affect users or release behavior.
- Keep the active heading as `## [X.Y.Z] - Unreleased` while preparing the release. `scripts/release.sh` stamps the final date.

## Commits

- Use [Conventional Commits](https://www.conventionalcommits.org/) for every release-related commit.
- Format commit messages as `<type>[optional scope]: <description>`, for example `docs: update changelog for v0.6.0`.
- Prefer these types: `feat`, `fix`, `refactor`, `perf`, `docs`, `test`, `chore`, and `ci`.
- Use `docs: update changelog for vX.Y.Z` for a changelog-only preparation commit.
- Keep the release script commit as `chore: release vX.Y.Z`.
- Keep one logical change per commit. Do not mix formatting-only, dependency, docs, and behavior changes unless the user explicitly asks for a combined release-prep commit.
- Mark breaking changes with `!` after the type or a `BREAKING CHANGE:` footer, and treat them as a major-version signal.
- Before committing, check `git config --local --get user.name` and `git config --local --get user.email`. If either is missing, ask the user to set repo-local identity with `git config --local user.name "Name"` and `git config --local user.email "email@example.com"`; do not write global git config.

## Workflow

### 1. Inspect Release State

Use local repository history:

Run:

```bash
git status --short
git describe --tags --abbrev=0
git log "$(git describe --tags --abbrev=0)..HEAD" --oneline --no-decorate
```

### 2. Choose Version

Use semantic versioning:

- Breaking change or `!` / `BREAKING CHANGE:` -> major
- User-facing `feat:` -> minor
- Fixes, refactors, maintenance, docs, or release tooling only -> patch

If the user already gave a version, use it. Otherwise propose the version and wait for confirmation before executing the release.

### 3. Prepare CHANGELOG.md

Update or create the target heading:

```markdown
## [X.Y.Z] - Unreleased
```

Write only sanitized, user-facing entries. Keep `Unreleased`; the release script replaces it with the current date during the release commit.

### 4. Validate Before Release

For release preparation work, run the checks for the touched area. For an actual release, `scripts/release.sh` runs:

- `npm run check`
- `npm test`
- `npm run knip`
- `cd src-tauri && cargo fmt --check`
- `cd src-tauri && cargo clippy --all-targets --all-features -- -D warnings`
- `cd src-tauri && cargo test`

Knip type-only unused export warnings are advisory if configured as warnings and the command exits 0.

### 5. Execute Release

Before running the release script:

1. Confirm with the user that pushing commits and tags is allowed.
2. Ensure the working tree is clean.
3. Ensure `CHANGELOG.md` contains exactly `## [X.Y.Z] - Unreleased`.
4. Do not use `git add .`; stage explicit release files only when manual staging is needed.

Run:

```bash
./scripts/release.sh X.Y.Z
```

The script bumps package, Cargo, and Tauri versions; stamps the changelog date; updates lockfiles; commits `chore: release vX.Y.Z`; creates an annotated tag; and pushes both commit and tag.

After release, report the pushed tag and point the user to GitHub Actions and Releases.
