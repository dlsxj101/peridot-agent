import test from 'node:test';
import assert from 'node:assert/strict';

import { sessionSearchSlashCommand } from '../src/sessionSearchCommand';

test('sessionSearchSlashCommand preserves a multi-word query', () => {
  assert.equal(sessionSearchSlashCommand(' parser failure '), '/session search parser failure');
});

test('sessionSearchSlashCommand rejects empty queries', () => {
  assert.throws(() => sessionSearchSlashCommand('   '), /Search query/);
});
