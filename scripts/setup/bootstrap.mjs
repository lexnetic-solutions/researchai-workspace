#!/usr/bin/env node
/**
 * ResearchAI bootstrap (Phase 0).
 *
 * Idempotent: safe to re-run. Performs
 *   1. `pnpm install` for the workspace
 *   2. `uv sync` for the Python document engine
 *   3. stages the app icon into src-tauri/icons
 *   4. prints a toolchain status table
 */
import { execSync } from 'node:child_process';
import { cpSync, existsSync, mkdirSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import path from 'node:path';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../..');

function run(cmd, opts = {}) {
  console.log(`\x1b[2m$\x1b[0m ${cmd}`);
  execSync(cmd, { stdio: 'inherit', cwd: root, ...opts });
}

function check(name, cmd) {
  try {
    const out = execSync(cmd, { encoding: 'utf8', cwd: root }).trim().split('\n')[0];
    console.log(`  ✔ ${name.padEnd(12)} ${out}`);
    return true;
  } catch {
    console.log(`  ✘ ${name.padEnd(12)} not found`);
    return false;
  }
}

console.log('\nResearchAI Workspace — setup\n');

console.log('[1/4] Toolchain status');
const node = check('node', 'node --version');
const pnpm = check('pnpm', 'pnpm --version');
const rust = check('rustc', 'rustc --version');
const uv = check('uv', 'uv --version');

console.log('\n[2/4] Installing workspace dependencies');
if (node && pnpm) run('pnpm install');
else console.log('  skipped (node/pnpm missing)');

console.log('\n[3/4] Python document engine');
if (uv) {
  run('uv sync', { cwd: path.join(root, 'services/document-engine') });
} else {
  console.log('  skipped (uv missing — install https://docs.astral.sh/uv/)');
}

console.log('\n[4/4] App icons');
const iconSrc = path.join(root, 'apps/desktop/public/app-icon.svg');
const iconDstDir = path.join(root, 'apps/desktop/src-tauri/icons');
mkdirSync(iconDstDir, { recursive: true });
if (existsSync(iconSrc)) {
  cpSync(iconSrc, path.join(iconDstDir, 'icon.svg'));
  console.log('  staged icon.svg (placeholder artwork)');
  console.log('  note: production icons need a 1024px PNG + `pnpm tauri icon`');
}

console.log('\nDone. Next steps:');
console.log('  pnpm desktop:dev      # run the desktop app');
console.log('  pnpm engine:run       # run the document engine sidecar');
console.log('  pnpm typecheck && pnpm test');
if (!rust) {
  console.log('\n⚠ Rust is not installed — required for the desktop app.');
  console.log('  Install with: curl --proto "=https" --tlsv1.2 -sSf https://sh.rustup.rs | sh');
}
