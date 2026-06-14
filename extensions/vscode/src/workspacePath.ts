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

/**
 * Returns true when `candidate` resolves to `root` or a descendant of it.
 *
 * Used to keep `openFile`/`openPath` — which act on paths that originate in
 * untrusted agent/tool output rendered in the webview — from escaping the
 * workspace (e.g. `../../../etc/passwd` or an absolute `~/.ssh/id_rsa`). The
 * comparison is on normalized paths with a separator boundary so that
 * `/foo-bar` is not treated as inside `/foo`.
 */
export function isPathWithinRoots(candidate: string, roots: Array<string | undefined>): boolean {
  const strip = (p: string): string => path.normalize(p).replace(/[\\/]+$/, '');
  const target = strip(candidate);
  for (const root of roots) {
    if (!root || !root.trim()) continue;
    const base = strip(root);
    if (target === base) return true;
    // Accept either separator: the host path module uses one, but remote
    // (WSL/SSH/container) fsPaths normalize to POSIX `/`.
    if (target.startsWith(base + path.sep) || target.startsWith(base + '/')) return true;
  }
  return false;
}

export function workspaceFindFilePatterns(input: string): string[] {
  const raw = input.trim().replace(/^file:\/\//, '');
  if (!raw || isAbsoluteWorkspacePath(raw)) return [];
  const variants = relativePathVariants(raw).filter(
    (variant) => !variant.startsWith('../') && !variant.includes('/../'),
  );
  const basename = path.posix.basename(raw.replace(/\\/g, '/'));
  return dedupe([
    ...variants.flatMap(ellipsisGlobVariants),
    ...variants.map((variant) => `**/${variant}`),
    basename ? `**/${basename}` : '',
  ].filter(Boolean));
}

export function workspaceFuzzyFindFilePatterns(input: string): string[] {
  const raw = input.trim().replace(/^file:\/\//, '');
  if (!raw || isAbsoluteWorkspacePath(raw)) return [];
  const variants = relativePathVariants(raw).filter(
    (variant) => !variant.startsWith('../') && !variant.includes('/../'),
  );
  const ext = path.posix.extname(raw.replace(/\\/g, '/'));
  if (!ext) return [];
  return dedupe(
    variants.flatMap((variant) => {
      const slash = variant.replace(/\\/g, '/');
      const ellipsisIndex = slash.split('/').findIndex((segment) => segment === '...');
      if (ellipsisIndex >= 0) {
        const prefix = slash.split('/').slice(0, ellipsisIndex).filter(Boolean).join('/');
        return prefix ? [`**/${prefix}/**/*${ext}`] : [`**/*${ext}`];
      }
      const parent = path.posix.dirname(slash);
      return parent && parent !== '.' ? [`**/${parent}/**/*${ext}`] : [`**/*${ext}`];
    }),
  );
}

export function bestWorkspaceFileMatch(input: string, candidates: string[]): string | undefined {
  let best: { value: string; score: number } | undefined;
  for (const candidate of candidates) {
    const score = workspaceFileMatchScore(input, candidate);
    if (score <= 0) continue;
    if (!best || score > best.score) best = { value: candidate, score };
  }
  return best && best.score >= 120 ? best.value : undefined;
}

function relativePathVariants(input: string): string[] {
  const slash = input.replace(/\\/g, '/').replace(/^\.\/+/, '');
  const normalized = path.posix.normalize(slash);
  const strippedParent = normalized.replace(/^(\.\.\/)+/, '');
  return dedupe([slash, normalized, strippedParent].filter((value) => value && value !== '.'));
}

function ellipsisGlobVariants(input: string): string[] {
  const slash = input.replace(/\\/g, '/');
  if (!slash.split('/').includes('...')) return [];
  return [`**/${slash.split('/').filter(Boolean).map((segment) => segment === '...' ? '**' : segment).join('/')}`];
}

function workspaceFileMatchScore(input: string, candidate: string): number {
  const requested = input.replace(/\\/g, '/');
  const requestedBase = path.posix.basename(requested);
  const candidateNormalized = candidate.replace(/\\/g, '/');
  const candidateBase = path.posix.basename(candidateNormalized);
  if (!requestedBase || !candidateBase) return 0;
  if (path.posix.extname(requestedBase).toLowerCase() !== path.posix.extname(candidateBase).toLowerCase()) {
    return 0;
  }

  const requestedStem = stripExtension(requestedBase).toLowerCase();
  const candidateStem = stripExtension(candidateBase).toLowerCase();
  let score = 0;
  if (requestedBase.toLowerCase() === candidateBase.toLowerCase()) score += 500;
  if (candidateStem.includes(requestedStem)) score += 250;

  const words = camelWords(stripExtension(requestedBase));
  if (words.length > 0 && words.every((word) => candidateStem.includes(word))) {
    score += 180 + words.length * 20;
  }

  const candidateLower = candidateNormalized.toLowerCase();
  for (const segment of requested.split('/')) {
    const clean = segment.trim().toLowerCase();
    if (!clean || clean === '...' || clean === requestedBase.toLowerCase()) continue;
    if (candidateLower.includes(`/${clean}/`) || candidateLower.includes(`/${clean}`)) score += 25;
  }
  return score;
}

function stripExtension(input: string): string {
  const ext = path.posix.extname(input);
  return ext ? input.slice(0, -ext.length) : input;
}

function camelWords(input: string): string[] {
  return input
    .replace(/([a-z0-9])([A-Z])/g, '$1 $2')
    .split(/[^A-Za-z0-9]+/)
    .map((word) => word.toLowerCase())
    .filter((word) => word.length >= 3);
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
