#!/usr/bin/env node
/**
 * Runs a command (default: `pnpm desktop:dev`) with the document engine
 * sidecar started alongside. The engine keeps running until the wrapped
 * command exits; the engine itself is killed afterwards.
 *
 * Usage:
 *   node scripts/dev/with-engine.mjs            # engine + desktop:dev
 *   node scripts/dev/with-engine.mjs -- <cmd>   # engine + <cmd>
 */
import { spawn } from 'node:child_process';
import http from 'node:http';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../..');
const ENGINE_URL = 'http://127.0.0.1:8737/health';

function waitForEngine(timeoutMs = 20000, intervalMs = 300) {
  const start = Date.now();
  return new Promise((resolve, reject) => {
    const attempt = () => {
      const req = http.get(ENGINE_URL, (res) => {
        res.resume();
        if (res.statusCode === 200) resolve();
        else retry();
      });
      req.on('error', retry);
    };
    const retry = () => {
      if (Date.now() - start > timeoutMs) reject(new Error('engine did not become healthy'));
      else setTimeout(attempt, intervalMs);
    };
    attempt();
  });
}

const engine = spawn(
  'uv',
  ['run', 'uvicorn', 'researchai_document_engine.app:app', '--host', '127.0.0.1', '--port', '8737'],
  { cwd: path.join(root, 'services/document-engine'), stdio: 'inherit' },
);

const argv = process.argv.slice(2);
const cmd = argv.includes('--')
  ? argv.slice(argv.indexOf('--') + 1).join(' ')
  : 'pnpm desktop:dev';

waitForEngine()
  .then(() => {
    console.log('[with-engine] document engine healthy on 127.0.0.1:8737');
    const child = spawn(cmd, { cwd: root, stdio: 'inherit', shell: true });
    child.on('exit', (code) => {
      engine.kill('SIGTERM');
      process.exit(code ?? 0);
    });
  })
  .catch((err) => {
    console.error('[with-engine]', err.message);
    engine.kill('SIGTERM');
    process.exit(1);
  });

process.on('SIGINT', () => {
  engine.kill('SIGTERM');
  process.exit(0);
});
