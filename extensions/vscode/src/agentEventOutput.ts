import { isRecord } from './util';

export function formatAgentEventForOutput(sessionId: string, event: unknown): string {
  if (!isRecord(event)) {
    return `[${sessionId}] event ${json(event)}`;
  }

  const kind = typeof event.kind === 'string' ? event.kind : 'unknown';
  switch (kind) {
    case 'started':
    case 'run_started':
      return `[${sessionId}] ${kind}: ${stringField(event, 'task')}`;
    case 'assistant_delta':
      return `[${sessionId}] assistant: ${stringField(event, 'delta')}`;
    case 'tool_started':
      return `[${sessionId}] tool started: ${stringField(event, 'name')}`;
    case 'tool_finished':
      return `[${sessionId}] tool finished: ${stringField(event, 'name')}`;
    case 'finished':
      return `[${sessionId}] finished: ${json(event)}`;
    case 'error':
      return `[${sessionId}] error: ${stringField(event, 'message')}`;
    case 'recovery':
      return `[${sessionId}] recovery: ${stringField(event, 'message')}`;
    case 'interrupted':
      return `[${sessionId}] interrupted: ${stringField(event, 'stage')}`;
    default:
      return `[${sessionId}] ${kind}: ${json(event)}`;
  }
}

function stringField(record: Record<string, unknown>, key: string): string {
  const value = record[key];
  return typeof value === 'string' ? value : json(value);
}


function json(value: unknown): string {
  try {
    const serialized = JSON.stringify(value);
    return serialized === undefined ? String(value) : serialized;
  } catch {
    return String(value);
  }
}
