export interface FileMentionContext {
  tokenStart: number;
  cursor: number;
  query: string;
  options: string[];
}

export const FILE_MENTION_LIMIT = 8;

export function fileMentionContext(
  input: string,
  cursor: number,
  files: string[],
): FileMentionContext | undefined {
  const token = currentFileMentionToken(input, cursor);
  if (!token) return undefined;
  const options = filterFileMentionPaths(files, token.query);
  if (options.length === 0) return undefined;
  return { ...token, cursor, options };
}

export function acceptFileMention(input: string, context: FileMentionContext, selected: number): string {
  const option = context.options[Math.min(Math.max(0, selected), context.options.length - 1)];
  if (!option) return input;
  return `${input.slice(0, context.tokenStart)}@${option} ${input.slice(context.cursor)}`;
}

export function currentFileMentionToken(
  input: string,
  cursor: number,
): { tokenStart: number; query: string } | undefined {
  const safeCursor = Math.min(Math.max(0, cursor), input.length);
  let index = safeCursor;
  while (index > 0) {
    const previous = index - 1;
    const ch = input[previous];
    if (ch === '@') {
      const validStart =
        previous === 0 || input[previous - 1] === ' ' || input[previous - 1] === '\t' || input[previous - 1] === '\n';
      if (!validStart) return undefined;
      const query = input.slice(previous + 1, safeCursor);
      if (/\s/.test(query)) return undefined;
      return { tokenStart: previous, query };
    }
    if (/\s/.test(ch ?? '')) return undefined;
    index = previous;
  }
  return undefined;
}

export function filterFileMentionPaths(files: string[], query: string): string[] {
  const normalized = [...new Set(files.map((file) => file.trim()).filter((file) => file.length > 0))]
    .sort((a, b) => a.localeCompare(b));
  if (query.length === 0) return normalized.slice(0, FILE_MENTION_LIMIT);
  const needle = query.toLowerCase();
  const exactSuffix: string[] = [];
  const startsWith: string[] = [];
  const basenameContains: string[] = [];
  const pathContains: string[] = [];
  for (const file of normalized) {
    const lower = file.toLowerCase();
    const basename = lower.split('/').pop() ?? lower;
    if (basename === needle) {
      exactSuffix.push(file);
    } else if (basename.startsWith(needle)) {
      startsWith.push(file);
    } else if (basename.includes(needle)) {
      basenameContains.push(file);
    } else if (lower.includes(needle)) {
      pathContains.push(file);
    }
  }
  return [...exactSuffix, ...startsWith, ...basenameContains, ...pathContains].slice(
    0,
    FILE_MENTION_LIMIT,
  );
}
