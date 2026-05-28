import type { DaemonSessionSummary } from './types';
import { sessionUsageDescription } from './sessionUsage';

export interface SessionReplayChoice {
  id: string;
  label: string;
  description?: string;
}

export function sessionReplayChoices(sessions: DaemonSessionSummary[]): SessionReplayChoice[] {
  const choices: SessionReplayChoice[] = [];
  const seen = new Set<string>();
  for (const session of sessions) {
    const id = session.id.trim();
    if (!id || seen.has(id)) continue;
    seen.add(id);
    choices.push({
      id,
      label: sessionTitle(session) ?? id,
      ...(sessionDescription(session) ? { description: sessionDescription(session) } : {}),
    });
  }
  return choices;
}

export function sessionReplaySlashCommand(target: string, last?: number): string {
  const id = target.trim();
  if (!id) {
    throw new Error('Session id is required.');
  }
  if (/\s/.test(id)) {
    throw new Error('Session id cannot contain whitespace.');
  }
  if (last !== undefined && (!Number.isInteger(last) || last <= 0)) {
    throw new Error('--last must be a positive integer.');
  }
  const args = ['/session replay', id];
  if (last !== undefined) {
    args.push('--last', String(last));
  }
  return args.join(' ');
}

export function parseReplayLastInput(value: string): number | undefined {
  const trimmed = value.trim();
  if (!trimmed) return undefined;
  const parsed = Number(trimmed);
  if (!Number.isInteger(parsed) || parsed <= 0) {
    throw new Error('Enter a positive whole number, or leave empty for the full replay.');
  }
  return parsed;
}

function sessionTitle(session: DaemonSessionSummary): string | undefined {
  return session.title ?? session.last_task ?? session.summary ?? session.id;
}

function sessionDescription(session: DaemonSessionSummary): string | undefined {
  return sessionUsageDescription(session);
}
