#!/usr/bin/env node
/**
 * Builds a single self-contained HTML file (apps/desktop/preview.html) from
 * the production Vite bundle so the UI can be previewed without a dev
 * server. The browser-preview mock backend in client.ts powers it.
 *
 * Usage: node scripts/dev/build-preview.mjs
 * (run `pnpm --filter @researchai/desktop build` first)
 */
import { readFileSync, writeFileSync, readdirSync } from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../..');
const dist = path.join(root, 'apps/desktop/dist');
const outFile = path.join(root, 'apps/desktop/preview.html');

const indexHtml = readFileSync(path.join(dist, 'index.html'), 'utf8');
const assetsDir = path.join(dist, 'assets');

let html = indexHtml;

for (const file of readdirSync(assetsDir)) {
  const abs = path.join(assetsDir, file);
  const content = readFileSync(abs);
  const isJs = file.endsWith('.js');
  const mime = isJs ? 'text/javascript' : 'text/css';
  // Escape closing script tags inside the payload so they cannot terminate
  // the inline <script> element early (standard `\/` escape is JS-safe).
  const js = isJs
    ? content.toString('utf8').replace(/<\/script/gi, '<\\/script')
    : content.toString('utf8');
  const tag = isJs
    ? `<script type="module">${js}</script>`
    : `<style>${js}</style>`;

  // Replace the asset reference (src or href) with the inlined payload.
  const refPatterns = [
    new RegExp(`<script[^>]*src="/assets/${file}"[^>]*></script>`),
    new RegExp(`<link[^>]*href="/assets/${file}"[^>]*>`),
  ];
  const matched = refPatterns.some((re) => re.test(html));
  if (matched) {
    const re = refPatterns.find((r) => r.test(html));
    // Function replacer is essential: a string replacement would interpret
    // `$&`/`` $` `` sequences inside the bundle as substitution patterns.
    html = html.replace(re, () => tag);
  } else {
    console.warn(`warn: no reference found for ${file}`);
  }
}

// Inline the favicon reference so no external requests remain.
html = html.replace(
  /<link[^>]*rel="icon"[^>]*>/,
  '<link rel="icon" href="data:image/svg+xml,%3Csvg xmlns=%22http://www.w3.org/2000/svg%22 viewBox=%220 0 64 64%22%3E%3Crect x=%222%22 y=%222%22 width=%2260%22 height=%2260%22 rx=%2214%22 fill=%22%231d4ed8%22/%3E%3C/svg%3E">',
);

// Relax CSP for this dev-only preview artefact: the shipped app keeps the
// strict policy from index.html, but inline payloads here need 'unsafe-inline'.
html = html.replace(
  /<meta[^>]*http-equiv="Content-Security-Policy"[^>]*>/,
  '<meta http-equiv="Content-Security-Policy" content="default-src \'self\'; script-src \'self\' \'unsafe-inline\'; style-src \'self\' \'unsafe-inline\'; img-src \'self\' data:">'
);

writeFileSync(outFile, html);
console.log(`wrote ${path.relative(root, outFile)} (${Math.round(html.length / 1024)} KiB)`);
