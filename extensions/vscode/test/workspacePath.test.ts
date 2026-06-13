import test from 'node:test';
import assert from 'node:assert/strict';

import {
  bestWorkspaceFileMatch,
  isPathWithinRoots,
  workspaceFileCandidatePaths,
  workspaceFindFilePatterns,
  workspaceFuzzyFindFilePatterns,
} from '../src/workspacePath';

test('isPathWithinRoots accepts the root and its descendants', () => {
  const root = '/home/user/project';
  assert.equal(isPathWithinRoots(root, [root]), true);
  assert.equal(isPathWithinRoots('/home/user/project/src/main.rs', [root]), true);
  assert.equal(isPathWithinRoots('/home/user/project/.peridot/exports/s1', [root]), true);
});

test('isPathWithinRoots rejects traversal, siblings, and absolute escapes', () => {
  const root = '/home/user/project';
  assert.equal(isPathWithinRoots('/home/user/project/../../../etc/passwd', [root]), false);
  assert.equal(isPathWithinRoots('/etc/passwd', [root]), false);
  assert.equal(isPathWithinRoots('/home/user/.ssh/id_rsa', [root]), false);
  // Sibling sharing a prefix must not count as inside.
  assert.equal(isPathWithinRoots('/home/user/project-evil/x', [root]), false);
  // Undefined / empty roots are ignored.
  assert.equal(isPathWithinRoots('/anything', [undefined, '']), false);
});

test('workspaceFileCandidatePaths keeps simple root-relative resolution', () => {
  assert.deepEqual(
    workspaceFileCandidatePaths('src/main/java/App.java', ['/Users/hyunjung/Megatus/megaapim']),
    ['/Users/hyunjung/Megatus/megaapim/src/main/java/App.java'],
  );
});

test('workspaceFileCandidatePaths handles workspace-name-prefixed paths', () => {
  const candidates = workspaceFileCandidatePaths(
    'megaapim/megaapim-repository-mongodb/src/main/java/com/megatus/megaapim/repository/mongodb/management/MongoApiKeyRepository.java',
    ['/Users/hyunjung/Megatus/megaapim/megaapim'],
  );

  assert.ok(
    candidates.includes(
      '/Users/hyunjung/Megatus/megaapim/megaapim/megaapim-repository-mongodb/src/main/java/com/megatus/megaapim/repository/mongodb/management/MongoApiKeyRepository.java',
    ),
  );
});

test('workspaceFindFilePatterns falls back to basename search for normalized relative paths', () => {
  assert.deepEqual(
    workspaceFindFilePatterns('megaapim/megaapim-repository-mongodb/../MongoApiKeyRepository.java'),
    [
      '**/megaapim/MongoApiKeyRepository.java',
      '**/MongoApiKeyRepository.java',
    ],
  );
});

test('workspaceFindFilePatterns expands abbreviated ellipsis paths', () => {
  assert.deepEqual(
    workspaceFindFilePatterns('megaapim/megaapim-repository-mongodb/.../MongoApiKeyRepository.java').slice(0, 1),
    ['**/megaapim/megaapim-repository-mongodb/**/MongoApiKeyRepository.java'],
  );
});

test('workspaceFuzzyFindFilePatterns narrows ellipsis paths by prefix and extension', () => {
  assert.deepEqual(
    workspaceFuzzyFindFilePatterns('megaapim/megaapim-repository-mongodb/.../ApiKeyMongo.java'),
    ['**/megaapim/megaapim-repository-mongodb/**/*.java'],
  );
});

test('bestWorkspaceFileMatch handles reordered camel-case filename hints', () => {
  const input = 'megaapim/megaapim-repository-mongodb/.../ApiKeyMongo.java';
  assert.equal(
    bestWorkspaceFileMatch(input, [
      '/Users/hyunjung/Megatus/megaapim/megaapim/megaapim-repository-mongodb/src/main/java/com/megatus/megaapim/repository/mongodb/management/MongoApiKeyRepository.java',
      '/Users/hyunjung/Megatus/megaapim/megaapim/megaapim-repository-mongodb/src/main/java/com/megatus/megaapim/repository/mongodb/management/OtherRepository.java',
    ]),
    '/Users/hyunjung/Megatus/megaapim/megaapim/megaapim-repository-mongodb/src/main/java/com/megatus/megaapim/repository/mongodb/management/MongoApiKeyRepository.java',
  );
});
