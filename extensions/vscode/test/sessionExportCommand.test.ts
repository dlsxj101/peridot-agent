import test from 'node:test';
import assert from 'node:assert/strict';

import {
  exportedArtifactsFromPayload,
  sessionExportChoices,
  sessionExportCommandResult,
  sessionExportDirectoryName,
} from '../src/sessionExportCommand';

test('sessionExportChoices puts active session first and dedupes persisted rows', () => {
  assert.deepEqual(
    sessionExportChoices(
      [
        { id: 's-1', title: 'First', status: 'done' },
        {
          id: 's-2',
          summary: 'Second summary',
          running: true,
          total_tokens: 2_400,
          total_cost_usd: 0.0456,
          turns_used: 4,
        },
      ],
      's-2',
    ),
    [
      {
        id: 's-2',
        label: 'Second summary',
        description: 'active session · $0.046 · 2.4K tok · 4 turns',
      },
      { id: 's-1', label: 'First', description: 'done' },
    ],
  );
});

test('sessionExportChoices keeps current session when it is not persisted yet', () => {
  assert.deepEqual(sessionExportChoices([], 'live-session'), [
    { id: 'live-session', label: 'live-session', description: 'active session' },
  ]);
});

test('sessionExportDirectoryName sanitizes target directory segment', () => {
  assert.equal(sessionExportDirectoryName('release prep'), 'peridot-session-release-prep');
  assert.equal(sessionExportDirectoryName(''), 'peridot-session-session');
});

test('exportedArtifactsFromPayload normalizes generated artifact rows', () => {
  assert.deepEqual(
    exportedArtifactsFromPayload({
      artifacts: [
        { class: 'attachments', path: 'attachments.json', count: 2 },
        { class: 'notes', path: 'notes.ndjson' },
        'ignored',
      ],
    }),
    [
      { class: 'attachments', path: 'attachments.json', count: 2 },
      { class: 'notes', path: 'notes.ndjson', count: 0 },
    ],
  );
});

test('sessionExportCommandResult preserves destination and artifacts for export cards', () => {
  const result = sessionExportCommandResult(
    {
      destination: '/tmp/peridot-session-s-1',
      files: ['tui_state.json'],
      artifacts: [
        { class: 'attachments', path: 'attachments.json', count: 1 },
        { class: 'timeline', path: 'timeline.json', count: 3 },
      ],
    },
    's-1',
    '/fallback',
  );

  assert.equal(result.kind, 'session_export');
  assert.equal(result.destination, '/tmp/peridot-session-s-1');
  assert.deepEqual(result.files, ['tui_state.json']);
  assert.deepEqual(result.artifacts, [
    { class: 'attachments', path: 'attachments.json', count: 1 },
    { class: 'timeline', path: 'timeline.json', count: 3 },
  ]);
  assert.deepEqual(
    result.items?.map((item) => [item.label, item.detail, item.source]),
    [
      ['Session', 's-1', 'session'],
      ['Destination', '/tmp/peridot-session-s-1', 'directory'],
      ['tui_state.json', 'full copy', 'full_copy'],
      ['attachments.json', 'attachments · 1 entries', 'artifact'],
      ['timeline.json', 'timeline · 3 entries', 'artifact'],
    ],
  );
});
