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

export async function execPeridotCli(
  args: string[],
  cwd: string,
): Promise<{ stdout: string; stderr: string }> {
  const binary = await resolvePeridotBinary();
  return execFile(binary, args, cwd);
}

export function execFile(
  command: string,
  args: string[],
  cwd: string,
): Promise<{ stdout: string; stderr: string }> {
  return new Promise((resolve, reject) => {
    childProcess.execFile(
      command,
      args,
      {
        cwd,
        env: peridotChildEnv(),
        maxBuffer: 10 * 1024 * 1024,
      },
      (error, stdout, stderr) => {
        if (error) {
          const detail = stderr.trim() || stdout.trim() || error.message;
          reject(new Error(detail));
          return;
        }
        resolve({ stdout, stderr });
      },
    );
  });
}

export function parseJson(raw: string): unknown {
  try {
    return JSON.parse(raw);
  } catch {
    return { output: raw };
  }
}
