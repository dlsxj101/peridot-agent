export interface SessionUsageLike {
  status?: string;
  running?: boolean;
  total_tokens?: number;
  total_cost_usd?: number;
  turns_used?: number;
}

export function sessionUsageDescription(session: SessionUsageLike): string | undefined {
  const parts = sessionStatusParts(session);
  parts.push(...sessionUsageParts(session));
  return parts.length > 0 ? parts.join(' · ') : undefined;
}

export function activeSessionUsageDescription(session?: SessionUsageLike): string {
  const parts = ['active session'];
  if (session) parts.push(...sessionUsageParts(session));
  return parts.join(' · ');
}

export function sessionUsageParts(session: SessionUsageLike): string[] {
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

function sessionStatusParts(session: SessionUsageLike): string[] {
  const parts: string[] = [];
  const status = session.status?.trim();
  if (status) parts.push(status);
  if (session.running === true && status?.toLowerCase() !== 'running') {
    parts.push('running');
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
