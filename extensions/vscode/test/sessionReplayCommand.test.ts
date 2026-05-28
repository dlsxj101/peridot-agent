import test from 'node:test';
import assert from 'node:assert/strict';

import {
  parseReplayLastInput,
  sessionReplayChoices,
  sessionReplaySlashCommand,
} from '../src/sessionReplayCommand';

test('sessionReplayChoices lists persisted sessions without duplicates', () => {
  assert.deepEqual(
    sessionReplayChoices([
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

test('sessionReplaySlashCommand quotes target and appends last limit', () => {
  assert.equal(sessionReplaySlashCommand("release prep's run", 12), "/session replay 'release prep'\\''s run' --last 12");
});

test('parseReplayLastInput accepts blank or positive integers only', () => {
  assert.equal(parseReplayLastInput(''), undefined);
  assert.equal(parseReplayLastInput(' 8 '), 8);
  assert.throws(() => parseReplayLastInput('0'), /positive whole number/);
  assert.throws(() => parseReplayLastInput('1.5'), /positive whole number/);
});
