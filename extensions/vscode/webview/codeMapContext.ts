import type { CodeMapSummary } from '../src/types';

export interface CodeMapPillView {
  label: string;
  tone: 'good' | 'warn' | 'mute';
  title: string;
}

export function codeMapContextPill(summary: CodeMapSummary | undefined): CodeMapPillView | undefined {
  if (!summary) return undefined;
  const state = summary.stale ? 'stale' : summary.indexExists === false ? 'missing' : 'fresh';
  const counts = [
    typeof summary.symbolCount === 'number' ? `${summary.symbolCount} sym` : undefined,
    typeof summary.todoCount === 'number' ? `${summary.todoCount} todos` : undefined,
  ].filter((part): part is string => Boolean(part));
  const label = counts.length > 0 ? `Code map ${state} · ${counts.join(' · ')}` : `Code map ${state}`;
  const details = [
    typeof summary.generatedAtUnix === 'number' ? `indexed at ${summary.generatedAtUnix}` : undefined,
    typeof summary.newestSourceMtimeUnix === 'number'
      ? `newest source ${summary.newestSourceMtimeUnix}`
      : undefined,
    typeof summary.walkedFiles === 'number' ? `${summary.walkedFiles} indexed file(s)` : undefined,
    typeof summary.sourceFiles === 'number' ? `${summary.sourceFiles} source file(s)` : undefined,
    summary.refreshed === true ? 'refreshed by last command' : undefined,
    summary.reason,
  ].filter((part): part is string => Boolean(part));
  return {
    label,
    tone: summary.stale || summary.indexExists === false ? 'warn' : 'good',
    title: details.length > 0 ? details.join('\n') : label,
  };
}
