#!/usr/bin/env bash
# Extract the stamped `## [X.Y.Z]` section of CHANGELOG.md for a release.
#
#   usage: extract-notes.sh <version|tag> [changelog-path]
#   e.g.:  extract-notes.sh v0.1.1   |   extract-notes.sh 0.1.1
#
# Prints the section body (heading excluded) to stdout. Empty output means
# the changelog has no stamped section for that version — callers fall back
# to a generic body rather than publishing an empty release (v0.1.1 did).
#
# Used by .github/workflows/release.yml (real releases) and
# .github/workflows/release-dry-run.yml (rehearsal), so a logic change is
# tested by the dry-run automatically.
set -euo pipefail

VER="${1:?usage: extract-notes.sh <version|tag> [changelog-path]}"
FILE="${2:-CHANGELOG.md}"
VER="${VER#v}" # accept `v0.1.1` as well as `0.1.1`

test -f "$FILE" || { echo "extract-notes: $FILE not found" >&2; exit 2; }

awk -v ver="$VER" '
  # A new `## [` heading ends the section (either the next release or,
  # for the last section, we stop below at the link footer).
  /^## \[/ {
    if (found) exit
    if (index($0, "## [" ver "]") == 1) { found = 1; next }
  }
  # Reference-link footer (`[0.1.1]: https://…`) ends the last section so
  # future-version links never leak into a release body.
  found && /^\[[^]]+\]:/ { exit }
  found { print }
' "$FILE"
