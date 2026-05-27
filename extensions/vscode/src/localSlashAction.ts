export type LocalSlashAction = 'showInfo';

export function localSlashAction(input: string): LocalSlashAction | undefined {
  const trimmed = input.trim();
  if (!trimmed.startsWith('/')) return undefined;
  const [command, ...restParts] = trimmed.slice(1).split(/\s+/);
  const rest = restParts.join(' ').trim();
  if (rest.length > 0) return undefined;
  switch (command) {
    case 'sidepanel':
    case 'status':
      return 'showInfo';
    default:
      return undefined;
  }
}
