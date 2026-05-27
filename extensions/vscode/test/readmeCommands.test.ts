import test from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';

interface PackageJson {
  contributes?: {
    commands?: Array<{
      command?: string;
      title?: string;
    }>;
  };
}

test('README command table documents every contributed command', () => {
  const pkg = JSON.parse(fs.readFileSync('package.json', 'utf8')) as PackageJson;
  const readme = fs.readFileSync('README.md', 'utf8');
  const titles = (pkg.contributes?.commands ?? [])
    .map((command) => command.title)
    .filter((title): title is string => typeof title === 'string');

  const missing = titles.filter((title) => !readme.includes(`| \`${title}\` |`));

  assert.deepEqual(missing, []);
});
