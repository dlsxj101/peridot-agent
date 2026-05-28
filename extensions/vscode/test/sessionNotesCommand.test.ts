import test from 'node:test';
import assert from 'node:assert/strict';
import {
  parseNotesLastInput,
  sessionNoteSlashCommand,
  sessionNotesClearSlashCommand,
  sessionNotesSlashCommand,
} from '../src/sessionNotesCommand';

test('sessionNoteSlashCommand builds note commands', () => {
  assert.equal(sessionNoteSlashCommand(' checkpoint verified '), '/note checkpoint verified');
  assert.throws(() => sessionNoteSlashCommand('  '), /Note text/);
});

test('sessionNotesSlashCommand builds list and last commands', () => {
  assert.equal(sessionNotesSlashCommand(), '/notes');
  assert.equal(sessionNotesSlashCommand(3), '/notes last 3');
  assert.throws(() => sessionNotesSlashCommand(0), /positive integer/);
});

test('sessionNotesClearSlashCommand builds clear command', () => {
  assert.equal(sessionNotesClearSlashCommand(), '/notes clear');
});

test('parseNotesLastInput accepts blank or positive integers only', () => {
  assert.equal(parseNotesLastInput(''), undefined);
  assert.equal(parseNotesLastInput(' 12 '), 12);
  assert.throws(() => parseNotesLastInput('0'), /positive integer/);
  assert.throws(() => parseNotesLastInput('1.5'), /positive integer/);
});
