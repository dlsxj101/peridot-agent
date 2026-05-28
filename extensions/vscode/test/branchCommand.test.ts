import test from 'node:test';
import assert from 'node:assert/strict';

import {
  branchListSlashCommand,
  branchPickerSlashCommand,
  branchRestoreSlashCommand,
  branchSaveSlashCommand,
  branchSnapshotChoices,
  branchSwitchSlashCommand,
  branchTreeSlashCommand,
  branchTurnSlashCommand,
  parseBranchSwitchInput,
  parseBranchTurnInput,
} from '../src/branchCommand';

test('branch slash helpers build parser-compatible commands', () => {
  assert.equal(branchPickerSlashCommand(), '/branch');
  assert.equal(branchListSlashCommand(), '/branch list');
  assert.equal(branchTreeSlashCommand(), '/branch tree');
  assert.equal(branchSaveSlashCommand(' checkpoint_1 '), '/branch save checkpoint_1');
  assert.equal(branchRestoreSlashCommand('release-branch'), '/branch restore release-branch');
  assert.equal(branchTurnSlashCommand(12), '/branch turn 12');
  assert.equal(branchSwitchSlashCommand(3), '/branch switch 3');
});

test('branch slash helpers validate names and numeric arguments', () => {
  assert.throws(() => branchSaveSlashCommand('bad name'), /ASCII letters/);
  assert.throws(() => branchRestoreSlashCommand('../bad'), /ASCII letters/);
  assert.throws(() => branchTurnSlashCommand(0), /positive integer/);
  assert.throws(() => branchSwitchSlashCommand(1.5), /positive integer/);
});

test('branchSnapshotChoices dedupes and sorts saved branch names', () => {
  assert.deepEqual(
    branchSnapshotChoices([' release ', '', 'parser', 'release']),
    [
      { name: 'parser', label: 'parser' },
      { name: 'release', label: 'release' },
    ],
  );
});

test('branch numeric input parsers accept positive integers only', () => {
  assert.equal(parseBranchTurnInput('42'), 42);
  assert.equal(parseBranchSwitchInput('2'), 2);
  assert.throws(() => parseBranchTurnInput('0'), /positive turn id/);
  assert.throws(() => parseBranchSwitchInput('abc'), /positive branch limb index/);
});
