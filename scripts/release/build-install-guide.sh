#!/usr/bin/env bash
# Build the per-OS "Download / Install" section shown at the top of every
# release page. Filenames come from SHA256SUMS (so the guide can never
# link an asset that isn't published); install steps are static text
# matched to each operating system's real-world quirks (Gatekeeper,
# SmartScreen, FUSE/glibc).
#
# Anchoring notes (verified against github.com release pages in a real
# browser):
#   - Release-body headings render WITHOUT ids, so explicit
#     `<a id="…"></a>` anchors are emitted before each heading.
#   - GitHub's sanitizer namespaces those ids to `user-content-…` but
#     does NOT rewrite fragment hrefs, so nav links carry the prefix.
#   - Fragment-only hrefs are intercepted on trusted clicks; full-URL
#     links (page URL + fragment) navigate reliably for real users.
#
# Usage: build-install-guide.sh <SHA256SUMS> <base-download-url> <out.md>
#   e.g. build-install-guide.sh SHA256SUMS \
#        https://github.com/owner/repo/releases/download/v0.1.6 out.md
set -euo pipefail

die() { echo "::error::build-install-guide: $*" >&2; exit 1; }

[ $# -eq 3 ] || die "usage: build-install-guide.sh <SHA256SUMS> <base-url> <out.md>"
SUMS="$1" BASE="$2" OUT="$3"
[ -f "$SUMS" ] || die "SHA256SUMS not found: $SUMS"
[ "$(grep -c . "$SUMS")" -eq 5 ] || die "expected exactly 5 entries in SHA256SUMS"

# The release page URL, derived from the download base
# (.../releases/download/vX.Y.Z -> .../releases/tag/vX.Y.Z), plus the
# owner/repo for the user-guide link.
PAGE=$(printf '%s' "$BASE" | sed 's|/download/|/tag/|')
[ "$PAGE" != "$BASE" ] || die "base URL does not contain /download/: $BASE"
REPO=$(printf '%s' "$BASE" | sed -E 's|https://github.com/([^/]+/[^/]+)/.*|\1|')
[ "$REPO" != "$BASE" ] || die "cannot derive owner/repo from $BASE"

pick() { # <filename regex> -> filename
  local f
  f=$(awk -v re="$1" '$2 ~ re { print $2 }' "$SUMS")
  [ -n "$f" ] || die "no asset matching $1 in $SUMS"
  printf '%s' "$f"
}

ARM64=$(pick '_aarch64\.dmg$')
X64DMG=$(pick '_x64\.dmg$')
EXE=$(pick '_x64-setup\.exe$')
APPIMAGE=$(pick '\.AppImage$')
DEB=$(pick '\.deb$')

VERSION=$(printf '%s' "$X64DMG" | sed -E 's/.*_([0-9]+\.[0-9]+\.[0-9]+)_x64\.dmg$/\1/')
[[ "$VERSION" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || die "could not parse a version from $X64DMG"

# Quoted heredoc + sed placeholders: the guide text contains backticks and
# command examples that an interpolated heredoc would execute.
sed \
  -e "s|{{BASE}}|$BASE|g" \
  -e "s|{{PAGE}}|$PAGE|g" \
  -e "s|{{REPO}}|$REPO|g" \
  -e "s|{{ARM64}}|$ARM64|g" \
  -e "s|{{X64DMG}}|$X64DMG|g" \
  -e "s|{{EXE}}|$EXE|g" \
  -e "s|{{APPIMAGE}}|$APPIMAGE|g" \
  -e "s|{{DEB}}|$DEB|g" \
  -e "s|{{VERSION}}|$VERSION|g" \
  > "$OUT" <<'EOF'
<!-- researchai-install-guide -->
**On this page:** [Download]({{PAGE}}#user-content-download) · [Install]({{PAGE}}#user-content-install) · [Release notes]({{PAGE}}#user-content-release-notes) · [SHA-256 checksums]({{PAGE}}#user-content-sha-256-checksums)

<a id="download"></a>

## Download

One click for your operating system:

| Your system | Download |
| --- | --- |
| **macOS — Apple Silicon** (M1–M4) | [{{ARM64}}]({{BASE}}/{{ARM64}}) |
| **macOS — Intel** | [{{X64DMG}}]({{BASE}}/{{X64DMG}}) |
| **Windows 10/11 (64-bit)** | [{{EXE}}]({{BASE}}/{{EXE}}) |
| **Linux — any distro (AppImage)** | [{{APPIMAGE}}]({{BASE}}/{{APPIMAGE}}) |
| **Linux — Debian/Ubuntu (.deb)** | [{{DEB}}]({{BASE}}/{{DEB}}) |

*Which Mac do you have?* Apple menu → **About This Mac → Chip**: Apple Silicon uses the `aarch64` file, Intel uses the `x64` file.

<a id="install"></a>

## Install

**macOS**

1. Open the `.dmg` and drag **ResearchAI Workspace** onto the Applications alias.
2. First launch: **right-click the app → Open → Open**. The build is unsigned, so Gatekeeper asks once; afterwards it opens normally.

**Windows**

1. Run `{{EXE}}` and choose *any user* or *all users*.
2. If Windows SmartScreen warns, click **More info → Run anyway** — the app is unsigned but built from the source in this repository.

**Linux**

- **AppImage (any distro):**
  `chmod +x {{APPIMAGE}} && ./{{APPIMAGE}}`
  (needs FUSE — on newer Ubuntu: `sudo apt install fuse2`.)
- **Debian/Ubuntu:**
  `sudo apt install ./{{DEB}}`
  (glibc ≥ 2.38, i.e. Ubuntu 24.04+), then launch **ResearchAI Workspace** from your app menu.

New to the app? The full walkthrough lives in
[`docs/USER_GUIDE.md`](https://github.com/{{REPO}}/blob/main/docs/USER_GUIDE.md).
Verify any download against the [SHA-256 checksums]({{PAGE}}#user-content-sha-256-checksums) below.
EOF

echo "wrote $OUT (version $VERSION, page $PAGE, 5 assets)"
