import test from 'node:test';
import assert from 'node:assert/strict';

import { localSlashAction } from '../src/localSlashAction';

test('localSlashAction keeps only editor-local sidepanel aliases', () => {
  assert.equal(localSlashAction('/sidepanel'), 'showInfo');
  assert.equal(localSlashAction('/status'), 'showInfo');
});

test('localSlashAction does not reparse daemon-backed commands', () => {
  assert.equal(localSlashAction('/info'), undefined);
  assert.equal(localSlashAction('/cost'), undefined);
  assert.equal(localSlashAction('/plan show'), undefined);
  assert.equal(localSlashAction('/session list'), undefined);
});

test('localSlashAction rejects arguments and non-slash input', () => {
  assert.equal(localSlashAction('/sidepanel extra'), undefined);
  assert.equal(localSlashAction('sidepanel'), undefined);
});
