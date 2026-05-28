import type { ChatSessionSummary, HudState } from '../src/types';

export type RunMetricTone = 'normal' | 'warn' | 'critical' | 'muted';

export interface RunMetricChip {
  label: string;
  value: string;
  tone: RunMetricTone;
  title: string;
}

export function runMetricChips(
  hud: HudState,
  sessions: ChatSessionSummary[] = [],
): RunMetricChip[] {
  const chips: RunMetricChip[] = [];
  const usage = hud.usage;
  const committee = committeeTotals(hud);
  if (usage) {
    const totalTokens =
      usage.inputTokens +
      usage.outputTokens +
      (usage.cacheReadTokens ?? 0) +
      (usage.cacheCreationTokens ?? 0);
    if (totalTokens > 0) {
      chips.push({
        label: 'Tokens',
        value: compactNumber(totalTokens),
        tone: 'muted',
        title: [
          `${totalTokens.toLocaleString()} executor tokens`,
          `input ${usage.inputTokens.toLocaleString()}`,
          `output ${usage.outputTokens.toLocaleString()}`,
          usage.cacheReadTokens ? `cache read ${usage.cacheReadTokens.toLocaleString()}` : undefined,
          usage.cacheCreationTokens ? `cache write ${usage.cacheCreationTokens.toLocaleString()}` : undefined,
          committee.tokens > 0 ? `committee ${committee.tokens.toLocaleString()}` : undefined,
        ].filter(Boolean).join(' · '),
      });
    }
  }

  const executorCost = usage?.costUsd ?? 0;
  const totalCost = executorCost + committee.costUsd;
  if (totalCost > 0) {
    chips.push({
      label: 'Cost',
      value: formatUsd(totalCost),
      tone: 'normal',
      title: [
        `total ${formatUsd(totalCost)}`,
        executorCost > 0 ? `executor ${formatUsd(executorCost)}` : undefined,
        committee.costUsd > 0 ? `committee ${formatUsd(committee.costUsd)}` : undefined,
      ].filter(Boolean).join(' · '),
    });
  }

  const aggregate = aggregateSessionUsage(sessions, hud);
  const currentTokens = usage
    ? usage.inputTokens +
      usage.outputTokens +
      (usage.cacheReadTokens ?? 0) +
      (usage.cacheCreationTokens ?? 0) +
      committee.tokens
    : committee.tokens;
  if (
    aggregate.sessions > 1 &&
    (aggregate.costUsd > totalCost + 0.000_001 || aggregate.tokens > currentTokens)
  ) {
    chips.push({
      label: 'All',
      value: aggregate.costUsd > 0 ? formatUsd(aggregate.costUsd) : compactNumber(aggregate.tokens),
      tone: 'muted',
      title: [
        `${aggregate.tokens.toLocaleString()} tokens`,
        aggregate.costUsd > 0 ? `${formatUsd(aggregate.costUsd)} total` : undefined,
        `${aggregate.sessions} sessions`,
      ].filter(Boolean).join(' · '),
    });
  }

  const budget = hud.budget;
  if (budget?.costLimit && budget.costLimit > 0) {
    const pct = budget.costUsed / budget.costLimit;
    chips.push({
      label: 'Budget',
      value: `${Math.round(pct * 100)}%`,
      tone: thresholdTone(pct),
      title: `${formatUsd(budget.costUsed)} / ${formatUsd(budget.costLimit)}`,
    });
  }

  if (budget && (budget.turnsLimit || budget.turnsUsed > 0)) {
    const hasLimit = typeof budget.turnsLimit === 'number' && budget.turnsLimit > 0;
    const pct = hasLimit ? budget.turnsUsed / (budget.turnsLimit as number) : 0;
    chips.push({
      label: 'Turns',
      value: hasLimit ? `${budget.turnsUsed}/${budget.turnsLimit}` : String(budget.turnsUsed),
      tone: hasLimit ? thresholdTone(pct) : 'muted',
      title: hasLimit
        ? `${budget.turnsUsed.toLocaleString()} / ${budget.turnsLimit?.toLocaleString()} turns`
        : `${budget.turnsUsed.toLocaleString()} turns used`,
    });
  }

  return chips;
}

function aggregateSessionUsage(
  sessions: ChatSessionSummary[],
  hud: HudState,
): { tokens: number; costUsd: number; sessions: number } {
  const current = currentHudUsage(hud);
  return sessions.reduce(
    (total, session) => {
      const tokens = Math.max(session.total_tokens ?? 0, session.active ? current.tokens : 0);
      const costUsd = Math.max(session.total_cost_usd ?? 0, session.active ? current.costUsd : 0);
      if (tokens <= 0 && costUsd <= 0) return total;
      return {
        tokens: total.tokens + tokens,
        costUsd: total.costUsd + costUsd,
        sessions: total.sessions + 1,
      };
    },
    { tokens: 0, costUsd: 0, sessions: 0 },
  );
}

function currentHudUsage(hud: HudState): { tokens: number; costUsd: number } {
  const usage = hud.usage;
  const committee = committeeTotals(hud);
  const executorTokens = usage
    ? usage.inputTokens +
      usage.outputTokens +
      (usage.cacheReadTokens ?? 0) +
      (usage.cacheCreationTokens ?? 0)
    : 0;
  return {
    tokens: executorTokens + committee.tokens,
    costUsd: (usage?.costUsd ?? 0) + committee.costUsd,
  };
}

function committeeTotals(hud: HudState): { tokens: number; costUsd: number } {
  return Object.values(hud.committee ?? {}).reduce(
    (total, role) => ({
      tokens: total.tokens + role.tokens,
      costUsd: total.costUsd + role.costUsd,
    }),
    { tokens: 0, costUsd: 0 },
  );
}

function thresholdTone(pct: number): RunMetricTone {
  if (pct >= 0.9) return 'critical';
  if (pct >= 0.75) return 'warn';
  return 'normal';
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
