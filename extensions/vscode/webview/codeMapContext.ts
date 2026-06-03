import type { CodeMapSummary } from '../src/types';
import { t, tf } from './i18n';

export interface CodeMapPillView {
  label: string;
  tone: 'good' | 'warn' | 'mute';
  title: string;
}

export function codeMapContextPill(summary: CodeMapSummary | undefined): CodeMapPillView | undefined {
  if (!summary) return undefined;
  const state = summary.stale
    ? t('stale', '오래됨')
    : summary.indexExists === false
      ? t('missing', '없음')
      : t('fresh', '최신');
  const counts = [
    typeof summary.symbolCount === 'number' ? tf('{count} sym', '심볼 {count}', { count: summary.symbolCount }) : undefined,
    typeof summary.todoCount === 'number' ? tf('{count} todos', 'TODO {count}', { count: summary.todoCount }) : undefined,
  ].filter((part): part is string => Boolean(part));
  const label = counts.length > 0
    ? tf('Code map {state} · {counts}', '코드 맵 {state} · {counts}', { state, counts: counts.join(' · ') })
    : tf('Code map {state}', '코드 맵 {state}', { state });
  const details = [
    typeof summary.generatedAtUnix === 'number' ? tf('indexed at {at}', '{at}에 인덱싱됨', { at: summary.generatedAtUnix }) : undefined,
    typeof summary.newestSourceMtimeUnix === 'number'
      ? tf('newest source {at}', '최신 소스 {at}', { at: summary.newestSourceMtimeUnix })
      : undefined,
    typeof summary.walkedFiles === 'number' ? tf('{count} indexed file(s)', '인덱싱된 파일 {count}개', { count: summary.walkedFiles }) : undefined,
    typeof summary.sourceFiles === 'number' ? tf('{count} source file(s)', '소스 파일 {count}개', { count: summary.sourceFiles }) : undefined,
    summary.refreshed === true ? t('refreshed by last command', '마지막 명령으로 새로고침됨') : undefined,
    summary.reason,
  ].filter((part): part is string => Boolean(part));
  return {
    label,
    tone: summary.stale || summary.indexExists === false ? 'warn' : 'good',
    title: details.length > 0 ? details.join('\n') : label,
  };
}
