// Small TTL cache for `peridot.status` results.
//
// `refreshStatus` was spawning a fresh daemon per call — once per workspace
// change, once on task finish, plus every manual refresh. That gave the
// sidebar a 200-500ms latency hit and littered the Output channel with
// spawn lines. Cache results for a few seconds; allow callers to force a
// re-read when state genuinely changed (login completed, task finished).

export interface CachedStatus<T> {
  value: T;
  fetchedAt: number;
}

export class StatusCache<T> {
  private cached: CachedStatus<T> | undefined;
  private inflight: Promise<T> | undefined;

  public constructor(
    private readonly fetcher: () => Promise<T>,
    private readonly ttlMs: number = 5000,
  ) {}

  public async get(force = false): Promise<T> {
    const now = Date.now();
    if (!force && this.cached && now - this.cached.fetchedAt < this.ttlMs) {
      return this.cached.value;
    }
    if (this.inflight) {
      return this.inflight;
    }
    this.inflight = (async () => {
      try {
        const value = await this.fetcher();
        this.cached = { value, fetchedAt: Date.now() };
        return value;
      } finally {
        this.inflight = undefined;
      }
    })();
    return this.inflight;
  }

  public invalidate(): void {
    this.cached = undefined;
  }
}
