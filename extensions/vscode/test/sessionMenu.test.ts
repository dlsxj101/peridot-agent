import test from 'node:test';
import assert from 'node:assert/strict';

import { sessionMenuSubtitle } from '../webview/sessionMenu';

test('sessionMenuSubtitle preserves status when usage is empty', () => {
  assert.equal(
    sessionMenuSubtitle({
      id: 's1',
      title: 'Draft',
      status: 'Idle',
      running: false,
      active: false,
    }),
    'Idle',
  );
});

test('sessionMenuSubtitle includes persisted usage totals', () => {
  assert.equal(
    sessionMenuSubtitle({
      id: 's2',
      title: 'Done',
      status: 'done',
      running: false,
      active: false,
      total_tokens: 12_400,
      total_cost_usd: 0.1234,
      turns_used: 3,
    }),
    'done · $0.123 · 12K tok · 3 turns',
  );
});

test('sessionMenuSubtitle keeps running sessions explicit', () => {
  assert.equal(
    sessionMenuSubtitle({
      id: 's3',
      title: 'Active',
      status: 'Running',
      running: true,
      active: true,
      total_tokens: 900,
      total_cost_usd: 0.0042,
      turns_used: 1,
    }),
    'In progress · $0.0042 · 900 tok · 1 turn',
  );
});
