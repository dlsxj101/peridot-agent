import test from 'node:test';
import assert from 'node:assert/strict';

import { codeMapFromCommandResult, markCodeMapStale } from '../src/codeMapContext';
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

test('codeMapContextPill summarizes freshness and counts', () => {
  assert.deepEqual(
    codeMapContextPill({
      indexExists: true,
      stale: false,
      symbolCount: 5,
      todoCount: 2,
      walkedFiles: 9,
      generatedAtUnix: 201,
    }),
    {
      label: 'Code map fresh · 5 sym · 2 todos',
      tone: 'good',
      title: 'indexed at 201\n9 indexed file(s)',
    },
  );

  const stale = codeMapContextPill({ stale: true, todoCount: 2, reason: 'file changed' });
  assert.equal(stale?.label, 'Code map stale · 2 todos');
  assert.equal(stale?.tone, 'warn');
  assert.match(stale?.title ?? '', /file changed/);
});
