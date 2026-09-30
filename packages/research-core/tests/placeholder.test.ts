import { test } from 'node:test';
import assert from 'node:assert/strict';
import { RESEARCH_CORE_PLACEHOLDER } from '../src/index.ts';

test('research-core placeholder is importable', () => {
  assert.equal(RESEARCH_CORE_PLACEHOLDER, true);
});
