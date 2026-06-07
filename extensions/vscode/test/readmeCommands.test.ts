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
  // Command titles are localized via package.nls (`%key%` placeholders); resolve
  // each to its English default before checking the README documents it. A
  // placeholder whose key is missing from package.nls.json stays unresolved and
  // fails the README check, so this also guards against dangling NLS keys.
  const nls = JSON.parse(fs.readFileSync('package.nls.json', 'utf8')) as Record<string, string>;
  const resolveTitle = (title: string): string =>
    title.startsWith('%') && title.endsWith('%') ? (nls[title.slice(1, -1)] ?? title) : title;
  const titles = (pkg.contributes?.commands ?? [])
    .map((command) => command.title)
    .filter((title): title is string => typeof title === 'string')
    .map(resolveTitle);

  const missing = titles.filter((title) => !readme.includes(`| \`${title}\` |`));

  assert.deepEqual(missing, []);
});
