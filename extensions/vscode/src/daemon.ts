// JSON-RPC client for the `peridot daemon` subprocess.
//
// v0.1.0 bridge surface: spawn the daemon, send requests such as
// `peridot.version` / `session.start` / `session.cancel`, and dispatch
// server-pushed notifications (`method: "event"`) to listeners.

import * as childProcess from 'child_process';
import { isRecord } from './util';
import { resolvePeridotBinary } from './peridotBin';
import { peridotChildEnv } from './processEnv';

/**
 * AgentRunEvent wire-format version the extension was built against.
 *
 * The daemon sends its own version as the very first stdout line in a
 * `peridot.handshake` notification. If the values disagree the daemon shipped
 * with a different version of the Peridot binary than this extension was
 * built for — the user is surfaced a one-shot warning so they can update,
 * instead of mysterious silent malfunctions when an unknown event variant
 * arrives.
 *
 * MUST be kept in sync with `AGENT_RUN_EVENT_SCHEMA_VERSION` in
 * `crates/peridot-core/src/requests.rs`. Bumping rules are documented there.
 */
export const EXPECTED_AGENT_RUN_EVENT_SCHEMA_VERSION = 1;

/** One JSON-RPC 2.0 request envelope. */
interface RpcRequest {
  jsonrpc: '2.0';
  id: number;
  method: string;
  params?: unknown;
}

/** One JSON-RPC 2.0 response envelope (success or error). */
interface RpcResponse {
  jsonrpc: '2.0';
  id: number;
  result?: unknown;
  error?: { code: number; message: string };
}

/** One JSON-RPC 2.0 server notification envelope. */
export interface RpcNotification {
  jsonrpc: '2.0';
  method: string;
  params?: unknown;
}

/** Listener invoked for daemon notifications. */
type NotificationListener = (notification: RpcNotification) => void;

/** Daemon process exit payload. */
export interface DaemonExit {
  code: number | null;
  signal: string | null;
}

/** Listener invoked when the daemon process exits. */
type ExitListener = (exit: DaemonExit) => void;

/** Payload of the daemon's initial `peridot.handshake` notification. */
export interface DaemonHandshake {
  /** Wire-format version of `AgentRunEvent`. */
  schemaVersion: number;
  /** Crate version of the daemon binary. */
  daemonVersion: string;
}

/** Listener invoked once when the daemon completes its initial handshake. */
type HandshakeListener = (handshake: DaemonHandshake) => void;

/**
 * Spawned `peridot daemon` process plus the bookkeeping needed to
 * correlate stdout lines with outstanding requests and dispatch
 * server-pushed event notifications.
 *
 * Each `send` call:
 *   1. assigns a monotonically increasing id,
 *   2. parks a Promise resolver in `pending`,
 *   3. writes the line to the child's stdin,
 *   4. resolves the Promise when stdout produces a line whose id matches.
 */
/**
 * Default per-request timeout. The daemon answers control RPCs
 * (`peridot.status`, `session.start`, `session.command`, …) promptly — none of
 * them block for the lifetime of an agent run, which streams over
 * notifications instead — so a request still outstanding after this long means
 * the response was dropped or the daemon wedged. Rejecting reclaims the pending
 * slot and surfaces a recoverable error instead of hanging the UI forever.
 */
const DEFAULT_REQUEST_TIMEOUT_MS = 300_000;

/**
 * Hard cap on a single newline-delimited stdout line. A daemon emitting one
 * pathologically large line (runaway event, multi-MB blob) would otherwise be
 * buffered in full in the extension host before it could be parsed; past this
 * size the partial line is dropped and parsing resyncs at the next newline.
 */
const MAX_LINE_LENGTH = 16 * 1024 * 1024;

export class PeridotDaemon {
  private child: childProcess.ChildProcessWithoutNullStreams;
  private buffer = '';
  private skipOversizedLine = false;
  private nextId = 1;
  private pending = new Map<
    number,
    { resolve: (value: unknown) => void; reject: (err: Error) => void }
  >();
  private notificationListeners = new Set<NotificationListener>();
  private exitListeners = new Set<ExitListener>();
  private handshakeListeners = new Set<HandshakeListener>();
  private handshake: DaemonHandshake | undefined;
  private exited = false;

  private constructor(child: childProcess.ChildProcessWithoutNullStreams) {
    this.child = child;
    this.child.stdout.setEncoding('utf8');
    this.child.stdout.on('data', (chunk: string) => this.handleChunk(chunk));
    this.child.on('exit', (code, signal) => {
      this.exited = true;
      this.rejectAll(new Error('peridot daemon exited'));
      for (const listener of this.exitListeners) {
        try {
          listener({ code, signal });
        } catch (err) {
          console.error('[peridot] exit listener failed:', err);
        }
      }
    });
    this.child.on('error', (err) => this.rejectAll(err));
  }

  /**
   * Resolves to a daemon ready for `send()` calls. The binary path is
   * either the configured override, the bundled binary, or `peridot`
   * on PATH (in that priority order). See `peridotBin.ts`.
   */
  public static async spawn(projectRoot: string): Promise<PeridotDaemon> {
    const binary = await resolvePeridotBinary();
    const child = childProcess.spawn(binary, ['--project', projectRoot, 'daemon'], {
      env: peridotChildEnv(),
      stdio: ['pipe', 'pipe', 'pipe'],
    });
    // stderr is forwarded to extension host's console for debugging.
    child.stderr.on('data', (chunk: Buffer) => {
      console.warn('[peridot daemon stderr]', chunk.toString());
    });
    return new PeridotDaemon(child);
  }

  /**
   * Sends one JSON-RPC request and awaits its response. Rejects with
   * the daemon's error message when the response carries an `error`
   * envelope.
   */
  public send(
    method: string,
    params?: unknown,
    timeoutMs: number = DEFAULT_REQUEST_TIMEOUT_MS,
  ): Promise<unknown> {
    const id = this.nextId++;
    const request: RpcRequest = { jsonrpc: '2.0', id, method, params };
    const line = JSON.stringify(request) + '\n';
    return new Promise((resolve, reject) => {
      let timer: ReturnType<typeof setTimeout> | undefined;
      const clear = () => {
        if (timer) clearTimeout(timer);
      };
      this.pending.set(id, {
        resolve: (value) => {
          clear();
          resolve(value);
        },
        reject: (err) => {
          clear();
          reject(err);
        },
      });
      if (timeoutMs > 0 && Number.isFinite(timeoutMs)) {
        timer = setTimeout(() => {
          // Reclaim the slot so a late response is ignored rather than
          // resolving an already-rejected promise.
          if (this.pending.delete(id)) {
            reject(new Error(`peridot daemon did not respond to "${method}" within ${timeoutMs}ms`));
          }
        }, timeoutMs);
      }
      this.child.stdin.write(line, (err) => {
        if (err) {
          clear();
          this.pending.delete(id);
          reject(err);
        }
      });
    });
  }

  /**
   * Registers a listener for server-pushed JSON-RPC notifications. Returns a
   * function that removes the listener.
   */
  public onNotification(listener: NotificationListener): () => void {
    this.notificationListeners.add(listener);
    return () => {
      this.notificationListeners.delete(listener);
    };
  }

  /** Registers a listener for daemon process exit. */
  public onExit(listener: ExitListener): () => void {
    this.exitListeners.add(listener);
    return () => {
      this.exitListeners.delete(listener);
    };
  }

  /**
   * Registers a listener for the daemon's initial `peridot.handshake`
   * notification. Fires once per daemon process. If the handshake already
   * arrived before this listener was attached, fires synchronously with
   * the cached value so late subscribers don't miss it.
   */
  public onHandshake(listener: HandshakeListener): () => void {
    this.handshakeListeners.add(listener);
    if (this.handshake) {
      try {
        listener(this.handshake);
      } catch (err) {
        console.error('[peridot] handshake listener failed:', err);
      }
    }
    return () => {
      this.handshakeListeners.delete(listener);
    };
  }

  /**
   * Asks the daemon to drain and exit. Best-effort: we send the
   * notification, close stdin, and wait briefly for the child to
   * leave gracefully before forcing a kill.
   */
  public async shutdown(): Promise<void> {
    if (this.exited) {
      return;
    }
    try {
      const request: RpcRequest = { jsonrpc: '2.0', id: this.nextId++, method: 'shutdown' };
      this.child.stdin.write(JSON.stringify(request) + '\n');
    } catch {
      // Daemon may already be down; nothing useful to do here.
    }
    this.child.stdin.end();
    await new Promise<void>((resolve) => {
      let settled = false;
      const timers: ReturnType<typeof setTimeout>[] = [];
      const finish = () => {
        if (settled) return;
        settled = true;
        for (const timer of timers) clearTimeout(timer);
        resolve();
      };
      // Graceful first: give the daemon a moment to drain and exit on its own.
      // Escalate to SIGTERM, then SIGKILL, and only resolve once the child has
      // actually exited (or we've force-killed it) so we never leave an orphan
      // holding the project root / DB locks.
      this.child.once('exit', finish);
      timers.push(setTimeout(() => this.child.kill('SIGTERM'), 2000));
      timers.push(
        setTimeout(() => {
          this.child.kill('SIGKILL');
          finish();
        }, 5000),
      );
    });
  }

  /**
   * Accumulates stdout and dispatches complete newline-delimited lines. Caps
   * the in-flight buffer at {@link MAX_LINE_LENGTH}: an oversized partial line
   * is dropped and parsing resyncs at the next newline rather than buffering an
   * unbounded blob.
   */
  private handleChunk(chunk: string) {
    this.buffer += chunk;
    for (;;) {
      const idx = this.buffer.indexOf('\n');
      if (idx < 0) {
        if (this.buffer.length > MAX_LINE_LENGTH) {
          console.error('[peridot] daemon stdout line exceeded cap; dropping');
          this.buffer = '';
          this.skipOversizedLine = true;
        }
        return;
      }
      const line = this.buffer.slice(0, idx);
      this.buffer = this.buffer.slice(idx + 1);
      if (this.skipOversizedLine) {
        // This newline terminates the tail of a dropped oversized line.
        this.skipOversizedLine = false;
        continue;
      }
      this.handleLine(line);
    }
  }

  private handleLine(line: string) {
    if (!line.trim()) {
      return;
    }
    let parsed: unknown;
    try {
      parsed = JSON.parse(line);
    } catch {
      console.error('[peridot] daemon emitted unparseable line:', line);
      return;
    }
    if (isRpcNotification(parsed)) {
      // The handshake notification is the daemon's first stdout line — peel
      // it off here so listeners that care about generic `event` traffic
      // don't see a one-shot version envelope, and so the daemon's
      // schema/version becomes queryable via `onHandshake`.
      if (parsed.method === 'peridot.handshake') {
        const handshake = extractHandshake(parsed.params);
        if (handshake) {
          this.handshake = handshake;
          for (const listener of this.handshakeListeners) {
            try {
              listener(handshake);
            } catch (err) {
              console.error('[peridot] handshake listener failed:', err);
            }
          }
        }
        return;
      }
      for (const listener of this.notificationListeners) {
        try {
          listener(parsed);
        } catch (err) {
          console.error('[peridot] notification listener failed:', err);
        }
      }
      return;
    }
    if (!isRpcResponse(parsed)) {
      console.warn('[peridot] unexpected daemon message:', parsed);
      return;
    }
    const slot = this.pending.get(parsed.id);
    if (!slot) {
      console.warn('[peridot] response for unknown id', parsed.id);
      return;
    }
    this.pending.delete(parsed.id);
    if (parsed.error) {
      slot.reject(new Error(`daemon error ${parsed.error.code}: ${parsed.error.message}`));
    } else {
      slot.resolve(parsed.result);
    }
  }

  private rejectAll(err: Error) {
    for (const [, slot] of this.pending) {
      slot.reject(err);
    }
    this.pending.clear();
  }
}


function isRpcNotification(value: unknown): value is RpcNotification {
  return (
    isRecord(value) &&
    value.jsonrpc === '2.0' &&
    typeof value.method === 'string' &&
    !('id' in value)
  );
}

function extractHandshake(params: unknown): DaemonHandshake | undefined {
  if (!isRecord(params)) return undefined;
  const schemaVersion = params.schema_version;
  const daemonVersion = params.daemon_version;
  if (typeof schemaVersion !== 'number' || typeof daemonVersion !== 'string') {
    return undefined;
  }
  return { schemaVersion, daemonVersion };
}

function isRpcResponse(value: unknown): value is RpcResponse {
  if (
    !isRecord(value) ||
    value.jsonrpc !== '2.0' ||
    typeof value.id !== 'number'
  ) {
    return false;
  }
  if (value.error === undefined) {
    // A success envelope must carry a `result` member (which may be null);
    // an envelope with neither result nor error is malformed.
    return 'result' in value;
  }
  return (
    isRecord(value.error) &&
    typeof value.error.code === 'number' &&
    typeof value.error.message === 'string'
  );
}
