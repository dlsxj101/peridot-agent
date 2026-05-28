export function sessionNoteSlashCommand(note: string): string {
  const trimmed = note.trim();
  if (!trimmed) {
    throw new Error('Note text is required.');
  }
  return `/note ${trimmed}`;
}

export function sessionNotesSlashCommand(last?: number): string {
  if (last === undefined) return '/notes';
  if (!Number.isInteger(last) || last <= 0) {
    throw new Error('Last note count must be a positive integer.');
  }
  return `/notes last ${last}`;
}

export function sessionNotesClearSlashCommand(): string {
  return '/notes clear';
}

export function parseNotesLastInput(value: string): number | undefined {
  const trimmed = value.trim();
  if (!trimmed) return undefined;
  if (!/^[1-9]\d*$/.test(trimmed)) {
    throw new Error('Enter a positive integer, or leave blank for all notes.');
  }
  return Number(trimmed);
}
