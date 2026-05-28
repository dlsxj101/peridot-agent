import type { CodeMapSummary, CommandResultView } from './types';

export function codeMapFromCommandResult(
  result: CommandResultView,
  existing?: CodeMapSummary,
): CodeMapSummary | undefined {
  if (result.kind === 'codemap_status') {
    return {
      indexExists: boolField(result.index_exists),
      stale: boolField(result.stale),
      sourceFiles: numberField(result.source_files),
      walkedFiles: numberField(result.walked_files),
      symbolCount: numberField(result.symbol_count),
      todoCount: numberField(result.todo_count),
      generatedAtUnix: numberField(result.generated_at_unix),
      newestSourceMtimeUnix: numberField(result.newest_source_mtime_unix),
      reason: undefined,
    };
  }

  if (result.kind !== 'codemap' && result.kind !== 'todos') return undefined;
  const todoCount =
    numberField(result.todo_count) ??
    (result.kind === 'todos' && Array.isArray(result.items) ? result.items.length : undefined);
  return {
    ...existing,
    indexExists: true,
    stale: false,
    walkedFiles: numberField(result.walked_files) ?? existing?.walkedFiles,
    symbolCount:
      result.kind === 'codemap'
        ? numberField(result.symbol_count) ?? existing?.symbolCount
        : existing?.symbolCount,
    todoCount: todoCount ?? existing?.todoCount,
    generatedAtUnix: numberField(result.generated_at_unix) ?? existing?.generatedAtUnix,
    refreshed: boolField(result.refreshed),
    reason: undefined,
  };
}

export function markCodeMapStale(
  existing: CodeMapSummary | undefined,
  reason = 'workspace files changed',
): CodeMapSummary {
  return {
    ...existing,
    stale: true,
    reason,
  };
}

function numberField(value: unknown): number | undefined {
  return typeof value === 'number' && Number.isFinite(value) ? value : undefined;
}

function boolField(value: unknown): boolean | undefined {
  return typeof value === 'boolean' ? value : undefined;
}
