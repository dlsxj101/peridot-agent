import type { CommandResultView, DaemonSessionSummary } from './types';

export function commandResultSessionContextFields(
  result: CommandResultView,
): Pick<
  DaemonSessionSummary,
  'notes_count' | 'last_note' | 'attachment_count' | 'attachment_paths'
> {
  const summary: Pick<
    DaemonSessionSummary,
    'notes_count' | 'last_note' | 'attachment_count' | 'attachment_paths'
  > = {};
  if (typeof result.notes_count === 'number') summary.notes_count = result.notes_count;
  if (typeof result.last_note === 'string' || result.last_note === null) {
    summary.last_note = result.last_note;
  }
  if (typeof result.attachment_count === 'number') {
    summary.attachment_count = result.attachment_count;
  }
  if (Array.isArray(result.attachment_paths)) {
    summary.attachment_paths = result.attachment_paths.filter(
      (path): path is string => typeof path === 'string',
    );
  }
  return summary;
}
