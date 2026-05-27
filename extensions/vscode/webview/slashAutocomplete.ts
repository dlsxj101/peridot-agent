import type { SlashCommandSpec } from '../src/types';

export interface SlashArgumentContext {
  command: SlashCommandSpec;
  options: string[];
}

export function filteredSlashCommands(
  input: string,
  slashCommands: SlashCommandSpec[],
): SlashCommandSpec[] {
  const query = input.trimEnd();
  if (!query.startsWith('/') || query.includes('\n')) return [];
  const needle = query.slice(1).trim().toLowerCase();
  const visibleCommands = slashCommands.filter((command) => command.archived !== true);
  if (needle.length === 0) return visibleCommands;
  return visibleCommands
    .filter((command) => {
      const name = command.name.slice(1).toLowerCase();
      const description = command.description.toLowerCase();
      return name.startsWith(needle) || name.includes(` ${needle}`) || description.includes(needle);
    })
    .sort((a, b) => slashCommandRank(a, needle) - slashCommandRank(b, needle) || a.name.localeCompare(b.name));
}

export function slashPickerItemCount(input: string, slashCommands: SlashCommandSpec[]): number {
  const argumentContext = slashArgumentContext(input, slashCommands);
  if (argumentContext) return argumentContext.options.length;
  return filteredSlashCommands(input, slashCommands).length;
}

export function slashExactSelectionIsRunnable(
  input: string,
  slashCommands: SlashCommandSpec[],
  selected: number,
): boolean {
  if (slashArgumentContext(input, slashCommands)) return false;
  const matches = filteredSlashCommands(input, slashCommands);
  const command = matches[selected];
  if (!command) return false;
  return input.trim() === command.name && (!command.argHint || command.argHint.startsWith('['));
}

export function slashArgumentContext(
  input: string,
  slashCommands: SlashCommandSpec[],
): SlashArgumentContext | undefined {
  const query = input;
  if (!query.startsWith('/') || query.includes('\n')) return undefined;
  const skillContext = skillNameArgumentContext(query, slashCommands);
  if (skillContext) return skillContext;
  const command = [...slashCommands]
    .sort((a, b) => b.name.length - a.name.length)
    .find(
      (candidate) =>
        !(query === candidate.name && candidate.argHint?.trim().startsWith('[')) &&
        (query === candidate.name || query.startsWith(`${candidate.name} `)),
    );
  if (!command) return undefined;
  const options = slashArgumentOptions(command);
  if (options.length === 0) return undefined;
  const rest = query.slice(command.name.length).trim().toLowerCase();
  if (rest && options.some((option) => option.toLowerCase() === rest)) return undefined;
  const filtered = rest
    ? options.filter((option) => option.toLowerCase().startsWith(rest))
    : options;
  return { command, options: filtered };
}

function skillNameArgumentContext(
  query: string,
  slashCommands: SlashCommandSpec[],
): SlashArgumentContext | undefined {
  const commandName = [
    '/skills show',
    '/skills view',
    '/skills use',
    '/skills pin',
    '/skills unpin',
    '/skills archive',
    '/skills restore',
  ]
    .filter((candidate) => query === candidate || query.startsWith(`${candidate} `))
    .sort((a, b) => b.length - a.length)[0];
  if (!commandName) return undefined;
  const options = skillNameOptionsForCommand(commandName, slashCommands);
  if (options.length === 0) return undefined;
  const rest = query.slice(commandName.length).trim().replace(/^\/+/, '').toLowerCase();
  if (rest && options.some((option) => option.toLowerCase() === rest)) return undefined;
  const filtered = rest
    ? options.filter((option) => option.toLowerCase().startsWith(rest))
    : options;
  if (filtered.length === 0) return undefined;
  return {
    command: {
      name: commandName,
      description: 'stored skill',
      category: 'skill',
    },
    options: filtered,
  };
}

function skillNameOptionsForCommand(commandName: string, slashCommands: SlashCommandSpec[]): string[] {
  const names = slashCommands
    .filter((command) => command.category === 'skill')
    .filter((command) => skillAppliesToCommand(commandName, command.archived === true))
    .map((command) => command.name.trim().replace(/^\/+/, ''))
    .filter((name) => name.length > 0 && !name.startsWith('skills') && !name.includes(' '));
  return [...new Set(names)].sort((a, b) => a.localeCompare(b));
}

function skillAppliesToCommand(commandName: string, archived: boolean): boolean {
  if (commandName === '/skills restore') return archived;
  if (commandName === '/skills show' || commandName === '/skills view') return true;
  return !archived;
}

export function slashArgumentOptions(command: SlashCommandSpec): string[] {
  if (Array.isArray(command.argOptions) && command.argOptions.length > 0) {
    return command.argOptions.filter((option) => option.trim().length > 0);
  }
  const hint = command.argHint?.trim();
  if (!hint) return [];
  const opensChoiceList =
    (hint.startsWith('<') && hint.endsWith('>')) ||
    (hint.startsWith('[') && hint.endsWith(']'));
  if (!opensChoiceList) return [];
  const body = hint.slice(1, -1);
  if (!body.includes('|') || /\s/.test(body)) return [];
  return body
    .split('|')
    .map((option) => option.trim())
    .filter((option) => option.length > 0 && !isPlaceholderSlashOption(option));
}

function slashCommandRank(command: SlashCommandSpec, needle: string): number {
  const name = command.name.slice(1).toLowerCase();
  const description = command.description.toLowerCase();
  if (name.startsWith(needle)) return 0;
  if (name.includes(` ${needle}`)) return 1;
  if (description.includes(needle)) return 2;
  return 3;
}

function isPlaceholderSlashOption(option: string): boolean {
  if (option.includes('<') || option.includes('>')) return true;
  return new Set(['branch', 'command', 'id', 'index', 'name', 'objective', 'task', 'text', 'title', 'url']).has(
    option.toLowerCase(),
  );
}
