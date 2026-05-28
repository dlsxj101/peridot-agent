import test from 'node:test';
import assert from 'node:assert/strict';

import { commandResultSessionContextFields } from '../src/sessionCommandSummary';

test('commandResultSessionContextFields preserves session context metadata', () => {
  assert.deepEqual(
    commandResultSessionContextFields({
      kind: 'session_save',
      notes_count: 2,
      last_note: 'latest',
      attachment_count: 3,
      attachment_paths: ['docs/a.md', 42 as unknown as string, 'src/main.rs'],
    }),
    {
      notes_count: 2,
      last_note: 'latest',
      attachment_count: 3,
      attachment_paths: ['docs/a.md', 'src/main.rs'],
    },
  );
});

test('commandResultSessionContextFields keeps explicit empty context snapshots', () => {
  assert.deepEqual(
    commandResultSessionContextFields({
      kind: 'session_import',
      notes_count: 0,
      last_note: null,
      attachment_count: 0,
      attachment_paths: [],
    }),
    {
      notes_count: 0,
      last_note: null,
      attachment_count: 0,
      attachment_paths: [],
    },
  );
});
