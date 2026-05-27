/**
 * Minimal session reconciliation helpers kept free of VS Code APIs so the
 * sidebar's daemon inventory behavior can be covered by ordinary Node tests.
 */

export interface ReconcileLocalSession {
  id: string;
  daemonSessionId?: string;
}

export interface ReconcileRemoteSession {
  id?: string | null;
}

export function staleDaemonBackedSessionIds(
  localSessions: Iterable<ReconcileLocalSession>,
  remoteSessions: Iterable<ReconcileRemoteSession>,
): string[] {
  const remoteIds = new Set<string>();
  for (const remote of remoteSessions) {
    const id = remote.id?.trim();
    if (id) remoteIds.add(id);
  }

  const staleIds: string[] = [];
  for (const session of localSessions) {
    if (session.daemonSessionId && !remoteIds.has(session.daemonSessionId)) {
      staleIds.push(session.id);
    }
  }
  return staleIds;
}
