import test from 'node:test';
import assert from 'node:assert/strict';

import {
  sessionCountSlashCommand,
  sessionCloseSlashCommand,
  sessionDeleteSlashCommand,
  sessionLocateSlashCommand,
  sessionNewSlashCommand,
  sessionRenameSlashCommand,
  sessionResumeSlashCommand,
  sessionShowSlashCommand,
  sessionSwitchSlashCommand,
  sessionTargetChoices,
} from '../src/sessionInspectCommand';

test('sessionCountSlashCommand builds the count command', () => {
  assert.equal(sessionCountSlashCommand(), '/session count');
});

test('sessionNewSlashCommand builds optional task commands', () => {
  assert.equal(sessionNewSlashCommand(), '/session new');
  assert.equal(sessionNewSlashCommand(' fix parser tests '), '/session new fix parser tests');
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

test('session target commands build parser-compatible id commands', () => {
  assert.equal(sessionShowSlashCommand(' s-1 '), '/session show s-1');
  assert.equal(sessionSwitchSlashCommand('s-1'), '/session switch s-1');
  assert.equal(sessionCloseSlashCommand('s-1'), '/session close s-1');
  assert.equal(sessionLocateSlashCommand('s-1'), '/session locate s-1');
  assert.equal(sessionResumeSlashCommand('s-1'), '/session resume s-1');
  assert.equal(sessionDeleteSlashCommand('s-1'), '/session delete s-1');
  assert.equal(sessionRenameSlashCommand('s-1', ' release   prep '), '/session rename s-1 release prep');
});

test('session target commands reject empty targets', () => {
  assert.throws(() => sessionShowSlashCommand('   '), /Session id/);
  assert.throws(() => sessionSwitchSlashCommand('   '), /Session id/);
  assert.throws(() => sessionCloseSlashCommand('   '), /Session id/);
  assert.throws(() => sessionLocateSlashCommand('   '), /Session id/);
  assert.throws(() => sessionResumeSlashCommand('   '), /Session id/);
  assert.throws(() => sessionDeleteSlashCommand('   '), /Session id/);
  assert.throws(() => sessionRenameSlashCommand('s-1', '   '), /Session title/);
  assert.throws(() => sessionShowSlashCommand('bad id'), /whitespace/);
});
