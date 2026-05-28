import type { DaemonSessionSummary } from './types';

export interface SessionExportChoice {
  id: string;
  label: string;
  description?: string;
}

export function sessionExportChoices(
  sessions: DaemonSessionSummary[],
  currentId?: string,
): SessionExportChoice[] {
  const choices: SessionExportChoice[] = [];
  const seen = new Set<string>();
  const push = (id: string, label?: string, description?: string) => {
    const trimmed = id.trim();
    if (!trimmed || seen.has(trimmed)) return;
    seen.add(trimmed);
    choices.push({
      id: trimmed,
      label: label?.trim() || trimmed,
      ...(description?.trim() ? { description: description.trim() } : {}),
    });
  };
  const current = currentId?.trim();
  if (current) {
    const match = sessions.find((session) => session.id === current);
    push(current, sessionTitle(match) ?? current, 'active session');
  }
  sessions.forEach((session) => {
    push(session.id, sessionTitle(session), sessionDescription(session));
  });
  return choices;
}

export function sessionExportDirectoryName(sessionId: string): string {
  const sanitized = sessionId.replace(/[^A-Za-z0-9._-]+/g, '-').replace(/^-+|-+$/g, '');
  return `peridot-session-${sanitized || 'session'}`;
}

function sessionTitle(session: DaemonSessionSummary | undefined): string | undefined {
  if (!session) return undefined;
  return session.title ?? session.last_task ?? session.summary ?? session.id;
}

function sessionDescription(session: DaemonSessionSummary): string | undefined {
  const parts = [session.status, session.running ? 'running' : undefined].filter(
    (part): part is string => typeof part === 'string' && part.trim().length > 0,
  );
  return parts.length > 0 ? parts.join(' · ') : undefined;
}
