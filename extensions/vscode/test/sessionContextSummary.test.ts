import test from 'node:test';
import assert from 'node:assert/strict';

import {
  sessionContextSummary,
  sessionContextSummaryChips,
} from '../webview/sessionContextSummary';

test('sessionContextSummary normalizes note and attachment metadata', () => {
  const summary = sessionContextSummary({
    kind: 'session_import',
    notes_count: 2.8,
    last_note: ' latest checkpoint ',
    attachment_count: 3.2,
    attachment_paths: [' docs/a.md ', 'DOCS/a.md', 'src/main.rs'],
  });

  assert.deepEqual(summary, {
    noteCount: 2,
    latestNote: 'latest checkpoint',
    attachmentCount: 3,
    attachmentPaths: ['docs/a.md', 'src/main.rs'],
  });
});

test('sessionContextSummaryChips renders explicit empty snapshots', () => {
  assert.deepEqual(
    sessionContextSummaryChips(
      sessionContextSummary({
        kind: 'session_save',
        notes_count: 0,
        attachment_count: 0,
        attachment_paths: [],
      }),
    ),
    ['0 notes', '0 attachments'],
  );
});

test('sessionContextSummary falls back to path count when count is absent', () => {
  assert.deepEqual(
    sessionContextSummary({
      kind: 'session_import',
      attachment_paths: ['docs/a.md', 'src/b.md'],
    }),
    {
      attachmentCount: 2,
      attachmentPaths: ['docs/a.md', 'src/b.md'],
    },
  );
});
