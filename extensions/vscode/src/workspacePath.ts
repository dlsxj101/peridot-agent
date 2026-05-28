import * as path from 'path';

export function isAbsoluteWorkspacePath(input: string): boolean {
  return input.startsWith('/') || /^[A-Za-z]:[\\/]/.test(input);
}

export function workspaceFileCandidatePaths(input: string, roots: Array<string | undefined>): string[] {
  const raw = input.trim().replace(/^file:\/\//, '');
  if (!raw) return [];
  if (isAbsoluteWorkspacePath(raw)) return dedupe([path.normalize(raw)]);

  const cleanRoots = dedupe(
    roots
      .map((root) => root?.trim())
      .filter((root): root is string => Boolean(root)),
  );
  const relatives = relativePathVariants(raw);
  const candidates: string[] = [];

  for (const root of cleanRoots) {
    for (const relative of relatives) {
      candidates.push(path.normalize(path.join(root, relative)));

      const first = firstPathSegment(relative);
      if (first && first === path.basename(root)) {
        candidates.push(path.normalize(path.join(path.dirname(root), relative)));
        candidates.push(path.normalize(path.join(root, stripFirstPathSegment(relative))));
      }
    }
  }

  return dedupe(candidates);
}

export function workspaceFindFilePatterns(input: string): string[] {
  const raw = input.trim().replace(/^file:\/\//, '');
  if (!raw || isAbsoluteWorkspacePath(raw)) return [];
  const variants = relativePathVariants(raw).filter(
    (variant) => !variant.startsWith('../') && !variant.includes('/../'),
  );
  const basename = path.posix.basename(raw.replace(/\\/g, '/'));
  return dedupe([
    ...variants.map((variant) => `**/${variant}`),
    basename ? `**/${basename}` : '',
  ].filter(Boolean));
}

function relativePathVariants(input: string): string[] {
  const slash = input.replace(/\\/g, '/').replace(/^\.\/+/, '');
  const normalized = path.posix.normalize(slash);
  const strippedParent = normalized.replace(/^(\.\.\/)+/, '');
  return dedupe([slash, normalized, strippedParent].filter((value) => value && value !== '.'));
}

function firstPathSegment(input: string): string | undefined {
  return input.replace(/\\/g, '/').split('/').find((segment) => segment.length > 0);
}

function stripFirstPathSegment(input: string): string {
  return input.replace(/\\/g, '/').split('/').filter(Boolean).slice(1).join('/');
}

function dedupe(values: string[]): string[] {
  const seen = new Set<string>();
  const result: string[] = [];
  for (const value of values) {
    if (seen.has(value)) continue;
    seen.add(value);
    result.push(value);
  }
  return result;
}
