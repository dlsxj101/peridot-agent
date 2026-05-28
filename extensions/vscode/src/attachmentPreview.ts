import * as fs from 'fs';
import * as path from 'path';
import type { AttachmentView, CommandResultItem, CommandResultView } from './types';

export const IMAGE_ATTACHMENT_PREVIEW_MAX_BYTES = 2 * 1024 * 1024;

type PreviewResolver = (absolutePath: string) => string | undefined;

export function addAttachmentPreviewUris(
  result: CommandResultView,
  workspaceRoot: string,
  resolvePreviewUri: PreviewResolver,
): CommandResultView {
  if (!hasAttachmentPayload(result)) return result;
  const next: CommandResultView = { ...result };
  if (result.attachment) {
    next.attachment = addPreviewUri(result.attachment, workspaceRoot, resolvePreviewUri);
  }
  if (Array.isArray(result.attachments)) {
    next.attachments = result.attachments.map((attachment) =>
      addPreviewUri(attachment, workspaceRoot, resolvePreviewUri),
    );
  }
  if (Array.isArray(result.items)) {
    next.items = result.items.map((item) =>
      item.source === 'attachment'
        ? addPreviewUri(item, workspaceRoot, resolvePreviewUri)
        : item,
    );
  }
  if (Array.isArray(result.removed)) {
    next.removed = result.removed.map((attachment) =>
      addPreviewUri(attachment, workspaceRoot, resolvePreviewUri),
    );
  }
  return next;
}

function hasAttachmentPayload(result: CommandResultView): boolean {
  return (
    result.kind === 'attach' ||
    result.kind === 'attachments' ||
    result.kind === 'detach' ||
    Boolean(result.attachment) ||
    Boolean(result.attachments) ||
    Boolean(result.removed) ||
    Boolean(result.items?.some((item) => item.source === 'attachment'))
  );
}

function addPreviewUri<T extends AttachmentView | CommandResultItem>(
  attachment: T,
  workspaceRoot: string,
  resolvePreviewUri: PreviewResolver,
): T {
  const mediaType = attachment.media_type ?? attachment.mediaType;
  if (!isPreviewableImageMediaType(mediaType)) return attachment;
  const relativePath = attachment.path ?? ('label' in attachment ? attachment.label : undefined);
  const absolutePath = workspaceLocalPath(workspaceRoot, relativePath);
  if (!absolutePath || !isSmallEnoughImage(absolutePath)) return attachment;
  const previewUri = resolvePreviewUri(absolutePath);
  if (!previewUri) return attachment;
  return {
    ...attachment,
    preview_uri: previewUri,
    previewUri,
  };
}

export function isPreviewableImageMediaType(mediaType: string | undefined): boolean {
  if (!mediaType?.startsWith('image/')) return false;
  return mediaType !== 'image/svg+xml';
}

function workspaceLocalPath(workspaceRoot: string, relativePath: string | undefined): string | undefined {
  if (!relativePath?.trim()) return undefined;
  const normalizedRoot = path.resolve(workspaceRoot);
  const absolutePath = path.resolve(normalizedRoot, relativePath);
  const relative = path.relative(normalizedRoot, absolutePath);
  if (relative.startsWith('..') || path.isAbsolute(relative)) return undefined;
  return absolutePath;
}

function isSmallEnoughImage(absolutePath: string): boolean {
  try {
    const stat = fs.statSync(absolutePath);
    return stat.isFile() && stat.size <= IMAGE_ATTACHMENT_PREVIEW_MAX_BYTES;
  } catch {
    return false;
  }
}
