import test from 'node:test';
import assert from 'node:assert/strict';

import { isTerminalAgentEvent, terminalStatusForEvent } from '../src/agentEventLifecycle';

test('isTerminalAgentEvent includes interrupted daemon events', () => {
  assert.equal(isTerminalAgentEvent({ kind: 'interrupted', stage: 'tool_call' }), true);
});

test('terminalStatusForEvent maps interrupted separately from failed', () => {
  assert.equal(terminalStatusForEvent({ kind: 'interrupted', stage: 'model_call' }), 'Interrupted');
  assert.equal(terminalStatusForEvent({ kind: 'error', message: 'boom' }), 'Failed');
  assert.equal(terminalStatusForEvent({ kind: 'approval_denied' }), 'Failed');
  assert.equal(terminalStatusForEvent({ kind: 'finished' }), 'Finished');
});

test('isTerminalAgentEvent ignores non-terminal daemon events', () => {
  assert.equal(isTerminalAgentEvent({ kind: 'run_started', task: 'fix tests' }), false);
  assert.equal(isTerminalAgentEvent({ kind: 'assistant_delta', delta: 'hello' }), false);
  assert.equal(isTerminalAgentEvent(null), false);
});
