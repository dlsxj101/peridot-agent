import test from 'node:test';
import assert from 'node:assert/strict';

import { sessionListSlashCommand, sessionListStatusChoices } from '../src/sessionListCommand';

test('sessionListSlashCommand builds all-session and filtered commands', () => {
  assert.equal(sessionListSlashCommand(), '/session list');
  assert.equal(sessionListSlashCommand('DONE'), '/session list --status done');
});

test('sessionListSlashCommand validates status filters', () => {
  assert.throws(() => sessionListSlashCommand('closed'), /--status/);
});

test('sessionListStatusChoices starts with all sessions', () => {
  const choices = sessionListStatusChoices();
  assert.equal(choices[0].label, 'All sessions');
  assert.equal(choices[0].status, undefined);
  assert.ok(choices.some((choice) => choice.status === 'failed'));
});
