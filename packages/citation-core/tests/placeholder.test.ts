import { test } from 'node:test';
import assert from 'node:assert/strict';
import { CITATION_CORE_PLACEHOLDER } from '../src/index.ts';

test('citation-core placeholder is importable', () => {
  assert.equal(CITATION_CORE_PLACEHOLDER, true);
});
