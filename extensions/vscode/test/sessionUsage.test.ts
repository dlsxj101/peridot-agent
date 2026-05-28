import test from 'node:test';
import assert from 'node:assert/strict';

import {
  activeSessionUsageDescription,
  sessionUsageDescription,
  sessionUsageParts,
} from '../src/sessionUsage';

test('sessionUsageDescription combines status and usage totals', () => {
  assert.equal(
    sessionUsageDescription({
      status: 'done',
      total_tokens: 12_400,
      total_cost_usd: 0.1234,
      turns_used: 3,
    }),
    'done · $0.123 · 12K tok · 3 turns',
  );
});

test('sessionUsageDescription dedupes running status', () => {
  assert.equal(sessionUsageDescription({ status: 'running', running: true }), 'running');
  assert.equal(sessionUsageDescription({ running: true }), 'running');
});

test('activeSessionUsageDescription keeps active label first', () => {
  assert.equal(
    activeSessionUsageDescription({
      total_tokens: 900,
      total_cost_usd: 0.0042,
      turns_used: 1,
    }),
    'active session · $0.0042 · 900 tok · 1 turn',
  );
});

test('sessionUsageParts omits empty totals', () => {
  assert.deepEqual(sessionUsageParts({ total_tokens: 0, total_cost_usd: 0, turns_used: 0 }), []);
});
