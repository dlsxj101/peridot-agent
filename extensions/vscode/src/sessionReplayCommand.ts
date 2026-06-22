import type { DaemonSessionSummary } from './types';
import { sessionContextDetail } from './sessionContextDetail';
import { sessionUsageDescription } from './sessionUsage';
import { parsePositiveInteger } from './util';

export interface SessionReplayChoice {
  id: string;
  label: string;
  description?: string;
  detail?: string;
}

export function sessionReplayChoices(sessions: DaemonSessionSummary[]): SessionReplayChoice[] {
  const choices: SessionReplayChoice[] = [];
  const seen = new Set<string>();
  for (const session of sessions) {
    const id = session.id.trim();
    if (!id || seen.has(id)) continue;
    seen.add(id);
    const description = sessionDescription(session);
    const detail = sessionDetail(session, id);
    choices.push({
      id,
      label: sessionTitle(session) ?? id,
      ...(description ? { description } : {}),
      ...(detail ? { detail } : {}),
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
  return parsePositiveInteger(value, 'Enter a positive whole number, or leave empty for the full replay.');
}

function sessionTitle(session: DaemonSessionSummary): string | undefined {
  return session.title ?? session.last_task ?? session.summary ?? session.id;
}

function sessionDescription(session: DaemonSessionSummary): string | undefined {
  return sessionUsageDescription(session);
}

function sessionDetail(session: DaemonSessionSummary, fallbackId: string): string | undefined {
  return sessionContextDetail(session, fallbackId);
}
