// Shared CLI / filesystem helpers for command handlers.
//
// Thin wrappers around child_process and the workspace filesystem used by the
// ship/merge and session-transfer command handlers: resolve + run the peridot
// binary, run an arbitrary command, parse JSON output defensively, and a couple
// of small string/path guards. Split out of `extension.ts` so multiple command
// modules can share them without re-importing the host entry point.

import * as childProcess from 'child_process';
import * as vscode from 'vscode';

import { resolvePeridotBinary } from '../peridotBin';
import { peridotChildEnv } from '../processEnv';
import type { PeridotSidebarProvider } from '../sidebar';

// Shared "no workspace folder" guard used by the command handlers. Returns the
// first workspace folder path, or surfaces the given message (sidebar problem +
// warning notification) and returns undefined — matching the pattern every
// command module previously inlined.
export async function ensureWorkspaceFolder(
  sidebar: PeridotSidebarProvider,
  message: string,
): Promise<string | undefined> {
  const folder = vscode.workspace.workspaceFolders?.[0]?.uri.fsPath;
  if (!folder) {
    sidebar.setWorkspaceProblem(message);
    await vscode.window.showWarningMessage(message);
  }
  return folder;
}

export function nonEmpty(value: string): string | undefined {
  const trimmed = value.trim();
  return trimmed.length > 0 ? trimmed : undefined;
}

export function sanitizePathSegment(value: string): string {
  const sanitized = value.replace(/[^A-Za-z0-9._-]+/g, '-').replace(/^-+|-+$/g, '');
  return sanitized.length > 0 ? sanitized : 'session';
}

export async function pathExists(filePath: string): Promise<boolean> {
  try {
    await vscode.workspace.fs.stat(vscode.Uri.file(filePath));
    return true;
  } catch {
    return false;
  }
}

export interface ExecOptions {
  // Hard ceiling so a wedged `gh` / `peridot` subprocess can't hang the UI
  // forever. The child is killed with SIGTERM once it elapses.
  timeoutMs?: number;
}

// Ship/push/PR work hits the network, so the default is generous; the goal is
// only to bound a truly stuck process, not to race legitimate slow operations.
const DEFAULT_EXEC_TIMEOUT_MS = 120_000;

export async function execPeridotCli(
  args: string[],
  cwd: string,
  options?: ExecOptions,
): Promise<{ stdout: string; stderr: string }> {
  const binary = await resolvePeridotBinary();
  return execFile(binary, args, cwd, options);
}

export function execFile(
  command: string,
  args: string[],
  cwd: string,
  options?: ExecOptions,
): Promise<{ stdout: string; stderr: string }> {
  const timeoutMs = options?.timeoutMs ?? DEFAULT_EXEC_TIMEOUT_MS;
  return new Promise((resolve, reject) => {
    childProcess.execFile(
      command,
      args,
      {
        cwd,
        env: peridotChildEnv(),
        maxBuffer: 10 * 1024 * 1024,
        timeout: timeoutMs,
        killSignal: 'SIGTERM',
      },
      (error, stdout, stderr) => {
        if (error) {
          // Node sets `killed` when it terminates the child for exceeding the
          // timeout; surface that distinctly so the user knows it hung rather
          // than failed on its own.
          if ((error as { killed?: boolean }).killed) {
            reject(new Error(`\`${command}\` timed out after ${Math.round(timeoutMs / 1000)}s`));
            return;
          }
          const detail = stderr.trim() || stdout.trim() || error.message;
          reject(new Error(detail));
          return;
        }
        resolve({ stdout, stderr });
      },
    );
  });
}

// Parses CLI `--output json` results. Throws on non-JSON output so a command
// that printed an error string (but exited 0) surfaces the error instead of
// being silently treated as an empty success. Every caller runs inside a
// try/catch that reports the message to the user.
export function parseJson(raw: string): unknown {
  try {
    return JSON.parse(raw);
  } catch {
    const preview = raw.trim().slice(0, 200);
    throw new Error(`Expected JSON output but got: ${preview || '(empty output)'}`);
  }
}
