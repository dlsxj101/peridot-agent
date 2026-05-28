export interface SessionImportCommandInput {
  source: string;
  id?: string;
  force?: boolean;
}

export function sessionImportSlashCommand(input: SessionImportCommandInput): string {
  const source = input.source.trim();
  if (!source) {
    throw new Error('Session import source is required.');
  }
  const id = input.id?.trim();
  if (id && /\s/.test(id)) {
    throw new Error('Session import id cannot contain whitespace.');
  }
  return [
    '/session import',
    source,
    ...(id ? ['--id', id] : []),
    ...(input.force === true ? ['--force'] : []),
  ].join(' ');
}
