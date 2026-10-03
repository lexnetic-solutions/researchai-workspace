#!/usr/bin/env node
// Phase 9 packaging scaffold: assembles `apps/desktop/src-tauri/packaging/resources/sidecar/`
// for `tauri build` — a frozen PyInstaller sidecar binary (or, with
// RESEARCHAI_SIDECAR_MODE=fallback, a launcher script for dev machines with
// `uv` installed) plus `engine.env`.
//
// The Tauri build then copies this directory into the bundle resources
// (Contents/Resources/sidecar/…), where services::engine_runtime finds and
// supervises it at app startup (spec §37/§46).
//
// Usage:
//   node scripts/package/collect-sidecar.mjs              # auto (binary if present, else fallback)
//   RESEARCHAI_SIDECAR_MODE=binary|fallback|skip node …

import { spawnSync } from 'node:child_process';
import { chmodSync, copyFileSync, cpSync, existsSync, mkdirSync, rmSync, statSync, writeFileSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const root = resolve(dirname(fileURLToPath(import.meta.url)), '../..');
const engineDir = join(root, 'services/document-engine');
const outDir = join(
  root,
  'apps/desktop/src-tauri/packaging/resources/sidecar',
);
const mode = (process.env.RESEARCHAI_SIDECAR_MODE ?? 'auto').toLowerCase();

mkdirSync(outDir, { recursive: true });

// Clean previous artefacts (keep engine.env if the operator wrote one).
for (const f of [
  'researchai-engine',
  'researchai-engine.exe',
  'researchai-engine-macos',
  'researchai-engine-linux',
  'researchai-engine-win.exe',
  // PyInstaller onedir runtime (Python dylib, stdlib, libs) — must never
  // go stale next to a freshly copied entry binary.
  '_internal',
]) {
  rmSync(join(outDir, f), { recursive: true, force: true });
}

function sh(cmd, cwd) {
  return spawnSync(cmd, { shell: true, cwd, encoding: 'utf8' });
}

// 1) Frozen binary? (CI builds it with PyInstaller before calling this).
// The spec produces the single-directory layout dist/researchai-engine/ with
// the entry binary inside (researchai-engine, or .exe on Windows). Only
// regular files qualify — on Windows the bare dist/researchai-engine path
// is the COLLECT directory and copyFileSync on it fails with EPERM.
const isFile = (p) => {
  try {
    return statSync(p).isFile();
  } catch {
    return false;
  }
};
const binarySources = [
  join(engineDir, 'dist/researchai-engine/researchai-engine'),
  join(engineDir, 'dist/researchai-engine/researchai-engine.exe'),
  join(engineDir, 'dist/researchai-engine-macos'),
  join(engineDir, 'dist/researchai-engine-linux'),
];
if (mode !== 'fallback' && mode !== 'skip') {
  const src = binarySources.find(isFile);
  if (src) {
    // Windows cannot execute an extensionless file: keep the .exe suffix.
    const dest = join(
      outDir,
      process.platform === 'win32' ? 'researchai-engine.exe' : 'researchai-engine',
    );
    copyFileSync(src, dest);
    chmodSync(dest, 0o755);
    // PyInstaller >= 6 onedir layout: the entry binary needs its sibling
    // `_internal/` runtime (Python dylib, stdlib, libs) beside it — without
    // it the sidecar dies at exec with "Failed to load Python shared
    // library …/_internal/Python" on the user's machine.
    const runtime = join(dirname(src), '_internal');
    if (existsSync(runtime)) {
      cpSync(runtime, join(outDir, '_internal'), { recursive: true });
      console.log('[collect-sidecar] bundled _internal/ runtime (PyInstaller onedir)');
    }
    console.log(`[collect-sidecar] bundled frozen binary: ${src}`);
    writeEnv();
    process.exit(0);
  }
  if (mode === 'binary') {
    console.error(
      '[collect-sidecar] RESEARCHAI_SIDECAR_MODE=binary but no PyInstaller output found — build it first:\n' +
        '  (cd services/document-engine && uv run pyinstaller packaging/pyinstaller.spec)',
    );
    process.exit(1);
  }
}

// 2) Fallback launcher: dev machine with `uv` on PATH. Unix only — a
// `#!/bin/sh` script cannot be executed as a sidecar on Windows, so there
// dev builds simply run with an externally started engine.
if (mode !== 'skip' && process.platform !== 'win32') {
  const uv = sh('command -v uv');
  if (uv.status === 0) {
    const dest = join(outDir, 'researchai-engine');
    writeFileSync(
      dest,
      `#!/bin/sh\n# Dev fallback launcher (not for release): runs the engine via uv.\nexec uv run --project "${engineDir}" uvicorn researchai_document_engine.app:app --host 127.0.0.1 --port "\${PORT:-8737}"\n`,
    );
    chmodSync(dest, 0o755);
    console.log('[collect-sidecar] bundled uv fallback launcher (dev only — do not ship)');
    writeEnv();
    process.exit(0);
  }
}

console.log('[collect-sidecar] no sidecar bundled (dev mode — engine must be started externally)');
process.exit(0);

function writeEnv() {
  const envPath = join(outDir, 'engine.env');
  if (!existsSync(envPath)) {
    writeFileSync(
      envPath,
      '# ResearchAI document-engine overrides (ops escape hatch).\n# PORT=8737\n',
    );
  }
}
