import { test } from 'node:test';
import assert from 'node:assert/strict';
import { UI_PLACEHOLDER } from '../src/index.ts';

test('ui placeholder is importable', () => {
  assert.equal(UI_PLACEHOLDER, true);
});
