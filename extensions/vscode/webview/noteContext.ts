import type { NoteSummary } from '../src/types';

export interface NotePillView {
  label: string;
  tone: 'good' | 'warn' | 'mute';
  title: string;
}

export function noteContextPill(summary: NoteSummary | undefined): NotePillView | undefined {
  if (!summary || summary.count <= 0) return undefined;
  const latest = summary.latest?.trim();
  const label = `Notes ${summary.count}`;
  return {
    label,
    tone: 'mute',
    title: latest ? `latest: ${latest}` : label,
  };
}
