export function isTerminalAgentEvent(event: unknown): boolean {
  if (!isRecord(event)) return false;
  return (
    event.kind === 'finished' ||
    event.kind === 'error' ||
    event.kind === 'approval_denied' ||
    event.kind === 'interrupted'
  );
}

export function terminalStatusForEvent(event: unknown): 'Finished' | 'Failed' | 'Interrupted' {
  if (!isRecord(event)) return 'Finished';
  if (event.kind === 'interrupted') return 'Interrupted';
  if (event.kind === 'error' || event.kind === 'approval_denied') return 'Failed';
  return 'Finished';
}

export function isAskUserWaitingEvent(event: unknown): boolean {
  return isRecord(event) && event.kind === 'ask_user_requested';
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null;
}
