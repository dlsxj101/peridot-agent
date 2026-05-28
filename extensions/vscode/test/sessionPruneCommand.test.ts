import test from 'node:test';
import assert from 'node:assert/strict';

import {
  parsePruneOlderThanDaysInput,
  sessionPruneSlashCommand,
  sessionPruneStatusChoices,
} from '../src/sessionPruneCommand';

test('sessionPruneSlashCommand builds dry-run and destructive commands', () => {
  assert.equal(sessionPruneSlashCommand({ dryRun: true }), '/session prune --dry-run');
  assert.equal(
    sessionPruneSlashCommand({ status: 'DONE', olderThanDays: 14 }),
    '/session prune --status done --older-than-days 14',
  );
});

test('sessionPruneSlashCommand validates status and age filters', () => {
  assert.throws(() => sessionPruneSlashCommand({ status: 'closed' }), /--status/);
  assert.throws(() => sessionPruneSlashCommand({ olderThanDays: 0 }), /--older-than-days/);
});

test('parsePruneOlderThanDaysInput accepts blank or positive integers only', () => {
  assert.equal(parsePruneOlderThanDaysInput(''), undefined);
  assert.equal(parsePruneOlderThanDaysInput(' 30 '), 30);
  assert.throws(() => parsePruneOlderThanDaysInput('-1'), /positive whole number/);
  assert.throws(() => parsePruneOlderThanDaysInput('2.5'), /positive whole number/);
});

test('sessionPruneStatusChoices starts with all statuses', () => {
  const choices = sessionPruneStatusChoices();
  assert.equal(choices[0].label, 'All statuses');
  assert.equal(choices[0].status, undefined);
  assert.ok(choices.some((choice) => choice.status === 'suspended'));
});
