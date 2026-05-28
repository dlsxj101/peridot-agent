import test from 'node:test';
import assert from 'node:assert/strict';

import { attachmentContextPill } from '../webview/attachmentContext';

test('attachmentContextPill summarizes session attachment paths', () => {
  assert.deepEqual(
    attachmentContextPill(['src/main.rs', 'docs/notes.md', 'src/main.rs', ' ']),
    {
      label: 'Attachments 2',
      tone: 'mute',
      title: 'docs/notes.md\nsrc/main.rs',
    },
  );
});

test('attachmentContextPill omits empty attachment inventories', () => {
  assert.equal(attachmentContextPill([]), undefined);
  assert.equal(attachmentContextPill(undefined), undefined);
});
