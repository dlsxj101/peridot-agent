// Drops the workspace cargo binary into `resources/peridot[.exe]` so a
// locally-packaged `.vsix` can verify the bundled-binary path. The CI
// release pipeline does the same swap with platform-specific binaries
// downloaded from the build matrix — see `.github/workflows/vscode-release.yml`.

import { copyFileSync, existsSync, mkdirSync, statSync, chmodSync } from 'node:fs';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import process from 'node:process';

const extensionDir = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const workspaceRoot = resolve(extensionDir, '..', '..');
const profile = process.argv.includes('--debug') ? 'debug' : 'release';
const isWindows = process.platform === 'win32';
const exe = isWindows ? '.exe' : '';

const source = resolve(workspaceRoot, 'target', profile, `peridot${exe}`);
const destDir = resolve(extensionDir, 'resources');
const dest = resolve(destDir, `peridot${exe}`);

if (!existsSync(source)) {
  console.error(`[bundle-binary] missing ${source}`);
  console.error(`[bundle-binary] build it first: cargo build ${profile === 'release' ? '--release ' : ''}-p peridot-cli`);
  process.exit(1);
}

mkdirSync(destDir, { recursive: true });
copyFileSync(source, dest);
if (!isWindows) {
  // copyFileSync preserves mode on most filesystems but be explicit so
  // the workflow / local copy always lands as 0o755.
  chmodSync(dest, 0o755);
}

const { size } = statSync(dest);
console.log(`[bundle-binary] copied ${profile} build → ${dest} (${(size / 1024 / 1024).toFixed(1)} MB)`);
