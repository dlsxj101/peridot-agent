import test from 'node:test';
import assert from 'node:assert/strict';

import {
  sessionCountSlashCommand,
  sessionLocateSlashCommand,
  sessionResumeSlashCommand,
  sessionShowSlashCommand,
  sessionTargetChoices,
} from '../src/sessionInspectCommand';

test('sessionCountSlashCommand builds the count command', () => {
  assert.equal(sessionCountSlashCommand(), '/session count');
});

test('sessionTargetChoices lists persisted sessions without duplicates', () => {
  assert.deepEqual(
    sessionTargetChoices([
      { id: 's-1', title: 'First', status: 'done' },
      { id: 's-2', last_task: 'Investigate bug', running: true },
      { id: 's-1', title: 'Duplicate' },
    ]),
    [
      { id: 's-1', label: 'First', description: 'done' },
      { id: 's-2', label: 'Investigate bug', description: 'running' },
    ],
  );
});

test('session show and locate commands quote targets', () => {
  assert.equal(sessionShowSlashCommand("release prep's run"), "/session show 'release prep'\\''s run'");
  assert.equal(sessionLocateSlashCommand(' release prep '), "/session locate 'release prep'");
  assert.equal(sessionResumeSlashCommand('release prep'), "/session resume 'release prep'");
});

test('session target commands reject empty targets', () => {
  assert.throws(() => sessionShowSlashCommand('   '), /Session id/);
  assert.throws(() => sessionLocateSlashCommand('   '), /Session id/);
  assert.throws(() => sessionResumeSlashCommand('   '), /Session id/);
});
