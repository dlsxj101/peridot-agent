import test from 'node:test';
import assert from 'node:assert/strict';

import { sessionExportChoices, sessionExportDirectoryName } from '../src/sessionExportCommand';

test('sessionExportChoices puts active session first and dedupes persisted rows', () => {
  assert.deepEqual(
    sessionExportChoices(
      [
        { id: 's-1', title: 'First', status: 'done' },
        { id: 's-2', summary: 'Second summary', running: true },
      ],
      's-2',
    ),
    [
      { id: 's-2', label: 'Second summary', description: 'active session' },
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
