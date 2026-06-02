import test from 'node:test';
import assert from 'node:assert/strict';

import { countTranscriptMatches, transcriptItemMatchesQuery } from '../webview/transcriptSearch';
import type { TranscriptItem } from '../src/types';

function item(partial: Partial<TranscriptItem>): TranscriptItem {
  return { role: 'assistant', text: '', ...partial } as TranscriptItem;
}

test('empty query matches everything', () => {
  assert.equal(transcriptItemMatchesQuery(item({ text: 'anything' }), ''), true);
  assert.equal(transcriptItemMatchesQuery(item({ text: 'anything' }), '   '), true);
});

test('matches message text case-insensitively', () => {
  const it = item({ text: 'Refactor the Scanner module' });
  assert.equal(transcriptItemMatchesQuery(it, 'scanner'), true);
  assert.equal(transcriptItemMatchesQuery(it, 'SCANNER'), true);
  assert.equal(transcriptItemMatchesQuery(it, 'parser'), false);
});

test('matches tool name, path, and result summary fields', () => {
  assert.equal(
    transcriptItemMatchesQuery(item({ role: 'tool', toolName: 'file_read' }), 'file_read'),
    true,
  );
  assert.equal(
    transcriptItemMatchesQuery(item({ role: 'tool', path: 'src/lib.rs' }), 'lib.rs'),
    true,
  );
  assert.equal(
    transcriptItemMatchesQuery(item({ role: 'tool', toolResultSummary: 'wrote 3 files' }), 'wrote'),
    true,
  );
});

test('countTranscriptMatches counts matches; full length for empty query', () => {
  const items = [
    item({ text: 'alpha' }),
    item({ text: 'beta' }),
    item({ text: 'alpha again' }),
  ];
  assert.equal(countTranscriptMatches(items, 'alpha'), 2);
  assert.equal(countTranscriptMatches(items, ''), 3);
  assert.equal(countTranscriptMatches([], 'x'), 0);
});
