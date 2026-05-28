import type { InlineImageAttachmentPayload } from '../src/types';

export const WEBVIEW_INLINE_IMAGE_MAX_BYTES = 5 * 1024 * 1024;

export interface InlineImageLike {
  name?: string;
  size: number;
  type: string;
  arrayBuffer: () => Promise<ArrayBuffer>;
}

const INLINE_IMAGE_TYPES = new Set([
  'image/png',
  'image/jpeg',
  'image/gif',
  'image/webp',
  'image/bmp',
]);

export function isAttachableInlineImage(file: Pick<InlineImageLike, 'size' | 'type'>): boolean {
  return INLINE_IMAGE_TYPES.has(file.type.toLowerCase()) && file.size <= WEBVIEW_INLINE_IMAGE_MAX_BYTES;
}

export async function inlineImagePayload(
  file: InlineImageLike,
): Promise<InlineImageAttachmentPayload | undefined> {
  if (!isAttachableInlineImage(file)) return undefined;
  const bytes = new Uint8Array(await file.arrayBuffer());
  return {
    fileName: file.name,
    mediaType: file.type.toLowerCase(),
    dataBase64: uint8ToBase64(bytes),
  };
}

function uint8ToBase64(bytes: Uint8Array): string {
  let binary = '';
  const chunkSize = 0x8000;
  for (let offset = 0; offset < bytes.length; offset += chunkSize) {
    const chunk = bytes.subarray(offset, offset + chunkSize);
    binary += String.fromCharCode(...chunk);
  }
  return btoa(binary);
}
