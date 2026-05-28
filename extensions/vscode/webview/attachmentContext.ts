export interface AttachmentPillView {
  label: string;
  tone: 'good' | 'warn' | 'mute';
  title: string;
}

export function attachmentContextPill(paths: readonly string[] | undefined): AttachmentPillView | undefined {
  const normalized = [...new Set((paths ?? []).map((path) => path.trim()).filter(Boolean))].sort();
  if (normalized.length === 0) return undefined;
  return {
    label: `Attachments ${normalized.length}`,
    tone: 'mute',
    title: normalized.join('\n'),
  };
}
