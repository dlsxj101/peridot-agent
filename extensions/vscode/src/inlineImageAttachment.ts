import * as path from 'path';
import type { InlineImageAttachmentPayload } from './types';

export const INLINE_IMAGE_ATTACHMENT_MAX_BYTES = 5 * 1024 * 1024;

export function decodeInlineImageAttachment(
  image: InlineImageAttachmentPayload,
): { fileName: string; mediaType: string; bytes: Buffer } {
  const mediaType = normalizeInlineImageMediaType(image.mediaType);
  if (!mediaType) {
    throw new Error('Only PNG, JPEG, GIF, WebP, and BMP images can be pasted or dropped.');
  }
  const bytes = Buffer.from(image.dataBase64, 'base64');
  if (bytes.length === 0) {
    throw new Error('The pasted image was empty.');
  }
  if (bytes.length > INLINE_IMAGE_ATTACHMENT_MAX_BYTES) {
    throw new Error(
      `The pasted image is ${bytes.length} bytes, above the ${INLINE_IMAGE_ATTACHMENT_MAX_BYTES} byte limit.`,
    );
  }
  return {
    bytes,
    mediaType,
    fileName: safeInlineImageFileName(image.fileName, mediaType),
  };
}

export function normalizeInlineImageMediaType(mediaType: string): string | undefined {
  const normalized = mediaType.trim().toLowerCase();
  return inlineImageExtension(normalized) ? normalized : undefined;
}

export function safeInlineImageFileName(fileName: string | undefined, mediaType: string): string {
  const fallback = `image.${inlineImageExtension(mediaType) ?? 'png'}`;
  const basename = path.basename(fileName?.trim() || fallback);
  const ext = inlineImageExtension(mediaType) ?? 'png';
  const stem = basename
    .replace(/\.[^.]+$/, '')
    .replace(/[^A-Za-z0-9._-]+/g, '-')
    .replace(/^-+|-+$/g, '')
    .slice(0, 64) || 'image';
  return `${stem}.${ext}`;
}

function inlineImageExtension(mediaType: string): string | undefined {
  switch (mediaType) {
    case 'image/png':
      return 'png';
    case 'image/jpeg':
      return 'jpg';
    case 'image/gif':
      return 'gif';
    case 'image/webp':
      return 'webp';
    case 'image/bmp':
      return 'bmp';
    default:
      return undefined;
  }
}
