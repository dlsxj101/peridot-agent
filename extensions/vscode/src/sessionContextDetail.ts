export interface SessionContextLike {
  id?: string;
  notes_count?: number;
  last_note?: string | null;
  attachment_count?: number;
  attachment_paths?: string[];
}

export function sessionContextDetail(
  session: SessionContextLike | undefined,
  fallbackId?: string,
): string | undefined {
  const id = fallbackId?.trim() || session?.id?.trim();
  const parts = session ? sessionContextParts(session) : [];
  if (!id && parts.length === 0) return undefined;
  return [id, ...parts].filter((part): part is string => Boolean(part)).join(' · ');
}

export function sessionContextParts(session: SessionContextLike): string[] {
  const parts: string[] = [];
  const note = compactText(session.last_note);
  const noteCount = positiveInteger(session.notes_count);
  if (noteCount !== undefined && note) {
    parts.push(`Notes ${noteCount}: ${note}`);
  } else if (noteCount !== undefined) {
    parts.push(`Notes ${noteCount}`);
  } else if (note) {
    parts.push(`Note: ${note}`);
  }

  const attachmentCount =
    positiveInteger(session.attachment_count) ?? positiveInteger(session.attachment_paths?.length);
  if (attachmentCount !== undefined) {
    parts.push(`Attachments ${attachmentCount}`);
  }
  return parts;
}

function positiveInteger(value: number | undefined): number | undefined {
  if (typeof value !== 'number' || !Number.isFinite(value)) return undefined;
  const rounded = Math.floor(value);
  return rounded > 0 ? rounded : undefined;
}

function compactText(value: string | null | undefined): string | undefined {
  const text = value?.replace(/\s+/g, ' ').trim();
  if (!text) return undefined;
  return text.length > 80 ? `${text.slice(0, 77)}...` : text;
}
