import type { ChatSessionSummary } from '../src/types';

export function sessionMenuSubtitle(session: ChatSessionSummary): string {
  const parts = [session.running ? 'In progress' : session.status];
  const usage = sessionUsageParts(session);
  if (usage.length > 0) parts.push(...usage);
  return parts.filter((part) => part.trim().length > 0).join(' · ');
}

function sessionUsageParts(session: ChatSessionSummary): string[] {
  const parts: string[] = [];
  if (typeof session.total_cost_usd === 'number' && session.total_cost_usd > 0) {
    parts.push(formatUsd(session.total_cost_usd));
  }
  if (typeof session.total_tokens === 'number' && session.total_tokens > 0) {
    parts.push(`${compactNumber(session.total_tokens)} tok`);
  }
  if (typeof session.turns_used === 'number' && session.turns_used > 0) {
    parts.push(`${session.turns_used.toLocaleString()} turn${session.turns_used === 1 ? '' : 's'}`);
  }
  return parts;
}

function compactNumber(value: number): string {
  if (value >= 1_000_000) return `${trimFixed(value / 1_000_000)}M`;
  if (value >= 1_000) return `${trimFixed(value / 1_000)}K`;
  return String(value);
}

function trimFixed(value: number): string {
  return value.toFixed(value >= 10 ? 0 : 1).replace(/\.0$/, '');
}

function formatUsd(value: number): string {
  if (value >= 1) return `$${value.toFixed(2)}`;
  if (value >= 0.01) return `$${value.toFixed(3)}`;
  return `$${value.toFixed(4)}`;
}
