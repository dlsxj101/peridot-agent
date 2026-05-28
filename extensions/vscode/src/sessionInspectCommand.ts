import type { DaemonSessionSummary } from './types';

export interface SessionTargetChoice {
  id: string;
  label: string;
  description?: string;
}

export function sessionCountSlashCommand(): string {
  return '/session count';
}

export function sessionTargetChoices(sessions: DaemonSessionSummary[]): SessionTargetChoice[] {
  const choices: SessionTargetChoice[] = [];
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

export function sessionShowSlashCommand(target: string): string {
  return `/session show ${quotedSessionTarget(target)}`;
}

export function sessionLocateSlashCommand(target: string): string {
  return `/session locate ${quotedSessionTarget(target)}`;
}

export function sessionResumeSlashCommand(target: string): string {
  return `/session resume ${quotedSessionTarget(target)}`;
}

function quotedSessionTarget(target: string): string {
  const id = target.trim();
  if (!id) {
    throw new Error('Session id is required.');
  }
  return shellQuote(id);
}

function sessionTitle(session: DaemonSessionSummary): string | undefined {
  return session.title ?? session.last_task ?? session.summary ?? session.id;
}

function sessionDescription(session: DaemonSessionSummary): string | undefined {
  const parts = [session.status, session.running ? 'running' : undefined].filter(
    (part): part is string => typeof part === 'string' && part.trim().length > 0,
  );
  return parts.length > 0 ? parts.join(' · ') : undefined;
}

function shellQuote(value: string): string {
  return `'${value.replace(/'/g, "'\\''")}'`;
}
