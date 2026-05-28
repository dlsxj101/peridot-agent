import test from 'node:test';
import assert from 'node:assert/strict';

import { sessionImportSlashCommand } from '../src/sessionImportCommand';

test('sessionImportSlashCommand builds bare import command', () => {
  assert.equal(
    sessionImportSlashCommand({ source: '/tmp/peridot session' }),
    '/session import /tmp/peridot session',
  );
});

test('sessionImportSlashCommand includes id and force flags', () => {
  assert.equal(
    sessionImportSlashCommand({
      source: '/tmp/exported-session',
      id: 'restored',
      force: true,
    }),
    '/session import /tmp/exported-session --id restored --force',
  );
});

test('sessionImportSlashCommand validates required source and simple id', () => {
  assert.throws(() => sessionImportSlashCommand({ source: ' ' }), /source is required/);
  assert.throws(
    () => sessionImportSlashCommand({ source: '/tmp/session', id: 'bad id' }),
    /cannot contain whitespace/,
  );
});
