// Locates the `peridot` binary the extension should spawn.
//
// Resolution order (first match wins):
//   1. `peridot.binaryPath` configuration override — for developers
//      iterating on a local build. Stale paths are ignored so a Windows
//      extension host does not try to spawn a WSL-only `/home/...` path.
//   2. Bundled binary inside the .vsix at
//      `<extension>/resources/peridot[.exe]`. This is what end users
//      get because we publish platform-specific .vsix targets via
//      `vsce publish --target <X>`.
//   3. Plain `peridot` on the system PATH — gives a graceful fallback
//      when the bundled binary is missing (development install, broken
//      platform target, etc.).
//
// The function verifies absolute file candidates exist on the current
// extension host before returning them. The daemon spawn itself still
// surfaces runtime errors if the file exists but cannot execute.

import * as fs from 'fs';
import * as path from 'path';
import * as vscode from 'vscode';

let cached: string | undefined;

export async function resolvePeridotBinary(): Promise<string> {
  if (cached) {
    return cached;
  }

  // 1. Configuration override.
  const override = vscode.workspace.getConfiguration('peridot').get<string>('binaryPath');
  const overridePath = override?.trim();
  if (overridePath && isUsableBinaryPath(overridePath)) {
    cached = overridePath;
    return cached;
  }

  // 2. Bundled binary. The extension host exposes its install
  // directory via `vscode.extensions.getExtension(<id>)?.extensionPath`.
  const ext = vscode.extensions.getExtension('dlsxj101.peridot-vscode');
  if (ext) {
    const exeName = process.platform === 'win32' ? 'peridot.exe' : 'peridot';
    const bundled = path.join(ext.extensionPath, 'resources', exeName);
    if (isUsableBinaryPath(bundled)) {
      cached = bundled;
      return cached;
    }
  }

  // 3. PATH fallback. Spawn-time PATH lookup handles the rest.
  cached = 'peridot';
  return cached;
}

/** Clears the memoised lookup so a follow-up call re-checks the disk. */
export function resetBinaryCache(): void {
  cached = undefined;
}

function isUsableBinaryPath(candidate: string): boolean {
  try {
    const stat = fs.statSync(candidate);
    return stat.isFile();
  } catch {
    return false;
  }
}
