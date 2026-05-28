import test from 'node:test';
import assert from 'node:assert/strict';

import { sessionContextDetail, sessionContextParts } from '../src/sessionContextDetail';

test('sessionContextDetail combines id with note and attachment context', () => {
  assert.equal(
    sessionContextDetail(
      {
        id: 's-1',
        notes_count: 2,
        last_note: ' latest checkpoint ',
        attachment_count: 3,
      },
      'fallback',
    ),
    'fallback · Notes 2: latest checkpoint · Attachments 3',
  );
});

test('sessionContextDetail falls back to session id and attachment path count', () => {
  assert.equal(
    sessionContextDetail({
      id: 's-2',
      attachment_paths: ['docs/a.md', 'src/main.rs'],
    }),
    's-2 · Attachments 2',
  );
});

test('sessionContextParts includes note text without a persisted count', () => {
  assert.deepEqual(sessionContextParts({ last_note: 'manual checkpoint' }), [
    'Note: manual checkpoint',
  ]);
});

test('sessionContextParts omits empty context and clips long notes', () => {
  assert.deepEqual(sessionContextParts({ notes_count: 0, attachment_count: 0 }), []);
  assert.deepEqual(sessionContextParts({ last_note: 'x'.repeat(90) }), [
    `Note: ${'x'.repeat(77)}...`,
  ]);
});
