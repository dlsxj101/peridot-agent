import test from 'node:test';
import assert from 'node:assert/strict';

import {
  ComposerHistory,
  canNavigateComposerHistory,
} from '../webview/composerHistory';

test('ComposerHistory keeps entries isolated per session', () => {
  const history = new ComposerHistory();
  history.record('session-a', 'first task');
  history.record('session-b', 'other task');
  history.record('session-a', 'second task');

  assert.equal(history.navigate('session-a', 'previous', ''), 'second task');
  assert.equal(history.navigate('session-a', 'previous', ''), 'first task');
  assert.equal(history.navigate('session-b', 'previous', ''), 'other task');
});

test('ComposerHistory restores the in-progress draft after walking forward', () => {
  const history = new ComposerHistory();
  history.record('session-a', 'first task');
  history.record('session-a', 'second task');

  assert.equal(history.navigate('session-a', 'previous', 'draft text'), 'second task');
  assert.equal(history.navigate('session-a', 'previous', 'second task'), 'first task');
  assert.equal(history.navigate('session-a', 'next', 'first task'), 'second task');
  assert.equal(history.navigate('session-a', 'next', 'second task'), 'draft text');
  assert.equal(history.navigate('session-a', 'next', 'draft text'), undefined);
});

test('ComposerHistory deduplicates repeated commands by moving them to the end', () => {
  const history = new ComposerHistory();
  history.record('session-a', 'first task');
  history.record('session-a', 'second task');
  history.record('session-a', 'first task');

  assert.deepEqual(history.entries('session-a'), ['second task', 'first task']);
});

test('canNavigateComposerHistory only captures arrows at textarea boundaries', () => {
  assert.equal(canNavigateComposerHistory('one line', 3, 3, 'previous'), true);
  assert.equal(canNavigateComposerHistory('one line', 3, 3, 'next'), true);
  assert.equal(canNavigateComposerHistory('first\nsecond', 3, 3, 'previous'), true);
  assert.equal(canNavigateComposerHistory('first\nsecond', 8, 8, 'previous'), false);
  assert.equal(canNavigateComposerHistory('first\nsecond', 3, 3, 'next'), false);
  assert.equal(canNavigateComposerHistory('first\nsecond', 8, 8, 'next'), true);
  assert.equal(canNavigateComposerHistory('one line', 1, 3, 'previous'), false);
});
