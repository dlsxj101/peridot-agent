import test from 'node:test';
import assert from 'node:assert/strict';

import {
  codeMapFromCommandResult,
  codeMapFromStatusResult,
  markCodeMapStale,
} from '../src/codeMapContext';
import { codeMapContextPill } from '../webview/codeMapContext';

test('codeMapFromCommandResult stores codemap status snapshots', () => {
  assert.deepEqual(
    codeMapFromCommandResult({
      kind: 'codemap_status',
      index_exists: true,
      stale: false,
      source_files: 12,
      walked_files: 10,
      symbol_count: 4,
      todo_count: 2,
      generated_at_unix: 100,
      newest_source_mtime_unix: 99,
    }),
    {
      indexExists: true,
      stale: false,
      sourceFiles: 12,
      walkedFiles: 10,
      symbolCount: 4,
      todoCount: 2,
      generatedAtUnix: 100,
      newestSourceMtimeUnix: 99,
      reason: undefined,
    },
  );
});

test('codeMapFromStatusResult stores daemon status snapshots', () => {
  assert.deepEqual(
    codeMapFromStatusResult({
      index_exists: false,
      stale: false,
      source_files: 0,
      walked_files: 0,
      symbol_count: 0,
      todo_count: 0,
      generated_at_unix: null,
    }),
    {
      indexExists: false,
      stale: false,
      sourceFiles: 0,
      walkedFiles: 0,
      symbolCount: 0,
      todoCount: 0,
      generatedAtUnix: undefined,
      newestSourceMtimeUnix: undefined,
      reason: undefined,
    },
  );
  assert.equal(codeMapFromStatusResult(undefined), undefined);
});

test('codeMapFromCommandResult updates from codemap and todos command results', () => {
  const current = codeMapFromCommandResult({
    kind: 'codemap',
    symbol_count: 5,
    todo_count: 3,
    walked_files: 8,
    generated_at_unix: 200,
    refreshed: true,
  });

  assert.deepEqual(current, {
    indexExists: true,
    stale: false,
    walkedFiles: 8,
    symbolCount: 5,
    todoCount: 3,
    generatedAtUnix: 200,
    refreshed: true,
    reason: undefined,
  });

  assert.deepEqual(
    codeMapFromCommandResult(
      {
        kind: 'todos',
        items: [{ label: 'TODO' }, { label: 'FIXME' }],
        walked_files: 9,
        generated_at_unix: 201,
        refreshed: false,
      },
      current,
    ),
    {
      indexExists: true,
      stale: false,
      walkedFiles: 9,
      symbolCount: 5,
      todoCount: 2,
      generatedAtUnix: 201,
      refreshed: false,
      reason: undefined,
    },
  );
});

test('markCodeMapStale preserves counts and records the reason', () => {
  assert.deepEqual(
    markCodeMapStale({ indexExists: true, stale: false, symbolCount: 5 }, 'file changed'),
    {
      indexExists: true,
      stale: true,
      symbolCount: 5,
      reason: 'file changed',
    },
  );
});

test('codeMapContextPill only warns when the index is missing', () => {
  // Fresh index: no pill.
  assert.equal(
    codeMapContextPill({
      indexExists: true,
      stale: false,
      symbolCount: 5,
      todoCount: 2,
      walkedFiles: 9,
      generatedAtUnix: 201,
    }),
    undefined,
  );

  // Stale index (still exists): no pill.
  assert.equal(
    codeMapContextPill({ stale: true, todoCount: 2, reason: 'file changed' }),
    undefined,
  );

  // Missing index: warn pill.
  const missing = codeMapContextPill({ indexExists: false, stale: false, reason: 'no index yet' });
  assert.equal(missing?.label, 'Code map missing');
  assert.equal(missing?.tone, 'warn');
  assert.match(missing?.title ?? '', /no index yet/);

  // Missing index without a reason falls back to the label.
  const missingNoReason = codeMapContextPill({ indexExists: false, stale: false });
  assert.equal(missingNoReason?.title, 'Code map missing');

  // No summary at all: no pill.
  assert.equal(codeMapContextPill(undefined), undefined);
});
