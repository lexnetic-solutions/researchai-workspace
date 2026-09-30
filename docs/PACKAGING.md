# Packaging & Distribution

## Targets (spec §37)

| Platform | Artefacts |
|---|---|
| macOS Apple Silicon | `ResearchAI-macOS-arm64.dmg` (+ `.app`) |
| macOS Intel | `ResearchAI-macOS-x64.dmg` |
| Windows x64 | `ResearchAI-Windows-x64.exe` (NSIS) + portable `.zip` |
| Linux x64 | `ResearchAI-Linux-x64.AppImage` (+ `.deb`) |

One source tree; per-OS builds in CI — binaries are never cross-compiled
between desktop OSes (spec §38).

## Bundle composition

- **Bundled:** the app itself (web assets + Rust binary), icons, licences and
  the document-engine sidecar (below).
- **Downloaded on demand (Phase 3+):** LLM/embedding/whisper models with
  checksum verification ([Settings → AI Models], spec §36) — the installer
  never ships a big model (spec §36).
- **User-installed, never bundled (spec §20/§21):** whisper.cpp (`whisper-cli`
  + GGML model), Piper (`piper` + `.onnx` voice) and ffmpeg — the app probes
  PATH/settings and gives actionable install hints.

## Document-engine sidecar (Phase 9)

The Python engine is frozen per-OS with PyInstaller and shipped inside the
app resources under `sidecar/`:

1. Freeze: `(cd services/document-engine && uv run pyinstaller packaging/pyinstaller.spec)`
2. Collect: `node scripts/package/collect-sidecar.mjs` → copies the binary
   (+ `engine.env` overrides) to `apps/desktop/src-tauri/packaging/resources/sidecar/`.
3. Bundle: `tauri build` ships it to `Contents/Resources/sidecar/` (macOS) or
   the equivalent resource dir per platform.
4. Runtime: `services::engine_runtime` spawns it at startup (unless an engine
   already answers on 127.0.0.1:8737 — the dev/screen flow), waits ≤30 s for
   `/health`, and kills the child on app exit. Missing sidecar files are a
   normal dev condition; the supervisor stays dormant.

CI (`release.yml`) builds the sidecar on all three OSes before `tauri build`.

## Local development

```bash
pnpm desktop:build     # tauri build → per-OS bundle for the current machine
```

Artefacts land in `apps/desktop/src-tauri/target/release/bundle/`.

## Release checklist (Phase 9)

1. Version bump + tag `v*` → `release.yml` builds sidecar + app for macOS
   (arm64/x64), Windows (x64) and Linux (x64: AppImage + deb), then attaches
   artefacts to the GitHub release (Linux dev builds remain supported via
   `ci.yml`).
   Before spending a real tag, rehearse the pipeline with the manual
   **Release dry-run** workflow (`.github/workflows/release-dry-run.yml`):
   Actions → Release dry-run → Run workflow. It builds the same three-OS
   matrix with ad-hoc signing, verifies the bundles + sidecar shipped, and
   uploads artefacts only — it cannot publish (`permissions: {}`).
2. macOS signing/notarization: set `APPLE_CERTIFICATE` secrets; without
   them the build is ad-hoc signed (Gatekeeper: right-click → Open).
3. First-run: the Home **Setup checklist** probes engine, local AI, speech
   in/out live (spec §48); Diagnostics covers data dir + DB migrations.
4. Backup/restore + crash-safe migrations verified against a copy of a real
   user workspace.
5. Update THIRD_PARTY_LICENSES.md from actual lockfiles.

## Icons

Platform sets are generated from `src-tauri/icons/icon.png` via
`pnpm --filter @researchai/desktop tauri icon src-tauri/icons/icon.png`
(icns/ico/png are checked in; regenerate after artwork changes).
