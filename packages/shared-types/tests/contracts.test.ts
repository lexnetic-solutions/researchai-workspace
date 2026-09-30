import { test } from 'node:test';
import assert from 'node:assert/strict';
import type { DiagnosticsReport, Err, Ok, Project } from '../src/index.ts';

test('Result envelope narrows correctly', () => {
  const ok: Ok<number> = { ok: true, data: 42 };
  const err: Err = { ok: false, error: 'boom' };

  assert.equal(ok.ok, true);
  if (ok.ok) assert.equal(ok.data, 42);
  assert.equal(err.ok, false);
});

test('Project shape matches DB contract', () => {
  const project: Project = {
    id: 'u-1',
    name: 'Thesis',
    description: null,
    createdAt: '2026-09-29T00:00:00Z',
    updatedAt: '2026-09-29T00:00:00Z',
  };
  assert.equal(project.description, null);
});

test('DiagnosticsReport carries checks list', () => {
  const report: DiagnosticsReport = {
    system: {
      osName: 'macOS',
      osVersion: '26',
      arch: 'arm64',
      cpu: { name: 'Apple M-series', cores: 8 },
      totalMemoryMb: 16384,
      availableMemoryMb: 8192,
      profile: 'standard',
    },
    checks: [
      { id: 'data-dir', label: 'Data directory', status: 'pass', detail: 'ok' },
    ],
    dataDirectory: '/tmp/ResearchAIData',
    databaseOk: true,
    documentEngineOk: false,
    generatedAt: '2026-09-29T00:00:00Z',
  };
  assert.equal(report.checks.length, 1);
});
