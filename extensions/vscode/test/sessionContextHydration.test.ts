import test from 'node:test';
import assert from 'node:assert/strict';

import { commandResultCanHydrateSessionContext } from '../src/sessionContextHydration';

test('commandResultCanHydrateSessionContext allows non-session-show results', () => {
  assert.equal(
    commandResultCanHydrateSessionContext({ kind: 'attach', session_id: 'other' }, 'active'),
    true,
  );
});

test('commandResultCanHydrateSessionContext matches active client or daemon session', () => {
  assert.equal(
    commandResultCanHydrateSessionContext(
      { kind: 'session_show', session_id: 'client-1' },
      ' client-1 ',
      'daemon-1',
    ),
    true,
  );
  assert.equal(
    commandResultCanHydrateSessionContext(
      { kind: 'session_show', session_id: 'daemon-1' },
      'client-1',
      ' daemon-1 ',
    ),
    true,
  );
});

test('commandResultCanHydrateSessionContext rejects other session show results', () => {
  assert.equal(
    commandResultCanHydrateSessionContext(
      { kind: 'session_show', session_id: 'other-session' },
      'client-1',
      'daemon-1',
    ),
    false,
  );
  assert.equal(
    commandResultCanHydrateSessionContext({ kind: 'session_show' }, 'client-1', 'daemon-1'),
    false,
  );
});
