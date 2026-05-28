import type { CommandResultView, DaemonSessionSummary, ExportedArtifactView } from './types';
import { sessionContextDetail } from './sessionContextDetail';
import { activeSessionUsageDescription, sessionUsageDescription } from './sessionUsage';

export interface SessionExportChoice {
  id: string;
  label: string;
  description?: string;
  detail?: string;
}

export function sessionExportChoices(
  sessions: DaemonSessionSummary[],
  currentId?: string,
): SessionExportChoice[] {
  const choices: SessionExportChoice[] = [];
  const seen = new Set<string>();
  const push = (id: string, label?: string, description?: string, detail?: string) => {
    const trimmed = id.trim();
    if (!trimmed || seen.has(trimmed)) return;
    seen.add(trimmed);
    choices.push({
      id: trimmed,
      label: label?.trim() || trimmed,
      ...(description?.trim() ? { description: description.trim() } : {}),
      ...(detail?.trim() ? { detail: detail.trim() } : {}),
    });
  };
  const current = currentId?.trim();
  if (current) {
    const match = sessions.find((session) => session.id === current);
    push(
      current,
      sessionTitle(match) ?? current,
      activeSessionUsageDescription(match),
      sessionContextDetail(match, current),
    );
  }
  sessions.forEach((session) => {
    push(
      session.id,
      sessionTitle(session),
      sessionDescription(session),
      sessionContextDetail(session, session.id),
    );
  });
  return choices;
}

export function sessionExportDirectoryName(sessionId: string): string {
  const sanitized = sessionId.replace(/[^A-Za-z0-9._-]+/g, '-').replace(/^-+|-+$/g, '');
  return `peridot-session-${sanitized || 'session'}`;
}

export function exportedArtifactsFromPayload(payload: unknown): ExportedArtifactView[] {
  if (!isRecord(payload) || !Array.isArray(payload.artifacts)) return [];
  return payload.artifacts
    .filter(isRecord)
    .map((artifact) => ({
      class: typeof artifact.class === 'string' ? artifact.class : 'artifact',
      path: typeof artifact.path === 'string' ? artifact.path : 'artifact',
      count: typeof artifact.count === 'number' ? artifact.count : 0,
    }));
}

export function sessionExportCommandResult(
  payload: unknown,
  sessionId: string,
  fallbackDestination: string,
): CommandResultView {
  const artifacts = exportedArtifactsFromPayload(payload);
  const destination =
    isRecord(payload) && typeof payload.destination === 'string'
      ? payload.destination
      : fallbackDestination;
  const files = isRecord(payload) && Array.isArray(payload.files)
    ? payload.files.filter((file): file is string => typeof file === 'string')
    : undefined;

  return {
    kind: 'session_export',
    title: 'Session Artifact Export',
    command: 'peridot session export',
    message: `Exported ${artifacts.length} artifact files from ${sessionId} to ${destination}`,
    destination,
    artifacts,
    ...(files ? { files } : {}),
    items: [
      { label: 'Session', detail: sessionId, source: 'session' },
      { label: 'Destination', detail: destination, source: 'directory' },
      ...(files ?? []).map((file) => ({
        label: file,
        detail: 'full copy',
        source: 'full_copy',
      })),
      ...artifacts.map((artifact) => ({
        label: artifact.path ?? 'artifact',
        detail: `${artifact.class ?? 'artifact'} · ${artifact.count ?? 0} entries`,
        source: 'artifact',
      })),
    ],
  };
}

function sessionTitle(session: DaemonSessionSummary | undefined): string | undefined {
  if (!session) return undefined;
  return session.title ?? session.last_task ?? session.summary ?? session.id;
}

function sessionDescription(session: DaemonSessionSummary): string | undefined {
  return sessionUsageDescription(session);
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null;
}
