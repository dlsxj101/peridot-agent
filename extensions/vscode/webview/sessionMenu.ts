import type { ChatSessionSummary } from '../src/types';
import { sessionUsageParts } from '../src/sessionUsage';

export function sessionMenuSubtitle(session: ChatSessionSummary): string {
  const parts = [session.running ? 'In progress' : session.status];
  const usage = sessionUsageParts(session);
  if (usage.length > 0) parts.push(...usage);
  return parts.filter((part) => part.trim().length > 0).join(' · ');
}
