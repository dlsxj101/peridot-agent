// Sidebar persistence (v2). Split out of sidebar.ts and kept free of any
// `vscode` import so the pure (de)serialization logic can be unit-tested with
// an in-memory store.
//
// v1 stored every session's full transcript under a single Memento key,
// re-serialized on every throttled save — O(total history) per write, growing
// unbounded and risking Memento size limits. v2 keeps a small index (top-level
// state + ordered session ids) under one key and each session's heavy data
// under its own key, and the writer only re-serializes sessions whose content
// hash changed.

import {
  HudState,
  NoteSummary,
  QueuedMessage,
  RunOptions,
  SidebarContext,
  SidebarState,
  TranscriptItem,
} from './types';

/** A persisted chat session (one conversation tab). */
export interface StoredChatSession {
  id: string;
  title: string;
  daemonSessionId?: string;
  status: string;
  running: boolean;
  transcript: TranscriptItem[];
  hud: HudState;
  runOptions: RunOptions;
  pendingApproval?: TranscriptItem;
  runStartedAtMs?: number;
  lastRunElapsedMs?: number;
  totalTokens?: number;
  totalCostUsd?: number;
  turnsUsed?: number;
  attachmentPaths?: string[];
  noteSummary?: NoteSummary;
  /**
   * True once the user has manually renamed this session. The async LLM
   * title-generation path must not overwrite a user-chosen title.
   */
  userRenamed?: boolean;
}

/** Legacy single-blob snapshot (read only, for migration). */
export interface PersistedSidebarSnapshot {
  version: 1;
  activeChatId?: string;
  nextSessionOrdinal: number;
  runOptions: RunOptions;
  context: SidebarContext;
  view: SidebarState['view'];
  landing: SidebarState['landing'];
  queue: QueuedMessage[];
  sessions: StoredChatSession[];
}

/** The v2 light index: top-level state plus the ordered session ids. */
interface PersistedIndexV2 {
  version: 2;
  activeChatId?: string;
  nextSessionOrdinal: number;
  runOptions: RunOptions;
  context: SidebarContext;
  view: SidebarState['view'];
  landing: SidebarState['landing'];
  queue: QueuedMessage[];
  sessionIds: string[];
}

/** Top-level fields persisted alongside the session list. */
export interface PersistedTopLevel {
  activeChatId?: string;
  nextSessionOrdinal: number;
  runOptions: RunOptions;
  context: SidebarContext;
  view: SidebarState['view'];
  landing: SidebarState['landing'];
  queue: QueuedMessage[];
}

/** Minimal Memento surface the helpers need; `vscode.Memento` satisfies it
 *  structurally, and tests provide a plain in-memory implementation. */
export interface PersistenceStore {
  get<T>(key: string): T | undefined;
  update(key: string, value: unknown): unknown;
  keys(): readonly string[];
}

export const PERSISTENCE_KEY = 'peridot.sidebarState.v1';
export const INDEX_KEY = 'peridot.sidebarIndex.v2';
export const SESSION_KEY_PREFIX = 'peridot.sidebarSession.v2.';

function sessionStorageKey(id: string): string {
  return `${SESSION_KEY_PREFIX}${id}`;
}

/** A cheap content fingerprint used to skip re-writing unchanged sessions.
 *  Captures the fields that move during a run (transcript growth, status) plus
 *  the metadata a save must not silently drop. */
export function sessionPersistHash(session: StoredChatSession): string {
  const last = session.transcript[session.transcript.length - 1];
  return [
    session.transcript.length,
    last?.text?.length ?? 0,
    last?.role ?? '',
    session.title,
    session.status,
    session.running ? 1 : 0,
    session.userRenamed ? 1 : 0,
    session.daemonSessionId ?? '',
    session.attachmentPaths?.length ?? 0,
    session.pendingApproval ? 1 : 0,
  ].join('|');
}

function indexTopLevel(source: PersistedIndexV2 | PersistedSidebarSnapshot): PersistedTopLevel {
  return {
    activeChatId: source.activeChatId,
    nextSessionOrdinal: source.nextSessionOrdinal,
    runOptions: source.runOptions,
    context: source.context,
    view: source.view,
    landing: source.landing,
    queue: source.queue,
  };
}

/** Writes the index plus any sessions whose hash changed (or all when
 *  `force`), prunes per-session keys no longer present, and clears the legacy
 *  v1 key. Mutates `hashes` to reflect what was written. */
export function writePersistedSidebar(
  storage: PersistenceStore,
  top: PersistedTopLevel,
  sessions: StoredChatSession[],
  hashes: Map<string, string>,
  force: boolean,
): void {
  const index: PersistedIndexV2 = {
    version: 2,
    activeChatId: top.activeChatId,
    nextSessionOrdinal: top.nextSessionOrdinal,
    runOptions: top.runOptions,
    context: top.context,
    view: top.view,
    landing: top.landing,
    queue: top.queue,
    sessionIds: sessions.map((session) => session.id),
  };
  void storage.update(INDEX_KEY, index);

  const liveKeys = new Set<string>();
  for (const session of sessions) {
    liveKeys.add(sessionStorageKey(session.id));
    const hash = sessionPersistHash(session);
    if (force || hashes.get(session.id) !== hash) {
      void storage.update(sessionStorageKey(session.id), session);
      hashes.set(session.id, hash);
    }
  }

  // Prune per-session keys for sessions that no longer exist.
  for (const key of storage.keys()) {
    if (key.startsWith(SESSION_KEY_PREFIX) && !liveKeys.has(key)) {
      void storage.update(key, undefined);
    }
  }
  for (const id of [...hashes.keys()]) {
    if (!sessions.some((session) => session.id === id)) hashes.delete(id);
  }

  // Drop the legacy single-blob snapshot once migrated.
  if (storage.get(PERSISTENCE_KEY) !== undefined) {
    void storage.update(PERSISTENCE_KEY, undefined);
  }
}

/** Reads persisted sidebar state, preferring v2 and falling back to the legacy
 *  v1 blob (`fromLegacy: true`, migrated on the next write). Returns
 *  `undefined` when no persisted state exists. */
export function readPersistedSidebar(
  storage: PersistenceStore,
): { top: PersistedTopLevel; sessions: StoredChatSession[]; fromLegacy: boolean } | undefined {
  const index = storage.get<PersistedIndexV2>(INDEX_KEY);
  if (index && index.version === 2 && Array.isArray(index.sessionIds)) {
    const sessions: StoredChatSession[] = [];
    for (const id of index.sessionIds) {
      const raw = storage.get<StoredChatSession>(sessionStorageKey(id));
      if (raw) sessions.push(raw);
    }
    return { top: indexTopLevel(index), sessions, fromLegacy: false };
  }
  const legacy = storage.get<PersistedSidebarSnapshot>(PERSISTENCE_KEY);
  if (legacy && legacy.version === 1 && Array.isArray(legacy.sessions)) {
    return { top: indexTopLevel(legacy), sessions: legacy.sessions, fromLegacy: true };
  }
  return undefined;
}
