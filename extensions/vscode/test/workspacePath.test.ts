import test from 'node:test';
import assert from 'node:assert/strict';

import { workspaceFileCandidatePaths, workspaceFindFilePatterns } from '../src/workspacePath';

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
