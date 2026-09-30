import { test } from 'node:test';
import assert from 'node:assert/strict';
import { RETRIEVAL_CORE_PLACEHOLDER } from '../src/index.ts';

test('retrieval-core placeholder is importable', () => {
  assert.equal(RETRIEVAL_CORE_PLACEHOLDER, true);
});
