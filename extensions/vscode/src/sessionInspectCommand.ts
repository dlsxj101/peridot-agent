import type { DaemonSessionSummary } from './types';
import { sessionContextDetail } from './sessionContextDetail';
import { sessionUsageDescription } from './sessionUsage';

export interface SessionTargetChoice {
  id: string;
  label: string;
  description?: string;
  detail?: string;
}

export function sessionCountSlashCommand(): string {
  return '/session count';
}

export function sessionNewSlashCommand(task?: string): string {
  const trimmedTask = task?.trim();
  return trimmedTask ? `/session new ${trimmedTask}` : '/session new';
}

export function sessionTargetChoices(sessions: DaemonSessionSummary[]): SessionTargetChoice[] {
  const choices: SessionTargetChoice[] = [];
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

export function sessionShowSlashCommand(target: string): string {
  return `/session show ${sessionTargetArg(target)}`;
}

export function sessionSwitchSlashCommand(target: string): string {
  return `/session switch ${sessionTargetArg(target)}`;
}

export function sessionCloseSlashCommand(target: string): string {
  return `/session close ${sessionTargetArg(target)}`;
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
  return sessionUsageDescription(session);
}

function sessionDetail(session: DaemonSessionSummary, fallbackId: string): string | undefined {
  return sessionContextDetail(session, fallbackId);
}
