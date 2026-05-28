import test from 'node:test';
import assert from 'node:assert/strict';

import { normalizeNumberDraft, type NumberSettingValue } from '../webview/settingsModel';

test('normalizeNumberDraft leaves empty number drafts out of the save payload', () => {
  const current: NumberSettingValue = {
    kind: 'U32',
    data: { value: 12, min: 1, max: 100, step: 1 },
  };

  assert.deepEqual(normalizeNumberDraft(current, ''), {
    displayValue: '12',
    reason: 'empty',
  });
});

test('normalizeNumberDraft clamps out-of-range values', () => {
  const current: NumberSettingValue = {
    kind: 'F64',
    data: { value: 2.5, min: 0, max: 5, step: 0.25 },
  };

  const result = normalizeNumberDraft(current, '9.5');

  assert.deepEqual(result, {
    value: { kind: 'F64', data: { value: 5, min: 0, max: 5, step: 0.25 } },
    displayValue: '5',
    reason: 'clamped',
  });
});

test('normalizeNumberDraft normalizes integer setting values before save', () => {
  const current: NumberSettingValue = {
    kind: 'Usize',
    data: { value: 8, min: 1, max: 50, step: 1 },
  };

  const result = normalizeNumberDraft(current, '12.9');

  assert.deepEqual(result, {
    value: { kind: 'Usize', data: { value: 12, min: 1, max: 50, step: 1 } },
    displayValue: '12',
    reason: 'normalized',
  });
});
