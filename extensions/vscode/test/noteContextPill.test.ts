import test from 'node:test';
import assert from 'node:assert/strict';

import { noteContextPill } from '../webview/noteContext';

test('noteContextPill summarizes session notes', () => {
  assert.deepEqual(noteContextPill({ count: 2, latest: 'checkpoint verified' }), {
    label: 'Notes 2',
    tone: 'mute',
    title: 'latest: checkpoint verified',
  });
});

test('noteContextPill omits empty note summaries', () => {
  assert.equal(noteContextPill({ count: 0 }), undefined);
  assert.equal(noteContextPill(undefined), undefined);
});
