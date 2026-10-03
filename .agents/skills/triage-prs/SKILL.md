---
name: triage-prs
description: Use this skill to triage the open PR queue before a release. Classifies every open PR into must-merge, candidate, superseded, or deferred; writes a working triage doc; and runs the merge loop end-to-end in a single focused session.
---

# Triage PRs

## Goal

Turn a backlog of open PRs into a shipped set of merges in a single
focused session: a tracked, resumable plan (`<VERSION>_PR_TRIAGE.md`),
then work it — rebase where needed, merge in isolation-safe batches, and
close superseded PRs with credit.

Adapted from Voicebox's `triage-prs` skill
(github.com/jamiepine/voicebox, MIT). Note: ResearchAI Workspace currently
develops direct-to-main, so this skill activates only when an external
contributor PR queue actually exists (issues are enabled; PRs are not yet
part of the flow).

Pairs with `draft-release-notes` (regenerate after merges) and
`release-bump` (cut after that).

## Prerequisites

- `gh` CLI authenticated against this repo.
- A dedicated worktree for PR review — never checkout contributor branches
  on `main`:
  ```bash
  git worktree add ../researchai-pr-review -b pr-review-<VERSION> main
  ```
- Target version decided (the triage doc is named after it).

## Workflow

1. **Gather metadata:**
   ```bash
   gh pr list --state open --limit 50 --json \
     number,title,author,isDraft,mergeable,mergeStateStatus,files,additions,deletions,maintainerCanModify \
     --jq '.[] | {num: .number, title, author: .author.login, mergeable, state: .mergeStateStatus, changes: "+\(.additions)/-\(.deletions)", files: [.files[].path]}'
   ```
   `mergeable: UNKNOWN` right after a push is normal — try the merge.

2. **Classify into tiers:**
   - **Tier 1 — Merge**: small, clean, fixes a real bug, low review cost.
   - **Tier 2 — Candidate**: 50–200 lines, sound-looking, needs a read.
   - **Supersede**: already covered by something merged — compare actual
     diffs, similar titles are not proof.
   - **Defer**: big features, dirty conflicts, drafts, anything risky for
     the release pipeline.

3. **Write `<VERSION>_PR_TRIAGE.md`** in the review worktree with a
   Progress header (your scoreboard), tier tables, and an order of
   attack. Update it after **every** action — it is the session state.

4. **Per PR:**
   ```bash
   cd ../researchai-pr-review && gh pr checkout <N>
   git show HEAD            # the PR's actual changes
   git show --stat HEAD
   ```
   - **Never review via `git diff main..HEAD` on a stale branch** — the
     diff includes every commit main gained since the fork as deletions.
   - Rebase before squash-merging (`git fetch origin main && git rebase
     origin/main`): the squash computes `diff(PR-head, merge-base)`, so a
     stale branch reverts in-between work.
   - Push the rebase back to the contributor's fork when
     `maintainerCanModify` is true (keeps GitHub's UI honest).
   - `gh pr merge <N> --squash`, then record `✅ merged <sha>` in the doc.

5. **Batch tiny fixes** (≤5 lines, clean CI, non-overlapping files) in one
   loop; verify each landed with `gh pr view <N> --json state,mergeCommit`.

6. **Close superseded** with a credit-pointing comment:
   `gh pr close <N> --comment "Closing — superseded by merged #<M> ..."`.

7. **Partial-apply** when a PR bundles good and questionable changes:
   `git checkout <pr-sha> -- <file>`, adjust, commit with a
   `Co-Authored-By: <author>` trailer, then close explaining what was
   kept vs dropped.

8. **When done**: every PR has a terminal status, progress shows N/N —
   run `draft-release-notes` against the new main, then `release-bump`.

## Gotchas

- **`main..HEAD` on a stale branch lies.** Review `git show HEAD`.
- **Squash-merging an unrebased branch reverts in-between work.**
- **Dependency version floors constrain what you can apply** — cherry-pick
  half a PR rather than all of it when a kwarg/API needs a newer pin.
- **Credit contributors on partial-applies** (`Co-Authored-By:` trailers).
- **Don't let perfect be the enemy of shipped** — flag known issues, file
  follow-ups, merge the fix.

## Notes

- The triage doc is the session state; lose it and you lose the session.
- Delete the doc after the release ships, or keep it as history.
- Direct-to-main commits remain the default for this repo — this skill is
  for when a real inbound PR queue exists.
