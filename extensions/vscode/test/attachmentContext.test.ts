import test from 'node:test';
import assert from 'node:assert/strict';

import { attachmentPathsFromCommandResult } from '../src/attachmentContext';

test('attachmentPathsFromCommandResult adds attached file paths', () => {
  assert.deepEqual(
    attachmentPathsFromCommandResult(
      {
        kind: 'attach',
        attachment: { path: 'src/main.rs' },
      },
      ['docs/notes.md'],
    ),
    ['docs/notes.md', 'src/main.rs'],
  );
});

test('attachmentPathsFromCommandResult replaces inventory from attachments results', () => {
  assert.deepEqual(
    attachmentPathsFromCommandResult({
      kind: 'attachments',
      attachments: [{ path: 'src/lib.rs' }, { path: 'docs/notes.md' }],
    }),
    ['docs/notes.md', 'src/lib.rs'],
  );
});

test('attachmentPathsFromCommandResult uses remaining attachments after detach', () => {
  assert.deepEqual(
    attachmentPathsFromCommandResult(
      {
        kind: 'detach',
        removed: [{ path: 'src/main.rs' }],
        attachments: [{ path: 'src/lib.rs' }],
      },
      ['src/lib.rs', 'src/main.rs'],
    ),
    ['src/lib.rs'],
  );
});

test('attachmentPathsFromCommandResult removes detached paths without remaining payload', () => {
  assert.deepEqual(
    attachmentPathsFromCommandResult(
      {
        kind: 'detach',
        removed: [{ path: 'src/main.rs' }],
      },
      ['src/lib.rs', 'src/main.rs'],
    ),
    ['src/lib.rs'],
  );
});
