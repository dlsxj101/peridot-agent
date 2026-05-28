import test from 'node:test';
import assert from 'node:assert/strict';

import {
  agentTranscriptItemForEvent,
  shouldSuppressAgentEventFallback,
} from '../src/agentEventTranscript';

test('agentTranscriptItemForEvent renders planner plan text', () => {
  assert.deepEqual(
    agentTranscriptItemForEvent('planner_plan_ready', {
      plan_text: '1. Inspect\n2. Patch',
    }),
    {
      role: 'status',
      text: 'committee planner ready:\n1. Inspect\n2. Patch',
    },
  );
});

test('agentTranscriptItemForEvent renders nested reviewer request changes', () => {
  assert.deepEqual(
    agentTranscriptItemForEvent('reviewer_verdict', {
      turn_index: 2,
      verdict: { kind: 'request_changes', comments: 'tighten the parser guard' },
    }),
    {
      role: 'status',
      text: 'committee reviewer (turn 2): request_changes - tighten the parser guard',
    },
  );
});

test('agentTranscriptItemForEvent renders reviewer blocks as errors', () => {
  assert.deepEqual(
    agentTranscriptItemForEvent('reviewer_verdict', {
      turn_index: 3,
      verdict: { kind: 'block', reason: 'same diff reached max review passes' },
    }),
    {
      role: 'error',
      text: 'committee reviewer (turn 3): block - same diff reached max review passes',
    },
  );
});

test('agentTranscriptItemForEvent accepts legacy flat replay-shaped verdicts', () => {
  assert.deepEqual(
    agentTranscriptItemForEvent('reviewer_verdict', {
      turn_index: 1,
      verdict: 'approve',
      comments: '',
    }),
    {
      role: 'status',
      text: 'committee reviewer (turn 1): approve',
    },
  );
});

test('agentTranscriptItemForEvent renders autofix attempts', () => {
  assert.deepEqual(
    agentTranscriptItemForEvent('auto_fix_attempt', {
      attempt: 1,
      max: 3,
      tool_name: 'verify_test',
      passed: false,
    }),
    {
      role: 'status',
      text: 'autofix: verify_test FAILED (attempt 1/3)',
    },
  );
});

test('agentTranscriptItemForEvent renders successful autofix attempts', () => {
  assert.deepEqual(
    agentTranscriptItemForEvent('auto_fix_attempt', {
      attempt: 2,
      max: 3,
      tool_name: 'verify_test',
      passed: true,
    }),
    {
      role: 'status',
      text: 'autofix: verify_test passed (attempt 2/3)',
    },
  );
});

test('agentTranscriptItemForEvent renders session saved events', () => {
  assert.deepEqual(
    agentTranscriptItemForEvent('session_saved', {
      session_id: 'session-123',
    }),
    {
      role: 'status',
      text: 'session: saved session-123 - resume with: peridot session resume session-123',
    },
  );
});

test('agentTranscriptItemForEvent renders session save failures as errors', () => {
  assert.deepEqual(
    agentTranscriptItemForEvent('session_save_failed', {
      session_id: 'session-123',
      message: 'disk full',
    }),
    {
      role: 'error',
      text: 'session: failed to save session-123: disk full',
    },
  );
});

test('agentTranscriptItemForEvent renders hook fired events', () => {
  assert.deepEqual(
    agentTranscriptItemForEvent('hook_fired', {
      name: 'pre:file_patch',
      category: 'tool',
      outcome: 'allow',
    }),
    {
      role: 'status',
      text: 'hook:pre:file_patch - tool: allow',
    },
  );
});

test('agentTranscriptItemForEvent renders blocking hook outcomes as errors', () => {
  assert.deepEqual(
    agentTranscriptItemForEvent('hook_fired', {
      name: 'event:budget_warning',
      category: 'event',
      outcome: 'blocked',
    }),
    {
      role: 'error',
      text: 'hook:event:budget_warning - event: blocked',
    },
  );
});

test('agentTranscriptItemForEvent ignores unrelated events', () => {
  assert.equal(agentTranscriptItemForEvent('assistant_delta', { delta: 'hello' }), undefined);
});

test('shouldSuppressAgentEventFallback treats additive future events as no-op', () => {
  assert.equal(shouldSuppressAgentEventFallback('new_future_event'), true);
  assert.equal(shouldSuppressAgentEventFallback(''), false);
});
