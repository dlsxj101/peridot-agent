import test from 'node:test';
import assert from 'node:assert/strict';

import {
  normalizeNoteSummary,
  noteSummaryFromCommandResult,
  noteSummaryFromDaemonSession,
} from '../src/noteContext';

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

test('noteSummaryFromDaemonSession hydrates session list note snapshots', () => {
  assert.deepEqual(
    noteSummaryFromDaemonSession({
      id: 'session-1',
      notes_count: 2,
      last_note: 'latest checkpoint',
    }),
    { count: 2, latest: 'latest checkpoint' },
  );
  assert.equal(noteSummaryFromDaemonSession({ id: 'legacy-session' }), undefined);
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
