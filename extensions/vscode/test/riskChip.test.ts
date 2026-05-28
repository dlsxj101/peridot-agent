import test from 'node:test';
import assert from 'node:assert/strict';

import { riskChipView } from '../webview/riskChip';

test('riskChipView renders known risk classes with stable tone and labels', () => {
  assert.deepEqual(riskChipView('destructive'), {
    className: 'risk-chip risk-chip-destructive',
    label: '!',
    title: 'Risk class: destructive',
  });
  assert.deepEqual(riskChipView('local_write'), {
    className: 'risk-chip risk-chip-local_write',
    label: 'W',
    title: 'Risk class: local write',
  });
});

test('riskChipView sanitizes unknown wire labels into an unknown css class', () => {
  assert.deepEqual(riskChipView('new_future_risk'), {
    className: 'risk-chip risk-chip-unknown',
    label: '?',
    title: 'Risk class: new future risk',
  });
});

test('riskChipView omits empty risk values', () => {
  assert.equal(riskChipView(undefined), undefined);
  assert.equal(riskChipView('   '), undefined);
});
