export type SettingValue =
  | { kind: 'Bool'; data: boolean }
  | { kind: 'Choice'; data: { options: string[]; selected: number } }
  | { kind: 'U32'; data: { value: number; min: number; max: number; step: number } }
  | { kind: 'F64'; data: { value: number; min: number; max: number; step: number } }
  | { kind: 'Usize'; data: { value: number; min: number; max: number; step: number } };

export type NumberSettingValue = Extract<
  SettingValue,
  { kind: 'U32' | 'F64' | 'Usize' }
>;

export interface NumberDraftResult {
  value?: NumberSettingValue;
  displayValue?: string;
  reason?: 'empty' | 'invalid' | 'clamped' | 'normalized';
}

export function normalizeNumberDraft(
  current: NumberSettingValue,
  rawInput: string,
): NumberDraftResult {
  const trimmed = rawInput.trim();
  if (trimmed.length === 0) {
    return { displayValue: String(current.data.value), reason: 'empty' };
  }

  const parsed = Number(trimmed);
  if (!Number.isFinite(parsed)) {
    return { displayValue: String(current.data.value), reason: 'invalid' };
  }

  const numeric = current.kind === 'F64' ? parsed : Math.trunc(parsed);
  const clamped = Math.min(current.data.max, Math.max(current.data.min, numeric));
  const next = withNumberValue(current, clamped);
  const reason =
    clamped !== numeric ? 'clamped' : numeric !== parsed ? 'normalized' : undefined;
  return {
    value: next,
    displayValue: String(clamped),
    reason,
  };
}

function withNumberValue(current: NumberSettingValue, value: number): NumberSettingValue {
  if (current.kind === 'U32') {
    return { kind: 'U32', data: { ...current.data, value } };
  }
  if (current.kind === 'Usize') {
    return { kind: 'Usize', data: { ...current.data, value } };
  }
  return { kind: 'F64', data: { ...current.data, value } };
}
