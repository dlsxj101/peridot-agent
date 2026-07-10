import type { CodeMapSummary } from '../src/types';
import { t } from './i18n';

export interface CodeMapPillView {
  label: string;
  tone: 'good' | 'warn' | 'mute';
  title: string;
}

export function codeMapContextPill(summary: CodeMapSummary | undefined): CodeMapPillView | undefined {
  // Only surface a pill when the workspace has no code map index at all. Stale
  // and fresh states are intentionally not rendered — they are noisy and the
  // code map is reachable from the overflow menu regardless.
  if (!summary || summary.indexExists !== false) return undefined;
  const label = t('Code map missing', '코드 맵 없음');
  const details = [
    summary.reason,
  ].filter((part): part is string => Boolean(part));
  return {
    label,
    tone: 'warn',
    title: details.length > 0 ? details.join('\n') : label,
  };
}
