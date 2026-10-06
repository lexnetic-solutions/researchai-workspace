#!/usr/bin/env bash
# Build the static Tauri updater manifest (latest.json) for a release.
#
# `tauri build` (with bundle.createUpdaterArtifacts) writes each updater
# artefact next to its installer together with a `<file>.sig`. Installed apps
# poll
#
#   https://github.com/<repo>/releases/latest/download/latest.json
#
# and expect { version, pub_date, platforms } where every platform entry's
# `signature` is the literal single-line base64 content of the `.sig` file
# (the plugin base64-decodes it verbatim — re-encoding or wrapping breaks
# verification).
#
# Each artefact is listed twice: once under the installer-suffixed target the
# plugin tries first (`darwin-aarch64-app`, `linux-x86_64-appimage`, …) and
# once under the plain `{os}-{arch}` fallback, so every install flavour of a
# platform that ships one installer resolves to the right artefact.
#
# Usage:
#   make-updater-manifest.sh --tag v0.1.6 --repo owner/name \
#       --artifacts DIR --out latest.json [--notes FILE] [--pub-date RFC3339]
#   make-updater-manifest.sh --self-test
set -euo pipefail

die() { echo "::error::make-updater-manifest: $*" >&2; exit 1; }

usage() {
  sed -n '2,20p' "$0" | sed 's/^# \{0,1\}//'
}

TAG="" REPO="" ARTIFACTS="" NOTES="" PUB_DATE="" OUT="" SELF_TEST=0

while [ $# -gt 0 ]; do
  case "$1" in
    --tag)       TAG="$2"; shift 2 ;;
    --repo)      REPO="$2"; shift 2 ;;
    --artifacts) ARTIFACTS="$2"; shift 2 ;;
    --notes)     NOTES="$2"; shift 2 ;;
    --pub-date)  PUB_DATE="$2"; shift 2 ;;
    --out)       OUT="$2"; shift 2 ;;
    --self-test) SELF_TEST=1; shift ;;
    -h|--help)   usage; exit 0 ;;
    *)           die "unknown argument: $1" ;;
  esac
done

# ---------------------------------------------------------------------------
# Core builder — expects TAG/REPO/ARTIFACTS/OUT to be set.
# ---------------------------------------------------------------------------

find_artifact() { # <glob> -> path (recursive; CI downloads flatten into dirs)
  local found
  found=$(find "$ARTIFACTS" -type f -name "$1" -print -quit)
  [ -n "$found" ] || die "missing updater artefact matching '$1' under $ARTIFACTS"
  printf '%s' "$found"
}

read_sig() { # <artifact path> -> signature string
  local sigfile="$1.sig"
  [ -f "$sigfile" ] || die "missing signature file: $sigfile"
  # Strip any line wrapping: the plugin wants the whole payload as one
  # base64 string with no newlines.
  tr -d '\n\r' < "$sigfile"
}

release_url() { # <artifact path> -> percent-encoded download URL
  local name enc
  name=$(basename "$1")
  enc=$(jq -rn --arg v "$name" '$v | @uri')
  printf 'https://github.com/%s/releases/download/%s/%s' "$REPO" "$TAG" "$enc"
}

build_manifest() {
  [ -n "$TAG" ] || die "--tag is required"
  [ -n "$REPO" ] || die "--repo is required"
  [ -n "$ARTIFACTS" ] || die "--artifacts is required"
  [ -n "$OUT" ] || die "--out is required"
  [ -d "$ARTIFACTS" ] || die "artifacts dir not found: $ARTIFACTS"

  local version="${TAG#v}"
  [[ "$version" =~ ^[0-9]+\.[0-9]+\.[0-9]+([-.][0-9A-Za-z.-]+)?$ ]] \
    || die "tag '$TAG' does not yield a semver version"
  local pub_date="${PUB_DATE:-$(date -u +%Y-%m-%dT%H:%M:%SZ)}"
  local notes_text=""
  if [ -n "$NOTES" ]; then
    [ -f "$NOTES" ] || die "notes file not found: $NOTES"
    notes_text=$(cat "$NOTES")
  fi

  local arm64 x64 win lin
  arm64=$(find_artifact '*_aarch64.app.tar.gz')
  x64=$(find_artifact '*_x64.app.tar.gz')
  win=$(find_artifact '*_x64-setup.exe')
  lin=$(find_artifact '*_amd64.AppImage')

  local arm64_url arm64_sig x64_url x64_sig win_url win_sig lin_url lin_sig
  arm64_url=$(release_url "$arm64"); arm64_sig=$(read_sig "$arm64")
  x64_url=$(release_url "$x64");     x64_sig=$(read_sig "$x64")
  win_url=$(release_url "$win");     win_sig=$(read_sig "$win")
  lin_url=$(release_url "$lin");     lin_sig=$(read_sig "$lin")

  jq -n \
    --arg version "$version" \
    --arg notes "$notes_text" \
    --arg pub_date "$pub_date" \
    --arg arm64_url "$arm64_url" --arg arm64_sig "$arm64_sig" \
    --arg x64_url "$x64_url"     --arg x64_sig "$x64_sig" \
    --arg win_url "$win_url"     --arg win_sig "$win_sig" \
    --arg lin_url "$lin_url"     --arg lin_sig "$lin_sig" \
    '{
      version: $version,
      notes: $notes,
      pub_date: $pub_date,
      platforms: {
        "darwin-aarch64-app":   { url: $arm64_url, signature: $arm64_sig },
        "darwin-aarch64":       { url: $arm64_url, signature: $arm64_sig },
        "darwin-x86_64-app":    { url: $x64_url,   signature: $x64_sig },
        "darwin-x86_64":        { url: $x64_url,   signature: $x64_sig },
        "windows-x86_64-nsis":  { url: $win_url,   signature: $win_sig },
        "windows-x86_64":       { url: $win_url,   signature: $win_sig },
        "linux-x86_64-appimage": { url: $lin_url,  signature: $lin_sig },
        "linux-x86_64":         { url: $lin_url,   signature: $lin_sig }
      }
    }' > "$OUT"

  echo "wrote $OUT (version $version, 8 platform entries)"
}

# ---------------------------------------------------------------------------
# Self-test: fixture artefacts → manifest → structural + format assertions.
# This is what CI rehearses so a broken manifest never reaches a release.
# ---------------------------------------------------------------------------

# Global so the EXIT trap can still see it after self_test returns.
SELF_TEST_TMP=""

self_test() {
  command -v jq >/dev/null || die "jq is required"
  SELF_TEST_TMP=$(mktemp -d)
  local tmp="$SELF_TEST_TMP"
  trap 'rm -rf "$SELF_TEST_TMP"' EXIT

  TAG="v9.9.9" REPO="example/repo" ARTIFACTS="$tmp" OUT="$tmp/latest.json"
  NOTES="$tmp/notes.md"
  echo "Fixture release notes." > "$NOTES"

  local f
  for f in \
    "ResearchAI.Workspace_9.9.9_aarch64.app.tar.gz" \
    "ResearchAI.Workspace_9.9.9_x64.app.tar.gz" \
    "ResearchAI.Workspace_9.9.9_x64-setup.exe" \
    "ResearchAI.Workspace_9.9.9_amd64.AppImage"; do
    echo "fixture bytes for $f" > "$tmp/$f"
    # Single-line base64, exactly like `tauri signer sign` writes.
    printf 'minisign fixture payload: %s' "$f" | base64 | tr -d '\n' > "$tmp/$f.sig"
  done

  build_manifest

  jq -e '.version == "9.9.9"' "$OUT" >/dev/null || die "self-test: version mismatch"
  jq -e '.notes == "Fixture release notes."' "$OUT" >/dev/null || die "self-test: notes mismatch"
  jq -e '.pub_date | test("^[0-9]{4}-[0-9]{2}-[0-9]{2}T[0-9]{2}:[0-9]{2}:[0-9]{2}Z$")' \
    "$OUT" >/dev/null || die "self-test: pub_date is not RFC 3339 UTC"

  local key
  for key in \
    darwin-aarch64-app darwin-aarch64 \
    darwin-x86_64-app darwin-x86_64 \
    windows-x86_64-nsis windows-x86_64 \
    linux-x86_64-appimage linux-x86_64; do
    jq -e --arg k "$key" '.platforms[$k].url
      | startswith("https://github.com/example/repo/releases/download/v9.9.9/")' \
      "$OUT" >/dev/null || die "self-test: bad url for $key"
    jq -e --arg k "$key" '.platforms[$k].signature | length > 0' \
      "$OUT" >/dev/null || die "self-test: empty signature for $key"
  done

  # The plugin base64-decodes the signature field, then minisign-decodes the
  # result — assert that first layer still yields our fixture payload.
  local decoded
  decoded=$(jq -r '.platforms["darwin-aarch64"].signature' "$OUT" \
    | openssl base64 -d -A)
  case "$decoded" in
    "minisign fixture payload: "*) ;;
    *) die "self-test: signature is not plain base64 of the .sig content" ;;
  esac

  # URL encoding: no raw spaces may survive in any platform URL.
  if jq -r '.platforms[].url' "$OUT" | grep -q ' '; then
    die "self-test: unencoded space in a platform URL"
  fi

  echo "self-test OK"
}

if [ "$SELF_TEST" = 1 ]; then
  command -v jq >/dev/null || die "jq is required"
  self_test
else
  command -v jq >/dev/null || die "jq is required"
  build_manifest
fi
