import test from 'node:test';
import assert from 'node:assert/strict';

import { normalizeNoteSummary, noteSummaryFromCommandResult } from '../src/noteContext';

test('noteSummaryFromCommandResult increments note commands and captures latest text', () => {
  assert.deepEqual(
    noteSummaryFromCommandResult(
      {
        kind: 'note',
        note: { text: 'second checkpoint' },
      },
      { count: 1, latest: 'first checkpoint' },
    ),
    { count: 2, latest: 'second checkpoint' },
  );
});

test('noteSummaryFromCommandResult uses notes total and latest listed note', () => {
  assert.deepEqual(
    noteSummaryFromCommandResult({
      kind: 'notes',
      total: 3,
      notes: [{ text: 'first' }, { text: 'latest' }],
    }),
    { count: 3, latest: 'latest' },
  );
});

test('noteSummaryFromCommandResult clears notes state', () => {
  assert.deepEqual(
    noteSummaryFromCommandResult({ kind: 'notes_clear' }, { count: 3, latest: 'old' }),
    { count: 0 },
  );
});

test('normalizeNoteSummary sanitizes persisted note context', () => {
  assert.deepEqual(normalizeNoteSummary({ count: 2.8, latest: '  useful note  ' }), {
    count: 2,
    latest: 'useful note',
  });
  assert.deepEqual(normalizeNoteSummary({ count: -1, latest: '' }), { count: 0 });
  assert.equal(normalizeNoteSummary({ latest: 'missing count' }), undefined);
  assert.equal(normalizeNoteSummary(undefined), undefined);
});
