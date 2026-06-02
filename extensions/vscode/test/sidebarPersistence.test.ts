import test from 'node:test';
import assert from 'node:assert/strict';

import {
  INDEX_KEY,
  PERSISTENCE_KEY,
  PersistedTopLevel,
  PersistenceStore,
  SESSION_KEY_PREFIX,
  StoredChatSession,
  readPersistedSidebar,
  sessionPersistHash,
  writePersistedSidebar,
} from '../src/sidebarPersistence';

class FakeMemento implements PersistenceStore {
  readonly store = new Map<string, unknown>();
  readonly writes = new Map<string, number>();

  get<T>(key: string): T | undefined {
    return this.store.has(key) ? (this.store.get(key) as T) : undefined;
  }

  update(key: string, value: unknown): unknown {
    this.writes.set(key, (this.writes.get(key) ?? 0) + 1);
    if (value === undefined) this.store.delete(key);
    else this.store.set(key, value);
    return undefined;
  }

  keys(): readonly string[] {
    return [...this.store.keys()];
  }
}

function session(id: string, text: string): StoredChatSession {
  return {
    id,
    title: `Session ${id}`,
    status: 'Idle',
    running: false,
    transcript: [{ role: 'assistant', text } as StoredChatSession['transcript'][number]],
    hud: {} as StoredChatSession['hud'],
    runOptions: {} as StoredChatSession['runOptions'],
  };
}

function top(activeChatId?: string): PersistedTopLevel {
  return {
    activeChatId,
    nextSessionOrdinal: 3,
    runOptions: {} as PersistedTopLevel['runOptions'],
    context: {} as PersistedTopLevel['context'],
    view: 'session',
    landing: 'home',
    queue: [],
  };
}

test('v2 round-trips sessions and top-level state', () => {
  const memento = new FakeMemento();
  const hashes = new Map<string, string>();
  const sessions = [session('a', 'hello'), session('b', 'world')];

  writePersistedSidebar(memento, top('a'), sessions, hashes, false);
  const restored = readPersistedSidebar(memento);

  assert.ok(restored);
  assert.equal(restored.fromLegacy, false);
  assert.equal(restored.top.activeChatId, 'a');
  assert.equal(restored.sessions.length, 2);
  assert.deepEqual(
    restored.sessions.map((s) => s.id),
    ['a', 'b'],
  );
  assert.equal(restored.sessions[1].transcript[0].text, 'world');
});

test('only changed sessions are re-written on a throttled save', () => {
  const memento = new FakeMemento();
  const hashes = new Map<string, string>();
  const a = session('a', 'hello');
  const b = session('b', 'world');

  writePersistedSidebar(memento, top('a'), [a, b], hashes, false);
  const writesAfterFirst = memento.writes.get(`${SESSION_KEY_PREFIX}b`);

  // Change only session a; b is byte-for-byte identical.
  const a2 = session('a', 'hello there');
  writePersistedSidebar(memento, top('a'), [a2, b], hashes, false);

  assert.equal(
    memento.writes.get(`${SESSION_KEY_PREFIX}b`),
    writesAfterFirst,
    'unchanged session must not be re-serialized',
  );
  assert.ok(
    (memento.writes.get(`${SESSION_KEY_PREFIX}a`) ?? 0) >= 2,
    'changed session must be re-written',
  );
});

test('force rewrites every session', () => {
  const memento = new FakeMemento();
  const hashes = new Map<string, string>();
  const a = session('a', 'hello');
  const b = session('b', 'world');
  writePersistedSidebar(memento, top('a'), [a, b], hashes, false);
  const before = memento.writes.get(`${SESSION_KEY_PREFIX}b`) ?? 0;
  writePersistedSidebar(memento, top('a'), [a, b], hashes, true);
  assert.equal(memento.writes.get(`${SESSION_KEY_PREFIX}b`), before + 1);
});

test('removed sessions are pruned from storage', () => {
  const memento = new FakeMemento();
  const hashes = new Map<string, string>();
  writePersistedSidebar(memento, top('a'), [session('a', 'x'), session('b', 'y')], hashes, false);
  assert.ok(memento.store.has(`${SESSION_KEY_PREFIX}b`));

  writePersistedSidebar(memento, top('a'), [session('a', 'x')], hashes, false);
  assert.ok(!memento.store.has(`${SESSION_KEY_PREFIX}b`), 'pruned key should be gone');
  assert.ok(!hashes.has('b'), 'pruned hash should be dropped');
});

test('migrates a legacy v1 blob and clears it on write', () => {
  const memento = new FakeMemento();
  memento.store.set(PERSISTENCE_KEY, {
    version: 1,
    activeChatId: 'a',
    nextSessionOrdinal: 2,
    runOptions: {},
    context: {},
    view: 'session',
    landing: 'home',
    queue: [],
    sessions: [session('a', 'legacy')],
  });

  const restored = readPersistedSidebar(memento);
  assert.ok(restored);
  assert.equal(restored.fromLegacy, true);
  assert.equal(restored.sessions[0].transcript[0].text, 'legacy');

  // Migration write: unseeded hashes → full write to v2, legacy key cleared.
  const hashes = new Map<string, string>();
  writePersistedSidebar(memento, restored.top, restored.sessions, hashes, true);
  assert.equal(memento.store.get(PERSISTENCE_KEY), undefined, 'legacy blob cleared');
  assert.ok(memento.store.has(INDEX_KEY));
  assert.ok(memento.store.has(`${SESSION_KEY_PREFIX}a`));

  // And it now reads back via the v2 path.
  const again = readPersistedSidebar(memento);
  assert.equal(again?.fromLegacy, false);
});

test('sessionPersistHash changes when the transcript grows', () => {
  const before = sessionPersistHash(session('a', 'hi'));
  const after = sessionPersistHash(session('a', 'hi there'));
  assert.notEqual(before, after);
});

test('readPersistedSidebar returns undefined when nothing is stored', () => {
  assert.equal(readPersistedSidebar(new FakeMemento()), undefined);
});
