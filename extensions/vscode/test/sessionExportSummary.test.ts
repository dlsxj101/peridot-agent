import test from 'node:test';
import assert from 'node:assert/strict';

import { sessionExportSummary } from '../webview/sessionExportSummary';

test('sessionExportSummary reports generated artifacts and full-copy entries', () => {
  const summary = sessionExportSummary({
    artifacts: [
      { class: 'attachments', path: 'attachments.json', count: 1 },
      { class: 'timeline', path: 'timeline.json', count: 3 },
    ],
    files: ['tui_state.json', 'transcript.ndjson'],
  });

  assert.deepEqual(summary.chips, ['2 generated files', '2 full-copy entries']);
  assert.deepEqual(
    summary.generatedArtifacts.map((artifact) => artifact.path),
    ['attachments.json', 'timeline.json'],
  );
  assert.deepEqual(summary.fullCopyFiles, ['tui_state.json', 'transcript.ndjson']);
});

test('sessionExportSummary keeps empty export cards explicit', () => {
  assert.deepEqual(sessionExportSummary(undefined), {
    generatedArtifacts: [],
    fullCopyFiles: [],
    chips: ['0 files'],
  });
});
