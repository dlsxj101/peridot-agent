import test from 'node:test';
import assert from 'node:assert/strict';

import {
  attachmentPathsFromCommandResult,
  attachmentPathsFromDaemonSession,
  normalizeAttachmentPaths,
} from '../src/attachmentContext';

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

test('normalizeAttachmentPaths sanitizes persisted session values', () => {
  assert.deepEqual(
    normalizeAttachmentPaths([' src/main.rs ', 'SRC/main.rs', '', 42, 'docs/notes.md']),
    ['docs/notes.md', 'src/main.rs'],
  );
  assert.deepEqual(normalizeAttachmentPaths(undefined), []);
});

test('attachmentPathsFromDaemonSession hydrates session list attachment values', () => {
  assert.deepEqual(
    attachmentPathsFromDaemonSession({
      id: 'session-1',
      attachment_paths: [' src/main.rs ', 'SRC/main.rs', 'docs/notes.md'],
    }),
    ['docs/notes.md', 'src/main.rs'],
  );
  assert.deepEqual(
    attachmentPathsFromDaemonSession({
      id: 'session-2',
      attachment_paths: [],
    }),
    [],
  );
  assert.equal(attachmentPathsFromDaemonSession({ id: 'legacy-session' }), undefined);
});
