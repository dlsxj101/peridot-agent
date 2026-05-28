import type { CommandResultItem, CommandResultView, NoteSummary } from './types';

export function noteSummaryFromCommandResult(
  result: CommandResultView,
  existing?: NoteSummary,
): NoteSummary | undefined {
  if (result.kind === 'note') {
    return normalizeNoteSummary({
      count: Math.max(0, existing?.count ?? 0) + 1,
      latest: latestNoteText(result) ?? noteTextFromMessage(result.message) ?? existing?.latest,
    });
  }

  if (result.kind === 'notes') {
    return normalizeNoteSummary({
      count: numberField(result.total) ?? noteItems(result).length,
      latest: latestNoteText(result),
    });
  }

  if (result.kind === 'notes_clear') {
    return { count: 0 };
  }

  return undefined;
}

export function normalizeNoteSummary(value: unknown): NoteSummary | undefined {
  if (!isRecord(value)) return undefined;
  const count = numberField(value.count);
  if (count === undefined) return undefined;
  const latest = stringField(value.latest);
  return {
    count: Math.max(0, Math.floor(count)),
    ...(latest ? { latest } : {}),
  };
}

function latestNoteText(result: CommandResultView): string | undefined {
  const notes = noteItems(result);
  const latest = notes[notes.length - 1];
  return noteText(latest);
}

function noteItems(result: CommandResultView): CommandResultItem[] {
  return [
    ...(result.note ? [result.note] : []),
    ...(Array.isArray(result.notes) ? result.notes : []),
    ...(Array.isArray(result.items)
      ? result.items.filter((item) => item.source === 'note' || noteText(item) !== undefined)
      : []),
  ];
}

function noteText(note: CommandResultItem | undefined): string | undefined {
  return stringField(note?.text) ?? stringField(note?.detail);
}

function noteTextFromMessage(message: string | undefined): string | undefined {
  const value = stringField(message);
  if (!value) return undefined;
  return value.toLowerCase().startsWith('note:') ? stringField(value.slice(5)) : value;
}

function numberField(value: unknown): number | undefined {
  return typeof value === 'number' && Number.isFinite(value) ? value : undefined;
}

function stringField(value: unknown): string | undefined {
  if (typeof value !== 'string') return undefined;
  const trimmed = value.trim();
  return trimmed.length > 0 ? trimmed : undefined;
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null;
}
