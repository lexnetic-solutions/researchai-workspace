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

- **Bundled:** the app itself (web assets + Rust binary), icons, licences.
- **Downloaded on demand (Phase 3+):** LLM/embedding/whisper models with
  checksum verification ([Settings → AI Models], spec §36) — the installer
  never ships a big model (spec §36).
- **Sidecars:** the Python document engine is packaged per-OS (PyInstaller or
  embedded runtime) from Phase 1; FFmpeg/llama.cpp/whisper.cpp ship as
  prebuilt native sidecar binaries with license-compatible builds
  ([THIRD_PARTY_LICENSES.md](../THIRD_PARTY_LICENSES.md)).

## Local development

```bash
pnpm desktop:build     # tauri build → per-OS bundle for the current machine
```

Artefacts land in `apps/desktop/src-tauri/target/release/bundle/`.

## Release checklist (Phase 9)

1. Version bump + tag → CI builds all three platforms.
2. Attach artefacts to the GitHub release.
3. First-run diagnostics verify: data dir writable, DB migrates, sidecar
   healthy, models present (spec §48).
4. Backup/restore + crash-safe migrations verified against a copy of a real
   user workspace.
5. Update THIRD_PARTY_LICENSES.md from actual lockfiles.
