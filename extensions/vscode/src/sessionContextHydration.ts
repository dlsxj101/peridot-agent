import type { CommandResultView } from './types';

export function commandResultCanHydrateSessionContext(
  result: CommandResultView,
  activeClientSessionId?: string,
  activeDaemonSessionId?: string,
): boolean {
  if (result.kind !== 'session_show') return true;
  const target = normalizeId(result.session_id);
  if (!target) return false;
  return target === normalizeId(activeClientSessionId) || target === normalizeId(activeDaemonSessionId);
}

function normalizeId(value: string | undefined): string | undefined {
  const trimmed = value?.trim();
  return trimmed && trimmed.length > 0 ? trimmed : undefined;
}
