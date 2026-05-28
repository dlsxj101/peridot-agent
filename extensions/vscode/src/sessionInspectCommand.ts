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
  return `/session show ${sessionTargetArg(target)}`;
}

export function sessionLocateSlashCommand(target: string): string {
  return `/session locate ${sessionTargetArg(target)}`;
}

export function sessionResumeSlashCommand(target: string): string {
  return `/session resume ${sessionTargetArg(target)}`;
}

export function sessionDeleteSlashCommand(target: string): string {
  return `/session delete ${sessionTargetArg(target)}`;
}

export function sessionRenameSlashCommand(target: string, title: string): string {
  const nextTitle = title.trim().replace(/\s+/g, ' ');
  if (!nextTitle) {
    throw new Error('Session title is required.');
  }
  return `/session rename ${sessionTargetArg(target)} ${nextTitle}`;
}

function sessionTargetArg(target: string): string {
  const id = target.trim();
  if (!id) {
    throw new Error('Session id is required.');
  }
  if (/\s/.test(id)) {
    throw new Error('Session id cannot contain whitespace.');
  }
  return id;
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
