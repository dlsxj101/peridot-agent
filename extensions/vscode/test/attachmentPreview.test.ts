import test from 'node:test';
import assert from 'node:assert/strict';
import * as fs from 'node:fs';
import * as os from 'node:os';
import * as path from 'node:path';

import {
  IMAGE_ATTACHMENT_PREVIEW_MAX_BYTES,
  addAttachmentPreviewUris,
  isPreviewableImageMediaType,
} from '../src/attachmentPreview';

test('addAttachmentPreviewUris adds preview URI for workspace image attachments', () => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), 'peridot-attachment-preview-'));
  try {
    fs.mkdirSync(path.join(root, 'screens'));
    fs.writeFileSync(path.join(root, 'screens', 'shot.png'), Buffer.from([0x89, 0x50, 0x4e, 0x47]));

    const result = addAttachmentPreviewUris(
      {
        kind: 'attach',
        attachment: {
          path: 'screens/shot.png',
          media_type: 'image/png',
          inlined: false,
        },
      },
      root,
      (absolutePath) => `preview://${path.basename(absolutePath)}`,
    );

    assert.equal(result.attachment?.preview_uri, 'preview://shot.png');
    assert.equal(result.attachment?.previewUri, 'preview://shot.png');
  } finally {
    fs.rmSync(root, { recursive: true, force: true });
  }
});

test('addAttachmentPreviewUris skips text, svg, escapes, and large images', () => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), 'peridot-attachment-preview-'));
  try {
    fs.writeFileSync(path.join(root, 'note.txt'), 'hello');
    fs.writeFileSync(path.join(root, 'vector.svg'), '<svg></svg>');
    fs.writeFileSync(
      path.join(root, 'large.png'),
      Buffer.alloc(IMAGE_ATTACHMENT_PREVIEW_MAX_BYTES + 1),
    );

    const result = addAttachmentPreviewUris(
      {
        kind: 'attachments',
        attachments: [
          { path: 'note.txt', media_type: 'text/plain' },
          { path: 'vector.svg', media_type: 'image/svg+xml' },
          { path: 'large.png', media_type: 'image/png' },
          { path: '../outside.png', media_type: 'image/png' },
        ],
      },
      root,
      () => 'preview://unexpected',
    );

    assert.deepEqual(
      result.attachments?.map((attachment) => attachment.preview_uri),
      [undefined, undefined, undefined, undefined],
    );
  } finally {
    fs.rmSync(root, { recursive: true, force: true });
  }
});

test('isPreviewableImageMediaType rejects svg and non-images', () => {
  assert.equal(isPreviewableImageMediaType('image/png'), true);
  assert.equal(isPreviewableImageMediaType('image/jpeg'), true);
  assert.equal(isPreviewableImageMediaType('image/svg+xml'), false);
  assert.equal(isPreviewableImageMediaType('text/plain'), false);
  assert.equal(isPreviewableImageMediaType(undefined), false);
});
