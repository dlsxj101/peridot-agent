import test from 'node:test';
import assert from 'node:assert/strict';

import {
  acceptFileMention,
  currentFileMentionToken,
  fileMentionContext,
  filterFileMentionPaths,
} from '../webview/fileMention';

const files = [
  'src/main.rs',
  'src/lib.rs',
  'tests/main.rs',
  'docs/notes.md',
  'crates/peridot-cli/src/main.rs',
];

test('currentFileMentionToken detects word-boundary @ tokens', () => {
  assert.deepEqual(currentFileMentionToken('@src', 4), { tokenStart: 0, query: 'src' });
  assert.deepEqual(currentFileMentionToken('read @main', 10), { tokenStart: 5, query: 'main' });
  assert.equal(currentFileMentionToken('email@test', 10), undefined);
  assert.equal(currentFileMentionToken('@foo bar', 8), undefined);
});

test('filterFileMentionPaths prioritizes basename matches', () => {
  assert.deepEqual(filterFileMentionPaths(files, 'main'), [
    'crates/peridot-cli/src/main.rs',
    'src/main.rs',
    'tests/main.rs',
  ]);
});

test('fileMentionContext returns capped options for the current cursor token', () => {
  const context = fileMentionContext('inspect @main please', 13, files);
  assert.equal(context?.query, 'main');
  assert.deepEqual(context?.options, [
    'crates/peridot-cli/src/main.rs',
    'src/main.rs',
    'tests/main.rs',
  ]);
});

test('acceptFileMention replaces only the active @ token', () => {
  const context = fileMentionContext('inspect @main please', 13, files);
  assert.ok(context);
  assert.equal(
    acceptFileMention('inspect @main please', context, 1),
    'inspect @src/main.rs  please',
  );
});
