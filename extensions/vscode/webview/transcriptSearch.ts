// Pure transcript-search helpers, kept free of DOM/`acquireVsCodeApi` side
// effects so they can be unit-tested (webview/index.ts can't be imported in a
// plain node test because it calls acquireVsCodeApi() at module load).

import type { TranscriptItem } from '../src/types';

/** Whether a transcript item matches a (case-insensitive) search query. An
 *  empty/whitespace query matches everything. Searches the human-readable
 *  text fields: message text, tool name, path, detail, and result summary. */
export function transcriptItemMatchesQuery(item: TranscriptItem, query: string): boolean {
  const needle = query.trim().toLowerCase();
  if (needle.length === 0) return true;
  const haystack = [item.text, item.toolName, item.path, item.detail, item.toolResultSummary]
    .filter((value): value is string => typeof value === 'string')
    .join('\n')
    .toLowerCase();
  return haystack.includes(needle);
}

/** Count transcript items matching `query`. Zero for an empty transcript;
 *  the full count for an empty query. */
export function countTranscriptMatches(items: readonly TranscriptItem[], query: string): number {
  if (query.trim().length === 0) return items.length;
  let count = 0;
  for (const item of items) {
    if (transcriptItemMatchesQuery(item, query)) count += 1;
  }
  return count;
}
