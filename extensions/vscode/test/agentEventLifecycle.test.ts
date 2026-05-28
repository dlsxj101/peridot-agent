import test from 'node:test';
import assert from 'node:assert/strict';

import {
  isAskUserWaitingEvent,
  isTerminalAgentEvent,
  terminalStatusForEvent,
} from '../src/agentEventLifecycle';

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

test('isAskUserWaitingEvent detects daemon ask-user prompts', () => {
  assert.equal(isAskUserWaitingEvent({ kind: 'ask_user_requested', request_id: 's:ask-user:1' }), true);
  assert.equal(isAskUserWaitingEvent({ kind: 'approval_waiting' }), false);
  assert.equal(isAskUserWaitingEvent(null), false);
});
