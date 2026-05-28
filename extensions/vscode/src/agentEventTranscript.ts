import type { TranscriptItem, TranscriptRole } from './types';

export function committeeTranscriptItemForEvent(
  kind: string,
  event: Record<string, unknown>,
): TranscriptItem | undefined {
  switch (kind) {
    case 'planner_plan_ready': {
      const planText = textField(event, 'plan_text')?.trim() ?? '';
      return {
        role: 'status',
        text: planText.length > 0 ? `committee planner ready:\n${planText}` : 'committee planner ready',
      };
    }
    case 'reviewer_verdict': {
      const turnIndex = numberField(event, 'turn_index');
      const verdict = reviewerVerdictSummary(event.verdict, event);
      const turnLabel = typeof turnIndex === 'number' ? ` (turn ${turnIndex})` : '';
      const base = `committee reviewer${turnLabel}: ${verdict.kind}`;
      return {
        role: verdict.role,
        text: verdict.detail.length > 0 ? `${base} - ${verdict.detail}` : base,
      };
    }
    default:
      return undefined;
  }
}

function reviewerVerdictSummary(
  value: unknown,
  event: Record<string, unknown>,
): { kind: string; detail: string; role: TranscriptRole } {
  if (isRecord(value)) {
    const kind = textField(value, 'kind') ?? 'unknown';
    if (kind === 'request_changes') {
      return {
        kind,
        detail: textField(value, 'comments')?.trim() ?? '',
        role: 'status',
      };
    }
    if (kind === 'block') {
      return {
        kind,
        detail: (textField(value, 'reason') ?? textField(value, 'comments') ?? '').trim(),
        role: 'error',
      };
    }
    return { kind, detail: '', role: 'status' };
  }

  const kind = typeof value === 'string' && value.trim().length > 0 ? value.trim() : 'unknown';
  const detail = (textField(event, 'comments') ?? textField(event, 'reason') ?? '').trim();
  return { kind, detail, role: kind === 'block' ? 'error' : 'status' };
}

function textField(record: Record<string, unknown>, key: string): string | undefined {
  const value = record[key];
  return typeof value === 'string' ? value : undefined;
}

function numberField(record: Record<string, unknown>, key: string): number | undefined {
  const value = record[key];
  return typeof value === 'number' && Number.isFinite(value) ? value : undefined;
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null;
}
