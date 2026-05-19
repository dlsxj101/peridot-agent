// JSON-RPC client for the `peridot daemon` subprocess.
//
// v0.0.1 surface: spawn the daemon, send `peridot.version` /
// `peridot.echo` / `shutdown`, await one response per request.
//
// v0.1.0 will add session.start + event notification dispatch
// (incoming messages without `id` are dispatched to a registered
// listener instead of a pending request).

import * as childProcess from 'child_process';
import * as readline from 'readline';
import { resolvePeridotBinary } from './peridotBin';

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

/**
 * Spawned `peridot daemon` process plus the bookkeeping needed to
 * correlate stdout lines with outstanding requests.
 *
 * Each `send` call:
 *   1. assigns a monotonically increasing id,
 *   2. parks a Promise resolver in `pending`,
 *   3. writes the line to the child's stdin,
 *   4. resolves the Promise when stdout produces a line whose id matches.
 */
export class PeridotDaemon {
  private child: childProcess.ChildProcessWithoutNullStreams;
  private rl: readline.Interface;
  private nextId = 1;
  private pending = new Map<
    number,
    { resolve: (value: unknown) => void; reject: (err: Error) => void }
  >();

  private constructor(child: childProcess.ChildProcessWithoutNullStreams) {
    this.child = child;
    this.rl = readline.createInterface({ input: child.stdout });
    this.rl.on('line', (line) => this.handleLine(line));
    this.child.on('exit', () => this.rejectAll(new Error('peridot daemon exited')));
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
  public send(method: string, params?: unknown): Promise<unknown> {
    const id = this.nextId++;
    const request: RpcRequest = { jsonrpc: '2.0', id, method, params };
    const line = JSON.stringify(request) + '\n';
    return new Promise((resolve, reject) => {
      this.pending.set(id, { resolve, reject });
      this.child.stdin.write(line, (err) => {
        if (err) {
          this.pending.delete(id);
          reject(err);
        }
      });
    });
  }

  /**
   * Asks the daemon to drain and exit. Best-effort: we send the
   * notification, close stdin, and wait briefly for the child to
   * leave gracefully before forcing a kill.
   */
  public async shutdown(): Promise<void> {
    try {
      const request: RpcRequest = { jsonrpc: '2.0', id: this.nextId++, method: 'shutdown' };
      this.child.stdin.write(JSON.stringify(request) + '\n');
    } catch {
      // Daemon may already be down; nothing useful to do here.
    }
    this.child.stdin.end();
    await new Promise<void>((resolve) => {
      const timer = setTimeout(() => {
        this.child.kill('SIGTERM');
        resolve();
      }, 2000);
      this.child.once('exit', () => {
        clearTimeout(timer);
        resolve();
      });
    });
  }

  private handleLine(line: string) {
    if (!line.trim()) {
      return;
    }
    let parsed: RpcResponse;
    try {
      parsed = JSON.parse(line);
    } catch (err) {
      console.error('[peridot] daemon emitted unparseable line:', line);
      return;
    }
    if (typeof parsed.id !== 'number') {
      // Notification surface lands here once v0.1.0 ships. v0.0.1
      // daemon never emits these, so log + drop.
      console.warn('[peridot] unexpected notification:', parsed);
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
