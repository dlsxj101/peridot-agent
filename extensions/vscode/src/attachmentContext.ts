import type {
  AttachmentView,
  CommandResultItem,
  CommandResultView,
  DaemonSessionSummary,
} from './types';

export function attachmentPathsFromCommandResult(
  result: CommandResultView,
  existing: string[] = [],
): string[] | undefined {
  if (result.kind === 'attach') {
    return dedupeSorted([...existing, ...attachmentPathsFromResult(result)]);
  }
  if (result.kind === 'attachments') {
    return dedupeSorted(attachmentPathsFromResult(result));
  }
  if (result.kind === 'detach') {
    const remaining = attachmentPathsFromResult(result);
    if (Array.isArray(result.attachments)) return dedupeSorted(remaining);
    const removed = attachmentPathsFromAttachments(result.removed);
    if (removed.length === 0) return undefined;
    return dedupeSorted(
      existing.filter((path) => !removed.some((candidate) => candidate.toLowerCase() === path.toLowerCase())),
    );
  }
  if (result.kind === 'session_show') {
    return dedupeSorted(attachmentPathsFromResult(result));
  }
  return undefined;
}

export function normalizeAttachmentPaths(values: readonly unknown[] | undefined): string[] {
  if (!Array.isArray(values)) return [];
  return dedupeSorted(values.filter((value): value is string => typeof value === 'string'));
}

export function attachmentPathsFromDaemonSession(
  session: DaemonSessionSummary,
): string[] | undefined {
  if (!Array.isArray(session.attachment_paths)) return undefined;
  return normalizeAttachmentPaths(session.attachment_paths);
}

function attachmentPathsFromResult(result: CommandResultView): string[] {
  return dedupeSorted([
    ...normalizeAttachmentPaths(result.attachment_paths),
    ...attachmentPathsFromAttachments(result.attachment ? [result.attachment] : undefined),
    ...attachmentPathsFromAttachments(result.attachments),
    ...attachmentPathsFromItems(result.items),
  ]);
}

function attachmentPathsFromAttachments(attachments: AttachmentView[] | undefined): string[] {
  if (!Array.isArray(attachments)) return [];
  return attachments
    .map((attachment) => attachment.path?.trim() ?? '')
    .filter((path) => path.length > 0);
}

function attachmentPathsFromItems(items: CommandResultItem[] | undefined): string[] {
  if (!Array.isArray(items)) return [];
  return items
    .filter((item) => item.source === 'attachment')
    .map((item) => (item.path ?? item.label ?? '').trim())
    .filter((path) => path.length > 0);
}

function dedupeSorted(values: string[]): string[] {
  const unique = new Map<string, string>();
  values
    .map((value) => value.trim())
    .filter((value) => value.length > 0)
    .forEach((value) => {
      const key = value.toLowerCase();
      if (!unique.has(key)) unique.set(key, value);
    });
  return [...unique.values()].sort((a, b) => a.localeCompare(b));
}
