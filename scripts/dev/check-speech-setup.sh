#!/usr/bin/env bash
# ResearchAI Workspace — speech setup checker.
#
# Verifies that a real Piper (TTS) and whisper.cpp (STT) installation works
# using the *exact* CLI invocations the app uses (see
# apps/desktop/src-tauri/src/services/tts.rs and transcription.rs):
#
#   piper --model <onnx> --length-scale <s> --output_file <wav>   (text on stdin)
#   whisper-cli -m <ggml> -f <16k-mono.wav> -oj -of <base>        (+ -l <lang>)
#
# A green run here means the paths entered in Settings → Speech are correct
# and the Audio tab will work. Everything runs offline; nothing is downloaded.
#
# Usage:
#   scripts/dev/check-speech-setup.sh [--piper PATH] [--voice PATH]
#       [--whisper PATH] [--model PATH] [--out DIR] [--keep]
#       [--skip-tts] [--skip-stt]
#
# Environment overrides: PIPER_PATH, PIPER_VOICE, WHISPER_CLI, WHISPER_MODEL,
# WHISPER_LANG ("auto" or an ISO code like "en"; default auto).
#
# Exit code: 0 when every requested check passed (skips don't count), 1 when
# any requested check failed.

set -uo pipefail

# ---------------------------------------------------------------- presets ---

if [ -t 1 ]; then
  C_G=$'\033[32m'; C_R=$'\033[31m'; C_Y=$'\033[33m'; C_B=$'\033[36m'; C_0=$'\033[0m'
else
  C_G=""; C_R=""; C_Y=""; C_B=""; C_0=""
fi

PASS=0; FAILS=0; SKIPS=0
ok()   { PASS=$((PASS + 1)); printf '%s[ok]%s   %s\n'   "$C_G" "$C_0" "$1"; }
fail() { FAILS=$((FAILS + 1)); printf '%s[FAIL]%s %s\n' "$C_R" "$C_0" "$1"; }
warn() { printf '%s[warn]%s %s\n' "$C_Y" "$C_0" "$1"; }
skip() { SKIPS=$((SKIPS + 1)); printf '%s[skip]%s %s\n' "$C_B" "$C_0" "$1"; }
info() { printf '        %s\n' "$1"; }

need_py=0
have_python3=0
command -v python3 >/dev/null 2>&1 && have_python3=1

# Find a tool: $1 = explicit path or empty. Echoes the resolved path, or
# returns 1. Well-known Homebrew / pip locations are probed as a fallback.
find_tool() {
  local p="$1" name="$2" cand
  if [ -n "$p" ]; then
    [ -x "$p" ] && { printf '%s' "$p"; return 0; }
    return 1
  fi
  if cand="$(command -v "$name" 2>/dev/null)" && [ -n "$cand" ]; then
    printf '%s' "$cand"; return 0
  fi
  for cand in "/opt/homebrew/bin/$name" "/usr/local/bin/$name" \
              "$HOME/.local/bin/$name"; do
    [ -x "$cand" ] && { printf '%s' "$cand"; return 0; }
  done
  return 1
}

# Trim helper for subprocess stderr.
trim() { local s; s="$(printf '%s' "$1" | tr -d '\r' | sed -e 's/[[:space:]]*$//' -e 's/^[[:space:]]*//')"; printf '%s' "${s:0:400}"; }

# ------------------------------------------------------------- arguments ---

PIPER="${PIPER_PATH:-}"; VOICE="${PIPER_VOICE:-}"
WHISPER="${WHISPER_CLI:-}"; MODEL="${WHISPER_MODEL:-}"
OUT_DIR=""; KEEP=0; DO_TTS=1; DO_STT=1; WHISPER_LANG="${WHISPER_LANG:-auto}"

while [ $# -gt 0 ]; do
  case "$1" in
    --piper)   PIPER="${2:-}"; shift 2 ;;
    --voice)   VOICE="${2:-}"; shift 2 ;;
    --whisper) WHISPER="${2:-}"; shift 2 ;;
    --model)   MODEL="${2:-}"; shift 2 ;;
    --out)     OUT_DIR="${2:-}"; shift 2 ;;
    --keep)    KEEP=1; shift ;;
    --skip-tts) DO_TTS=0; shift ;;
    --skip-stt) DO_STT=0; shift ;;
    -h|--help) sed -n '2,25p' "$0"; exit 0 ;;
    *) fail "unknown argument: $1 (see --help)"; exit 1 ;;
  esac
done

if [ -z "$OUT_DIR" ]; then
  OUT_DIR="$(mktemp -d "${TMPDIR:-/tmp}/researchai-speech-check.XXXXXX")"
else
  mkdir -p "$OUT_DIR"
fi
if [ "$KEEP" -eq 0 ]; then
  trap 'rm -rf "$OUT_DIR"' EXIT
else
  info "keeping scratch files in $OUT_DIR"
fi

echo "ResearchAI Workspace — speech setup check"
info "scratch: $OUT_DIR"

# ------------------------------------------------------- TTS: Piper voice ---

tts_ok=0
if [ "$DO_TTS" -eq 0 ]; then
  skip "TTS (Piper) — skipped by request"
elif [ -z "$VOICE" ]; then
  fail "Piper voice model not set. Pass --voice /path/to/voice.onnx (or set PIPER_VOICE)."
  info "Voices (.onnx + .onnx.json pairs) come from the rhasspy/piper-voices"
  info "collection on Hugging Face — see docs/SPEECH_SETUP.md."
elif [ ! -f "$VOICE" ]; then
  fail "Piper voice model not found: $VOICE"
else
  if PIPER_RESOLVED="$(find_tool "$PIPER" piper)"; then
    need_py=1
    info "piper binary : $PIPER_RESOLVED"
    info "voice model  : $VOICE"
    if [ ! -f "${VOICE%.onnx}.onnx.json" ] && [ ! -f "$VOICE.json" ]; then
      warn "no .onnx.json beside the voice — Piper needs its config there too."
    fi
    OUT_WAV="$OUT_DIR/tts-check.wav"
    rm -f "$OUT_WAV"
    ERR="$(printf 'Setup check for ResearchAI Workspace.' \
      | "$PIPER_RESOLVED" --model "$VOICE" --length-scale 1 \
        --output_file "$OUT_WAV" 2>&1 || true)"
    if [ ! -s "$OUT_WAV" ]; then
      fail "piper produced no audio."
      e="$(trim "$ERR")"; [ -n "$e" ] && info "piper said: $e"
    elif [ "$have_python3" -eq 1 ]; then
      if META="$(python3 scripts/dev/_wav_validate.py "$OUT_WAV" 2>&1)"; then
        tts_ok=1
        ok "piper rendered a valid WAV ($META)"
      else
        fail "piper output is not a readable PCM WAV: $(trim "$META")"
      fi
    else
      tts_ok=1
      warn "python3 not found — skipped strict WAV validation (file is non-empty)."
      ok "piper produced audio (${OUT_WAV})"
    fi
  else
    fail "piper binary not found (tried \$PIPER_PATH, PATH, /opt/homebrew/bin, /usr/local/bin, ~/.local/bin)."
    info "Install it with \`brew install piper\` or \`pip install piper-tts\` — see docs/SPEECH_SETUP.md."
  fi
fi

# ------------------------------------------------------------- macOS say ---

if [ "$(uname -s)" = "Darwin" ]; then
  if say -v ? >/dev/null 2>&1; then
    info "macOS \`say\` fallback is available (Settings → Speech → provider: macos-say)."
  else
    warn "macOS \`say\` not responding — the Piper path above remains the default."
  fi
fi

# ---------------------------------------------------------------- ffmpeg ---

if command -v ffmpeg >/dev/null 2>&1; then
  ok "ffmpeg on PATH — MP3 export and audio conversion are available."
else
  warn "ffmpeg not on PATH — MP3 export stays WAV and non-16 kHz inputs cannot be converted."
  info "Optional: \`brew install ffmpeg\`."
fi

# --------------------------------------------------- STT: whisper.cpp CLI ---

stt_ok=0
if [ "$DO_STT" -eq 0 ]; then
  skip "STT (whisper.cpp) — skipped by request"
elif [ -z "$MODEL" ]; then
  fail "Whisper model not set. Pass --model /path/to/ggml-base.bin (or set WHISPER_MODEL)."
  info "GGML models ship from the ggml-org whisper.cpp collection on Hugging Face"
  info "— see docs/SPEECH_SETUP.md."
elif [ ! -f "$MODEL" ]; then
  fail "Whisper model not found: $MODEL"
else
  if WHISPER_RESOLVED="$(find_tool "$WHISPER" whisper-cli)" || WHISPER_RESOLVED="$(find_tool "" main)"; then
    need_py=1
    info "whisper binary: $WHISPER_RESOLVED"
    info "whisper model : $MODEL"

    # Synthesize 1 s of 16 kHz mono 16-bit PCM silence in pure shell — the
    # exact native input the app's sniff_wav() requires (rate 16000, ch 1).
    STT_WAV="$OUT_DIR/stt-check.wav"
    printf 'RIFF' > "$STT_WAV"
    printf '\x24\x7d\x00\x00' >> "$STT_WAV"   # 36 + 32000
    printf 'WAVEfmt ' >> "$STT_WAV"
    printf '\x10\x00\x00\x00' >> "$STT_WAV"   # fmt chunk = 16 bytes
    printf '\x01\x00' >> "$STT_WAV"           # PCM
    printf '\x01\x00' >> "$STT_WAV"           # mono
    printf '\x80\x3e\x00\x00' >> "$STT_WAV"   # 16000 Hz
    printf '\x00\x7d\x00\x00' >> "$STT_WAV"   # 32000 B/s
    printf '\x02\x00' >> "$STT_WAV"           # block align
    printf '\x10\x00' >> "$STT_WAV"           # 16 bits
    printf 'data' >> "$STT_WAV"
    printf '\x00\x7d\x00\x00' >> "$STT_WAV"   # 32000 bytes of payload
    head -c 32000 /dev/zero >> "$STT_WAV"

    rm -f "$OUT_DIR/stt-check.json"
    LANG_ARGS=()
    if [ -n "$WHISPER_LANG" ] && [ "$WHISPER_LANG" != "auto" ]; then
      LANG_ARGS=(-l "$(printf '%s' "$WHISPER_LANG" | tr '[:upper:]' '[:lower:]')")
    fi
    ERR="$( (cd "$OUT_DIR" && "$WHISPER_RESOLVED" -m "$MODEL" -f "$STT_WAV" \
      -oj -of "$OUT_DIR/stt-check" ${LANG_ARGS[@]+"${LANG_ARGS[@]}"} 2>&1) || true)"
    JSON="$OUT_DIR/stt-check.json"
    if [ ! -f "$JSON" ]; then
      fail "whisper-cli produced no JSON (-oj) output."
      e="$(trim "$ERR")"; [ -n "$e" ] && info "whisper said: $e"
    elif [ "$have_python3" -eq 1 ]; then
      if META="$(python3 scripts/dev/_stt_json_validate.py "$JSON" 2>&1)"; then
        stt_ok=1
        ok "whisper-cli transcribed and emitted valid JSON ($META)"
        info "(an empty transcription is expected — the probe input is silence.)"
      else
        fail "whisper JSON is not parseable in the app's schema: $(trim "$META")"
      fi
    else
      stt_ok=1
      warn "python3 not found — skipped JSON schema validation."
      ok "whisper-cli produced JSON output (${JSON})"
    fi
  else
    fail "whisper-cli not found (tried \$WHISPER_CLI, PATH, /opt/homebrew/bin, /usr/local/bin, ~/.local/bin)."
    info "Install with \`brew install whisper-cpp\` or build whisper.cpp — see docs/SPEECH_SETUP.md."
  fi
fi

# -------------------------------------------------------------- summary ---

echo
echo "─────────────────────────────────────────────"
if [ "$tts_ok" -eq 1 ]; then
  info "TTS: ready — set this binary + voice in Settings → Speech."
fi
if [ "$stt_ok" -eq 1 ]; then
  info "STT: ready — set this binary + model in Settings → Speech."
fi
printf 'Summary: %d passed, %d failed, %d skipped\n' "$PASS" "$FAILS" "$SKIPS"
if [ "$need_py" -eq 1 ] && [ "$have_python3" -eq 0 ]; then
  warn "python3 was unavailable, so output validation was skipped (macOS: \`xcode-select --install\`)."
fi
[ "$FAILS" -eq 0 ] || exit 1
exit 0
