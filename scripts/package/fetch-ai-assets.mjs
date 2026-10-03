#!/usr/bin/env node
// Fetches the AI assets that ship inside the installer so Ask-AI works
// with zero setup: the llama.cpp `llama-server` binary for this platform
// and the small starter GGUF (Qwen3-0.6B Q4_K_M, Apache-2.0).
//
// Output (bundled via tauri.conf.json `bundle.resources`):
//   apps/desktop/src-tauri/packaging/resources/llama/llama-server[.exe]
//   apps/desktop/src-tauri/packaging/resources/models/<name>.gguf
//
// Usage:
//   node scripts/package/fetch-ai-assets.mjs           # this platform + GGUF
//   SKIP_GGUF=1 node scripts/package/fetch-ai-assets.mjs
//
// Versions are pinned (reproducible releases); override for testing:
//   LLAMA_CPP_TAG, LLAMA_CPP_ASSET, QWEN_GGUF_URL

import { chmodSync, createWriteStream, existsSync, mkdirSync, readdirSync, renameSync, rmSync, statSync } from 'node:fs';
import { spawnSync } from 'node:child_process';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const root = resolve(dirname(fileURLToPath(import.meta.url)), '../..');
const outDir = join(root, 'apps/desktop/src-tauri/packaging/resources');

// --- Pinned versions -------------------------------------------------------
// llama.cpp publishes per-OS binary archives on every tagged build.
const LLAMA_TAG = process.env.LLAMA_CPP_TAG ?? 'b11370';
const LLAMA_ASSET =
  process.env.LLAMA_CPP_ASSET ??
  ({
    darwin: { arm64: `llama-${LLAMA_TAG}-bin-macos-arm64.tar.gz`, x64: `llama-${LLAMA_TAG}-bin-macos-x64.tar.gz` },
    linux: { x64: `llama-${LLAMA_TAG}-bin-ubuntu-x64.tar.gz`, arm64: `llama-${LLAMA_TAG}-bin-ubuntu-arm64.tar.gz` },
    win32: { x64: `llama-${LLAMA_TAG}-bin-win-cpu-x64.zip`, arm64: `llama-${LLAMA_TAG}-bin-win-cpu-arm64.zip` },
  }[process.platform]?.[process.arch]);
if (!LLAMA_ASSET) {
  console.error(`[fetch-ai-assets] no llama.cpp build for ${process.platform}/${process.arch}`);
  process.exit(1);
}
const LLAMA_URL = `https://github.com/ggml-org/llama.cpp/releases/download/${LLAMA_TAG}/${LLAMA_ASSET}`;

// Qwen3-0.6B Instruct Q4_K_M (~450 MB) — Apache-2.0, runs on CPU.
const GGUF_URL =
  process.env.QWEN_GGUF_URL ??
  'https://huggingface.co/unsloth/Qwen3-0.6B-GGUF/resolve/main/Qwen3-0.6B-Q4_K_M.gguf';

async function download(url, dest) {
  if (existsSync(dest) && statSync(dest).size > 0) {
    console.log(`[fetch-ai-assets] already present: ${dest}`);
    return;
  }
  mkdirSync(dirname(dest), { recursive: true });
  console.log(`[fetch-ai-assets] downloading ${url}`);
  const res = await fetch(url, { redirect: 'follow' });
  if (!res.ok) throw new Error(`HTTP ${res.status} for ${url}`);
  const tmp = `${dest}.part`;
  const out = createWriteStream(tmp);
  const { pipeline } = await import('node:stream/promises');
  await pipeline(res.body, out);
  renameSync(tmp, dest);
  // Owners-writable, otherwise a later re-copy of a bundled resource
  // (Tauri build script preserves modes) truncates a read-only file and
  // fails with EACCES.
  chmodSync(dest, 0o644);
  console.log(`[fetch-ai-assets] saved ${dest} (${statSync(dest).size} bytes)`);
}

function extract(archive, into) {
  mkdirSync(into, { recursive: true });
  // Windows runners ship bsdtar (`tar`) which also reads .zip; macOS and
  // Linux use their system tar for the .tar.gz archives.
  const args = archive.endsWith('.zip') ? ['-xf', archive] : ['-xzf', archive];
  const r = spawnSync('tar', args, { cwd: into, stdio: 'inherit' });
  if (r.status !== 0) throw new Error(`tar failed for ${archive}`);
}

async function main() {
  // 1) llama-server binary + its runtime libs. The macOS build is
  //    dylib-based with an `@loader_path` rpath, so EVERYTHING must end up
  //    flat beside the binary — bundling the launcher alone fails at dyld.
  const llamaDir = join(outDir, 'llama');
  const exe = process.platform === 'win32' ? 'llama-server.exe' : 'llama-server';
  rmSync(llamaDir, { recursive: true, force: true });
  mkdirSync(llamaDir, { recursive: true });

  const archive = join(llamaDir, LLAMA_ASSET);
  await download(LLAMA_URL, archive);
  const stage = join(llamaDir, '.extract');
  extract(archive, stage);
  const found = findFile(stage, exe);
  if (!found) throw new Error(`llama-server not found inside ${LLAMA_ASSET}`);
  // Lift the binary's directory (archive root or a nested folder) into
  // llamaDir so dylibs, data files and LICENSE sit beside the server.
  for (const entry of readdirSync(dirname(found))) {
    renameSync(join(dirname(found), entry), join(llamaDir, entry));
  }
  rmSync(stage, { recursive: true, force: true });
  rmSync(archive, { force: true });
  chmodSync(join(llamaDir, exe), 0o755);

  // Smoke test: dyld/dll resolution is the whole point of the flattening.
  const probe = spawnSync(join(llamaDir, exe), ['--version'], { encoding: 'utf8' });
  if (probe.status !== 0) {
    throw new Error(`${exe} --version failed after extraction: ${probe.stderr || probe.error}`);
  }
  console.log(`[fetch-ai-assets] llama-server ready: ${probe.stdout.split('\n')[0]}`);

  // 2) Starter GGUF
  if (process.env.SKIP_GGUF !== '1') {
    const name = GGUF_URL.split('/').pop();
    await download(GGUF_URL, join(outDir, 'models', name));
  }
  console.log('[fetch-ai-assets] done');
}

function findFile(dir, name) {
  const stack = [dir];
  while (stack.length) {
    const d = stack.pop();
    for (const e of readdirSync(d, { withFileTypes: true })) {
      const p = join(d, e.name);
      if (e.isDirectory()) stack.push(p);
      else if (e.name === name) return p;
    }
  }
  return null;
}

main().catch((err) => {
  console.error(`[fetch-ai-assets] failed: ${err.message}`);
  process.exit(1);
});
