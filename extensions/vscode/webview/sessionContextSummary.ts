import type { CommandResultView } from '../src/types';

export interface SessionContextSummary {
  noteCount?: number;
  latestNote?: string;
  attachmentCount?: number;
  attachmentPaths: string[];
}

export function sessionContextSummary(result: CommandResultView | undefined): SessionContextSummary {
  const noteCount =
    typeof result?.notes_count === 'number' && Number.isFinite(result.notes_count)
      ? Math.max(0, Math.floor(result.notes_count))
      : undefined;
  const latestNote =
    typeof result?.last_note === 'string' && result.last_note.trim().length > 0
      ? result.last_note.trim()
      : undefined;
  const attachmentPaths = Array.isArray(result?.attachment_paths)
    ? result.attachment_paths
        .filter((path): path is string => typeof path === 'string')
        .map((path) => path.trim())
        .filter((path) => path.length > 0)
    : [];
  const attachmentCount =
    typeof result?.attachment_count === 'number' && Number.isFinite(result.attachment_count)
      ? Math.max(0, Math.floor(result.attachment_count))
      : attachmentPaths.length > 0
        ? attachmentPaths.length
        : undefined;
  return {
    ...(noteCount !== undefined ? { noteCount } : {}),
    ...(latestNote ? { latestNote } : {}),
    ...(attachmentCount !== undefined ? { attachmentCount } : {}),
    attachmentPaths: dedupeCaseInsensitive(attachmentPaths),
  };
}

export function sessionContextSummaryChips(summary: SessionContextSummary): string[] {
  const chips: string[] = [];
  if (summary.noteCount !== undefined) {
    chips.push(`${summary.noteCount} ${summary.noteCount === 1 ? 'note' : 'notes'}`);
  }
  if (summary.attachmentCount !== undefined) {
    chips.push(
      `${summary.attachmentCount} ${summary.attachmentCount === 1 ? 'attachment' : 'attachments'}`,
    );
  }
  return chips;
}

function dedupeCaseInsensitive(values: string[]): string[] {
  const unique = new Map<string, string>();
  for (const value of values) {
    const key = value.toLowerCase();
    if (!unique.has(key)) unique.set(key, value);
  }
  return [...unique.values()];
}
