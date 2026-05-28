export const SESSION_LIST_STATUSES = ['idle', 'running', 'suspended', 'done', 'failed'] as const;

export type SessionListStatus = (typeof SESSION_LIST_STATUSES)[number];

export interface SessionListStatusChoice {
  label: string;
  description?: string;
  status?: SessionListStatus;
}

export function sessionListStatusChoices(): SessionListStatusChoice[] {
  return [
    { label: 'All sessions', description: 'Show every persisted session' },
    { label: 'Idle', description: 'Saved inactive sessions', status: 'idle' },
    { label: 'Running', description: 'Sessions still marked running', status: 'running' },
    { label: 'Suspended', description: 'Interrupted or crash-recovered sessions', status: 'suspended' },
    { label: 'Done', description: 'Completed sessions', status: 'done' },
    { label: 'Failed', description: 'Failed sessions', status: 'failed' },
  ];
}

export function sessionListSlashCommand(status?: string): string {
  const normalized = status?.trim().toLowerCase();
  if (!normalized) return '/session list';
  if (!isSessionListStatus(normalized)) {
    throw new Error('--status must be one of idle, running, suspended, done, or failed.');
  }
  return `/session list --status ${normalized}`;
}

function isSessionListStatus(value: string): value is SessionListStatus {
  return SESSION_LIST_STATUSES.some((status) => status === value);
}
