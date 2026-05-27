import test from 'node:test';
import assert from 'node:assert/strict';

import { staleDaemonBackedSessionIds } from '../src/sessionReconcile';

test('staleDaemonBackedSessionIds prunes only missing daemon-backed sessions', () => {
  const staleIds = staleDaemonBackedSessionIds(
    [
      { id: 'local-draft' },
      { id: 'client-a', daemonSessionId: 'daemon-a' },
      { id: 'client-b', daemonSessionId: 'daemon-b' },
    ],
    [{ id: 'daemon-b' }],
  );

  assert.deepEqual(staleIds, ['client-a']);
});

test('staleDaemonBackedSessionIds treats an empty remote inventory as authoritative', () => {
  const staleIds = staleDaemonBackedSessionIds(
    [
      { id: 'local-draft' },
      { id: 'client-a', daemonSessionId: 'daemon-a' },
      { id: 'client-b', daemonSessionId: 'daemon-b' },
    ],
    [],
  );

  assert.deepEqual(staleIds, ['client-a', 'client-b']);
});

test('staleDaemonBackedSessionIds trims remote ids and ignores malformed rows', () => {
  const staleIds = staleDaemonBackedSessionIds(
    [
      { id: 'client-a', daemonSessionId: 'daemon-a' },
      { id: 'client-b', daemonSessionId: 'daemon-b' },
    ],
    [{ id: ' daemon-a ' }, { id: '' }, { id: null }],
  );

  assert.deepEqual(staleIds, ['client-b']);
});
