import { spawnSync } from 'node:child_process';
import { readdirSync, statSync } from 'node:fs';
import { join } from 'node:path';

const outDir = 'out-test';

function testFiles(root) {
  const files = [];
  for (const entry of readdirSync(root)) {
    const path = join(root, entry);
    const stat = statSync(path);
    if (stat.isDirectory()) {
      files.push(...testFiles(path));
    } else if (entry.endsWith('.test.js')) {
      files.push(path);
    }
  }
  return files;
}

const files = testFiles(outDir).sort();
if (files.length === 0) {
  console.error(`No compiled unit tests found under ${outDir}`);
  process.exit(1);
}

const result = spawnSync(process.execPath, ['--test', ...files], {
  stdio: 'inherit',
});
process.exit(result.status ?? 1);
