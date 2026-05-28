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
      {
        id: 's-2',
        last_task: 'Investigate bug',
        running: true,
        total_tokens: 1_200,
        total_cost_usd: 0.0123,
        turns_used: 2,
      },
      { id: 's-1', title: 'Duplicate' },
    ]),
    [
      { id: 's-1', label: 'First', description: 'done' },
      {
        id: 's-2',
        label: 'Investigate bug',
        description: 'running · $0.012 · 1.2K tok · 2 turns',
      },
    ],
  );
});

test('sessionReplaySlashCommand builds parser-compatible target and last limit', () => {
  assert.equal(sessionReplaySlashCommand('s-1', 12), '/session replay s-1 --last 12');
  assert.throws(() => sessionReplaySlashCommand('bad id'), /whitespace/);
});

test('parseReplayLastInput accepts blank or positive integers only', () => {
  assert.equal(parseReplayLastInput(''), undefined);
  assert.equal(parseReplayLastInput(' 8 '), 8);
  assert.throws(() => parseReplayLastInput('0'), /positive whole number/);
  assert.throws(() => parseReplayLastInput('1.5'), /positive whole number/);
});
