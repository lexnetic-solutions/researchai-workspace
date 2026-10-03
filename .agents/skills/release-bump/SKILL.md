---
name: release-bump
description: Use this skill to finalize a release. It stamps the [Unreleased] changelog section with a version and date, bumps the version across all tracked files, and creates the release commit and tag. Only run when ready to ship.
---

# Release Bump

## Goal

Finalize the changelog draft, bump the version everywhere, and create a
tagged release commit. When the tag is pushed, `.github/workflows/release.yml`
builds installers on all four targets and attaches them to a **published**
GitHub Release (it does not create a draft, and it does not bump the
version — the tag alone changes nothing).

Adapted from Voicebox's `release-bump` skill
(github.com/jamiepine/voicebox, MIT) for ResearchAI Workspace's version
files and release workflow.

## Prerequisites

- `gh` CLI authenticated (`gh auth status`).
- Working tree clean except for the `CHANGELOG.md` draft (if the
  `[Unreleased]` section is empty, run `draft-release-notes` first).

## Workflow

1. **Check state:**

   ```bash
   git status --porcelain
   grep '^## \[Unreleased\]' -A 2 CHANGELOG.md
   ```

2. **Determine the bump level** (`patch` | `minor` | `major`). Ask the
   user if not specified. Current version: `grep -m1 version package.json`.

3. **Stamp the changelog.** Replace the `## [Unreleased]` body with an
   empty placeholder and insert the stamped section immediately after:

   ```markdown
   ## [Unreleased]

   ## [X.Y.Z] - YYYY-MM-DD

   <the content that was in [Unreleased]>
   ```

   Update the reference links at the bottom: `[Unreleased]` now compares
   against the new tag; add a `[X.Y.Z]` link against the previous tag.

4. **Bump the version in all 7 files** (there is no bumpversion here —
   edit each explicitly):

   | File | Field |
   |---|---|
   | `apps/desktop/src-tauri/tauri.conf.json` | `version` (names the release assets!) |
   | `apps/desktop/src-tauri/Cargo.toml` | `package.version` |
   | `apps/desktop/src-tauri/Cargo.lock` | via step 5 |
   | `apps/desktop/package.json` | `version` |
   | `package.json` (root) | `version` |
   | `packages/shared-types/package.json` | `version` |
   | `services/document-engine/pyproject.toml` | `version` |

5. **Refresh the lockfile and reinstall:**

   ```bash
   cd apps/desktop/src-tauri && cargo update -p researchai --precise X.Y.Z
   cd <repo root> && pnpm install --frozen-lockfile=false
   ```

6. **Verify before committing** (battery must be fully green):

   ```bash
   pnpm exec tsc --noEmit        # in apps/desktop
   pnpm build
   cargo test --manifest-path apps/desktop/src-tauri/Cargo.toml
   ```

7. **Commit and tag:**

   ```bash
   git add -u && git add CHANGELOG.md pnpm-lock.yaml
   git commit -m "Post-plan: bump workspace version to X.Y.Z for the next release"
   git tag vX.Y.Z
   ```

8. **Push branch and tag together:**

   ```bash
   git push origin main --follow-tags
   ```

   The tag push triggers `release.yml`. **Watch the run**: the workflow
   has `concurrency: release-<ref>` with `cancel-in-progress: false`, so a
   stale queued run blocks the new one — if the run sits `pending`, find
   and cancel the stale run: `gh run list --workflow=release.yml`, then
   `gh run cancel <id>`.

9. **Confirm assets** carry the new version name (they are built from
   `tauri.conf.json`), e.g. `ResearchAI.Workspace_0.1.2_aarch64.dmg`.

## Error Recovery

- **Wrongly-named assets already published** (version mismatch): delete
  the bad cut before re-tagging —
  `gh release delete vX.Y.Z --yes && git push origin :refs/tags/vX.Y.Z && git tag -d vX.Y.Z`,
  fix versions, re-commit, re-tag, re-push. Cancel the old run first to
  free the concurrency lock.
- **Tag pushed with a version typo**: same deletion sequence; never move
  a tag that CI has already built from.
- Never amend a release commit that has been pushed.

## Notes

- Do NOT push unless the user asks. Report the tag and suggest
  `git push origin main --follow-tags`.
- The release is published immediately (isDraft: false) — a pushed tag IS
  the release. Treat tagging as the point of no return.
