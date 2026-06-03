import type { ChatSessionSummary } from '../src/types';
import { sessionContextParts } from '../src/sessionContextDetail';
import { sessionUsageParts } from '../src/sessionUsage';
import { t } from './i18n';

export function sessionMenuSubtitle(session: ChatSessionSummary): string {
  const parts = [session.running ? t('In progress', '진행 중') : session.status];
  const usage = sessionUsageParts(session);
  if (usage.length > 0) parts.push(...usage);
  parts.push(...sessionContextParts(session));
  return parts.filter((part) => part.trim().length > 0).join(' · ');
}
