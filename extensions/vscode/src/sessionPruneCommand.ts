import { parsePositiveInteger } from './util';

export const SESSION_PRUNE_STATUSES = ['idle', 'running', 'suspended', 'done', 'failed'] as const;

export type SessionPruneStatus = (typeof SESSION_PRUNE_STATUSES)[number];

export interface SessionPruneOptions {
  status?: string;
  olderThanDays?: number;
  dryRun?: boolean;
}

export interface SessionPruneStatusChoice {
  label: string;
  description?: string;
  status?: SessionPruneStatus;
}

export function sessionPruneStatusChoices(): SessionPruneStatusChoice[] {
  return [
    { label: 'All statuses', description: 'Match every persisted session' },
    { label: 'Done', description: 'Completed sessions', status: 'done' },
    { label: 'Failed', description: 'Failed sessions', status: 'failed' },
    { label: 'Suspended', description: 'Interrupted or crash-recovered sessions', status: 'suspended' },
    { label: 'Idle', description: 'Saved inactive sessions', status: 'idle' },
    { label: 'Running', description: 'Sessions still marked running', status: 'running' },
  ];
}

export function sessionPruneSlashCommand(options: SessionPruneOptions = {}): string {
  const args = ['/session prune'];
  const status = options.status?.trim().toLowerCase();
  if (status) {
    if (!isSessionPruneStatus(status)) {
      throw new Error('--status must be one of idle, running, suspended, done, or failed.');
    }
    args.push('--status', status);
  }
  if (options.olderThanDays !== undefined) {
    if (!Number.isInteger(options.olderThanDays) || options.olderThanDays <= 0) {
      throw new Error('--older-than-days must be a positive integer.');
    }
    args.push('--older-than-days', String(options.olderThanDays));
  }
  if (options.dryRun === true) {
    args.push('--dry-run');
  }
  return args.join(' ');
}

export function parsePruneOlderThanDaysInput(value: string): number | undefined {
  return parsePositiveInteger(value, 'Enter a positive whole number, or leave empty for no age filter.');
}

function isSessionPruneStatus(value: string): value is SessionPruneStatus {
  return SESSION_PRUNE_STATUSES.some((status) => status === value);
}
