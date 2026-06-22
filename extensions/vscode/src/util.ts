// Pure, dependency-free helpers shared across the extension host modules.
// Kept free of `vscode` imports so it stays unit-testable under node --test.

/** Narrows an unknown value to a plain object (non-null, typeof 'object'). */
export function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null;
}

/**
 * Parses a strictly-positive integer from user input, returning `undefined` for
 * blank input and throwing `errorMessage` for anything that isn't a positive
 * whole number. Accepts the same forms as `Number()` (e.g. "10", "1.0", "01").
 */
export function parsePositiveInteger(value: string, errorMessage: string): number | undefined {
  const trimmed = value.trim();
  if (!trimmed) return undefined;
  const parsed = Number(trimmed);
  if (!Number.isInteger(parsed) || parsed <= 0) {
    throw new Error(errorMessage);
  }
  return parsed;
}
