export function sessionSearchSlashCommand(query: string): string {
  const trimmed = query.trim();
  if (!trimmed) {
    throw new Error('Search query is required.');
  }
  return `/session search ${trimmed}`;
}
