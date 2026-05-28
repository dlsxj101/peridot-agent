import test from 'node:test';
import assert from 'node:assert/strict';

import {
  INLINE_IMAGE_ATTACHMENT_MAX_BYTES,
  decodeInlineImageAttachment,
  normalizeInlineImageMediaType,
  safeInlineImageFileName,
} from '../src/inlineImageAttachment';
import {
  WEBVIEW_INLINE_IMAGE_MAX_BYTES,
  inlineImagePayload,
  isAttachableInlineImage,
} from '../webview/inlineImageAttachment';

test('decodeInlineImageAttachment validates and sanitizes pasted images', () => {
  const decoded = decodeInlineImageAttachment({
    fileName: '../../Screen Shot 1.PNG',
    mediaType: 'image/png',
    dataBase64: Buffer.from([1, 2, 3]).toString('base64'),
  });

  assert.equal(decoded.fileName, 'Screen-Shot-1.png');
  assert.equal(decoded.mediaType, 'image/png');
  assert.deepEqual([...decoded.bytes], [1, 2, 3]);
});

test('decodeInlineImageAttachment rejects unsupported and oversized payloads', () => {
  assert.throws(
    () =>
      decodeInlineImageAttachment({
        mediaType: 'image/svg+xml',
        dataBase64: Buffer.from('svg').toString('base64'),
      }),
    /Only PNG/,
  );
  assert.throws(
    () =>
      decodeInlineImageAttachment({
        mediaType: 'image/png',
        dataBase64: Buffer.alloc(INLINE_IMAGE_ATTACHMENT_MAX_BYTES + 1).toString('base64'),
      }),
    /above/,
  );
});

test('inline image media helpers normalize expected formats', () => {
  assert.equal(normalizeInlineImageMediaType(' IMAGE/JPEG '), 'image/jpeg');
  assert.equal(normalizeInlineImageMediaType('image/svg+xml'), undefined);
  assert.equal(safeInlineImageFileName(' pasted screen.webp ', 'image/webp'), 'pasted-screen.webp');
});

test('webview inlineImagePayload encodes attachable image files', async () => {
  const payload = await inlineImagePayload({
    name: 'drop.png',
    type: 'image/png',
    size: 3,
    arrayBuffer: async () => Uint8Array.from([4, 5, 6]).buffer,
  });

  assert.equal(payload?.fileName, 'drop.png');
  assert.equal(payload?.mediaType, 'image/png');
  assert.deepEqual([...Buffer.from(payload?.dataBase64 ?? '', 'base64')], [4, 5, 6]);
});

test('webview inline image filter rejects unsupported and large files', async () => {
  assert.equal(isAttachableInlineImage({ type: 'image/png', size: 1 }), true);
  assert.equal(isAttachableInlineImage({ type: 'image/svg+xml', size: 1 }), false);
  assert.equal(
    isAttachableInlineImage({ type: 'image/png', size: WEBVIEW_INLINE_IMAGE_MAX_BYTES + 1 }),
    false,
  );
  assert.equal(
    await inlineImagePayload({
      name: 'large.png',
      type: 'image/png',
      size: WEBVIEW_INLINE_IMAGE_MAX_BYTES + 1,
      arrayBuffer: async () => new ArrayBuffer(0),
    }),
    undefined,
  );
});
