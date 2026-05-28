import test from 'node:test';
import assert from 'node:assert/strict';

import { committeeTranscriptItemForEvent } from '../src/agentEventTranscript';

test('committeeTranscriptItemForEvent renders planner plan text', () => {
  assert.deepEqual(
    committeeTranscriptItemForEvent('planner_plan_ready', {
      plan_text: '1. Inspect\n2. Patch',
    }),
    {
      role: 'status',
      text: 'committee planner ready:\n1. Inspect\n2. Patch',
    },
  );
});

test('committeeTranscriptItemForEvent renders nested reviewer request changes', () => {
  assert.deepEqual(
    committeeTranscriptItemForEvent('reviewer_verdict', {
      turn_index: 2,
      verdict: { kind: 'request_changes', comments: 'tighten the parser guard' },
    }),
    {
      role: 'status',
      text: 'committee reviewer (turn 2): request_changes - tighten the parser guard',
    },
  );
});

test('committeeTranscriptItemForEvent renders reviewer blocks as errors', () => {
  assert.deepEqual(
    committeeTranscriptItemForEvent('reviewer_verdict', {
      turn_index: 3,
      verdict: { kind: 'block', reason: 'same diff reached max review passes' },
    }),
    {
      role: 'error',
      text: 'committee reviewer (turn 3): block - same diff reached max review passes',
    },
  );
});

test('committeeTranscriptItemForEvent accepts legacy flat replay-shaped verdicts', () => {
  assert.deepEqual(
    committeeTranscriptItemForEvent('reviewer_verdict', {
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

test('committeeTranscriptItemForEvent ignores unrelated events', () => {
  assert.equal(committeeTranscriptItemForEvent('assistant_delta', { delta: 'hello' }), undefined);
});
