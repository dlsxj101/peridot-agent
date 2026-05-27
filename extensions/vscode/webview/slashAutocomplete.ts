import type { SlashCommandSpec } from '../src/types';

export interface SlashArgumentContext {
  command: SlashCommandSpec;
  options: string[];
  appendSpace?: boolean;
}

export interface SlashSessionTarget {
  id: string;
  title?: string;
}

export interface SlashMcpServerTarget {
  name: string;
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

export function slashPickerItemCount(
  input: string,
  slashCommands: SlashCommandSpec[],
  sessionTargets: SlashSessionTarget[] = [],
  mcpServers: SlashMcpServerTarget[] = [],
  models: string[] = [],
  branches: string[] = [],
): number {
  const argumentContext = slashArgumentContext(input, slashCommands, sessionTargets, mcpServers, models, branches);
  if (argumentContext) return argumentContext.options.length;
  return filteredSlashCommands(input, slashCommands).length;
}

export function slashExactSelectionIsRunnable(
  input: string,
  slashCommands: SlashCommandSpec[],
  selected: number,
  sessionTargets: SlashSessionTarget[] = [],
  mcpServers: SlashMcpServerTarget[] = [],
  models: string[] = [],
  branches: string[] = [],
): boolean {
  if (slashArgumentContext(input, slashCommands, sessionTargets, mcpServers, models, branches)) return false;
  const matches = filteredSlashCommands(input, slashCommands);
  const command = matches[selected];
  if (!command) return false;
  return input.trim() === command.name && (!command.argHint || command.argHint.startsWith('['));
}

export function slashArgumentContext(
  input: string,
  slashCommands: SlashCommandSpec[],
  sessionTargets: SlashSessionTarget[] = [],
  mcpServers: SlashMcpServerTarget[] = [],
  models: string[] = [],
  branches: string[] = [],
): SlashArgumentContext | undefined {
  const query = input;
  if (!query.startsWith('/') || query.includes('\n')) return undefined;
  const modelContext = modelNameArgumentContext(query, models);
  if (modelContext) return modelContext;
  const skillContext = skillNameArgumentContext(query, slashCommands);
  if (skillContext) return skillContext;
  const sessionContext = sessionTargetArgumentContext(query, sessionTargets);
  if (sessionContext) return sessionContext;
  const mcpServerContext = mcpServerArgumentContext(query, mcpServers);
  if (mcpServerContext) return mcpServerContext;
  const mcpAddContext = mcpAddTransportArgumentContext(query);
  if (mcpAddContext) return mcpAddContext;
  const branchContext = branchSnapshotArgumentContext(query, branches);
  if (branchContext) return branchContext;
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

function branchSnapshotArgumentContext(
  query: string,
  branches: string[],
): SlashArgumentContext | undefined {
  const commandName = '/branch restore';
  if (query !== commandName && !query.startsWith(`${commandName} `)) return undefined;
  const needle = query.slice(commandName.length).trim().toLowerCase();
  if (/\s/.test(needle)) return undefined;
  const options = [...new Set(
    branches
      .map((branch) => branch.trim())
      .filter((branch) => branch.length > 0)
      .filter((branch) => needle.length === 0 || branch.toLowerCase().startsWith(needle)),
  )].sort((a, b) => a.localeCompare(b));
  if (needle && options.some((option) => option.toLowerCase() === needle)) return undefined;
  if (options.length === 0) return undefined;
  return {
    command: {
      name: commandName,
      description: 'branch snapshot',
      category: 'branch',
    },
    options,
  };
}

function modelNameArgumentContext(
  query: string,
  models: string[],
): SlashArgumentContext | undefined {
  const commandName = ['/subagent model', '/model']
    .filter((candidate) => query === candidate || query.startsWith(`${candidate} `))
    .sort((a, b) => b.length - a.length)[0];
  if (!commandName) return undefined;
  const needle = query.slice(commandName.length).trim().toLowerCase();
  if (/\s/.test(needle)) return undefined;
  const options = [...new Set([
    ...models.map((model) => model.trim()).filter((model) => model.length > 0),
    ...(commandName === '/subagent model' ? ['reset'] : []),
  ])]
    .filter((model) => needle.length === 0 || model.toLowerCase().startsWith(needle))
    .sort((a, b) => a.localeCompare(b));
  if (needle && options.some((option) => option.toLowerCase() === needle)) return undefined;
  if (options.length === 0) return undefined;
  return {
    command: {
      name: commandName,
      description: 'model',
      category: 'session',
    },
    options,
  };
}

function mcpServerArgumentContext(
  query: string,
  mcpServers: SlashMcpServerTarget[],
): SlashArgumentContext | undefined {
  const commandName = ['/mcp remove', '/mcp test']
    .filter((candidate) => query === candidate || query.startsWith(`${candidate} `))
    .sort((a, b) => b.length - a.length)[0];
  if (!commandName) return undefined;
  const needle = query.slice(commandName.length).trim().toLowerCase();
  if (/\s/.test(needle)) return undefined;
  const options = [...new Set(
    mcpServers
      .map((server) => server.name.trim())
      .filter((name) => name.length > 0)
      .filter((name) => needle.length === 0 || name.toLowerCase().startsWith(needle)),
  )].sort((a, b) => a.localeCompare(b));
  if (needle && options.some((option) => option.toLowerCase() === needle)) return undefined;
  if (options.length === 0) return undefined;
  return {
    command: {
      name: commandName,
      description: 'MCP server',
      category: 'mcp',
    },
    options,
  };
}

function sessionTargetArgumentContext(
  query: string,
  sessionTargets: SlashSessionTarget[],
): SlashArgumentContext | undefined {
  const commandName = ['/session switch', '/session close', '/session delete', '/session rename']
    .filter((candidate) => query === candidate || query.startsWith(`${candidate} `))
    .sort((a, b) => b.length - a.length)[0];
  if (!commandName) return undefined;
  const rest = query.slice(commandName.length).trim();
  if (commandName === '/session rename' && /\s/.test(rest)) return undefined;
  const needle = rest.toLowerCase();
  const options = [...new Set(
    sessionTargets
      .filter((session) => session.id.trim().length > 0)
      .filter(
        (session) =>
          needle.length === 0 ||
          session.id.toLowerCase().startsWith(needle) ||
          (session.title ?? '').toLowerCase().startsWith(needle),
      )
      .map((session) => session.id.trim()),
  )].sort((a, b) => a.localeCompare(b));
  if (needle && options.some((option) => option.toLowerCase() === needle)) return undefined;
  if (options.length === 0) return undefined;
  return {
    command: {
      name: commandName,
      description: 'session target',
      category: 'session',
    },
    options,
    appendSpace: commandName === '/session rename',
  };
}

function mcpAddTransportArgumentContext(query: string): SlashArgumentContext | undefined {
  const commandName = '/mcp add';
  if (!query.startsWith(`${commandName} `)) return undefined;
  const rest = query.slice(commandName.length).trimStart();
  const hasTrailingSpace = /\s$/.test(rest);
  const parts = rest.split(/\s+/).filter((part) => part.length > 0);
  const serverName = parts[0]?.trim();
  if (!serverName) return undefined;
  const transportPrefix = parts[1];
  if (parts.length > 2) return undefined;
  if (transportPrefix === undefined && !hasTrailingSpace) return undefined;
  const needle = (transportPrefix ?? '').toLowerCase();
  const candidates = ['stdio', 'http'];
  if (needle && candidates.some((candidate) => candidate.toLowerCase() === needle)) return undefined;
  const options = candidates.filter((candidate) => needle.length === 0 || candidate.startsWith(needle));
  if (options.length === 0) return undefined;
  return {
    command: {
      name: `${commandName} ${serverName}`,
      description: 'MCP transport',
      category: 'mcp',
    },
    options,
    appendSpace: true,
  };
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
