---
name: draft-release-notes
description: Use this skill to draft or update the [Unreleased] section of CHANGELOG.md from the actual changes since the last tag. Run it at any point during development to keep a working copy of the release narrative. Does NOT bump versions or create tags.
---

# Draft Release Notes

## Goal

Update the `[Unreleased]` section at the top of `CHANGELOG.md` with a
narrative release story based on the real changes since the last tag. This
is a **non-destructive working copy** — run it as many times as you want
during development.

Adapted from Voicebox's `draft-release-notes` skill
(github.com/jamiepine/voicebox, MIT) for ResearchAI Workspace's release
flow (tag-push triggers `.github/workflows/release.yml`).

## Workflow

1. **Identify the last release tag and gather changes.**

   ```bash
   LAST_TAG=$(git tag --list "v*" --sort=-v:refname | head -n 1)
   git log --oneline "$LAST_TAG"..HEAD
   git diff --stat "$LAST_TAG"..HEAD
   ```

   GitHub's generated notes are usually thin on this repo (commits land
   directly on `main` without PRs) — the commit log and diff stat are the
   real raw material:

   ```bash
   gh api repos/:owner/:repo/releases/generate-notes \
     -f tag_name="vNEXT" \
     -f target_commitish="$(git rev-parse HEAD)" \
     -f previous_tag_name="$LAST_TAG" \
     --jq '.body'
   ```

2. **Draft the release narrative** as markdown for the `[Unreleased]`
   section. Do not include the `## [Unreleased]` heading itself — just the
   body content.

3. **Update CHANGELOG.md**: replace everything between
   `## [Unreleased]` and the next `## [` heading. Preserve the header
   comment block and all stamped release sections below. The
   `[Unreleased]` section must always exist and be first.

4. **Do NOT commit, tag, or bump versions.** Leave the file modified in
   the working tree.

## Release Story Format

```markdown
## [Unreleased]

<One strong opening paragraph: what this release is about and why it
matters. Tie it to concrete shipped changes. No vague hype.>

### <Feature/Theme Group>
- Bullet points with specifics (commands, events, files where useful)

### Bug Fixes
- ...
```

### Style Guidelines

- **Factual and specific.** Every claim must trace to a real commit —
  this repo has no PR numbers to reference, so cite subsystems and
  behaviour instead.
- **Narrative over list.** Lead with a paragraph that tells the story,
  then support with bullets.
- **Group by theme, not by commit.** Cluster related changes under
  descriptive headings (`Performance`, `Added`, `Fixed`).
- **Skip trivial chores** (typo fixes, CI tweaks) unless they are the
  bulk of the release.
- **Match the voice of existing sections** — read the `[0.1.1]` and
  `[0.1.0]` entries for tone reference.

## When There Are No Changes

If `git log "$LAST_TAG"..HEAD` is empty, leave `[Unreleased]` empty (just
the heading) and tell the user there is nothing to draft.

## Notes

- Only touch the `[Unreleased]` section; never edit stamped sections.
- The `release-bump` skill depends on this draft being current before it
  finalizes — run this first, then `release-bump`.
