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
import { chmodSync, copyFileSync, existsSync, mkdirSync, rmSync, writeFileSync } from 'node:fs';
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
for (const f of ['researchai-engine', 'researchai-engine-macos', 'researchai-engine-linux', 'researchai-engine-win.exe']) {
  rmSync(join(outDir, f), { force: true });
}

function sh(cmd, cwd) {
  return spawnSync(cmd, { shell: true, cwd, encoding: 'utf8' });
}

// 1) Frozen binary? (CI builds it with PyInstaller before calling this)
const binarySources = [
  join(engineDir, 'dist/researchai-engine/researchai-engine'),
  join(engineDir, 'dist/researchai-engine-macos'),
  join(engineDir, 'dist/researchai-engine'),
];
if (mode !== 'fallback' && mode !== 'skip') {
  const src = binarySources.find(existsSync);
  if (src) {
    const dest = join(outDir, 'researchai-engine');
    copyFileSync(src, dest);
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

// 2) Fallback launcher: dev machine with `uv` on PATH.
if (mode !== 'skip') {
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
