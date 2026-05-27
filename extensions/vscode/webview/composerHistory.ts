export type ComposerHistoryDirection = 'previous' | 'next';

const MAX_HISTORY_ENTRIES = 50;

export class ComposerHistory {
  private readonly entriesBySession = new Map<string, string[]>();
  private readonly cursorBySession = new Map<string, number>();
  private readonly draftBySession = new Map<string, string>();

  record(sessionKey: string, value: string): void {
    const entry = value.trim();
    if (!entry) return;
    const entries = (this.entriesBySession.get(sessionKey) ?? []).filter(
      (item) => item !== entry,
    );
    entries.push(entry);
    this.entriesBySession.set(sessionKey, entries.slice(-MAX_HISTORY_ENTRIES));
    this.resetNavigation(sessionKey);
  }

  navigate(
    sessionKey: string,
    direction: ComposerHistoryDirection,
    currentDraft: string,
  ): string | undefined {
    const entries = this.entriesBySession.get(sessionKey) ?? [];
    if (entries.length === 0) return undefined;

    const currentCursor = this.cursorBySession.get(sessionKey);
    if (direction === 'previous') {
      if (currentCursor === undefined) {
        this.draftBySession.set(sessionKey, currentDraft);
        const nextCursor = entries.length - 1;
        this.cursorBySession.set(sessionKey, nextCursor);
        return entries[nextCursor];
      }
      const nextCursor = Math.max(0, currentCursor - 1);
      this.cursorBySession.set(sessionKey, nextCursor);
      return entries[nextCursor];
    }

    if (currentCursor === undefined) return undefined;
    if (currentCursor >= entries.length - 1) {
      const draft = this.draftBySession.get(sessionKey) ?? '';
      this.resetNavigation(sessionKey);
      return draft;
    }
    const nextCursor = currentCursor + 1;
    this.cursorBySession.set(sessionKey, nextCursor);
    return entries[nextCursor];
  }

  resetNavigation(sessionKey: string): void {
    this.cursorBySession.delete(sessionKey);
    this.draftBySession.delete(sessionKey);
  }

  entries(sessionKey: string): string[] {
    return [...(this.entriesBySession.get(sessionKey) ?? [])];
  }
}

export function canNavigateComposerHistory(
  value: string,
  selectionStart: number,
  selectionEnd: number,
  direction: ComposerHistoryDirection,
): boolean {
  if (selectionStart !== selectionEnd) return false;
  if (direction === 'previous') {
    return !value.slice(0, selectionStart).includes('\n');
  }
  return !value.slice(selectionEnd).includes('\n');
}
