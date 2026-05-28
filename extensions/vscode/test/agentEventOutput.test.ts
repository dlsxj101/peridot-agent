import test from 'node:test';
import assert from 'node:assert/strict';

import { formatAgentEventForOutput } from '../src/agentEventOutput';

test('formatAgentEventForOutput renders recovery message for Output channel', () => {
  assert.equal(
    formatAgentEventForOutput('session-1', {
      kind: 'recovery',
      message: 'Recovery directive: try a different read-only command',
    }),
    '[session-1] recovery: Recovery directive: try a different read-only command',
  );
});

test('formatAgentEventForOutput keeps unknown events debuggable', () => {
  assert.equal(
    formatAgentEventForOutput('session-2', {
      kind: 'future_event',
      value: 42,
    }),
    '[session-2] future_event: {"kind":"future_event","value":42}',
  );
});
