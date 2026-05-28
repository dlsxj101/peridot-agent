import type { McpServerSummary } from './types';

export interface McpServerChoice {
  name: string;
  label: string;
  description?: string;
}

export type McpTransport = 'stdio' | 'http';

export function mcpServerChoices(servers: McpServerSummary[]): McpServerChoice[] {
  const choices: McpServerChoice[] = [];
  const seen = new Set<string>();
  for (const server of servers) {
    const name = server.name.trim();
    if (!name || seen.has(name)) continue;
    seen.add(name);
    const description = mcpServerDescription(server);
    choices.push({
      name,
      label: name,
      ...(description ? { description } : {}),
    });
  }
  return choices.sort((a, b) => a.label.localeCompare(b.label));
}

export function mcpTestSlashCommand(name: string): string {
  return `/mcp test ${mcpServerNameArg(name)}`;
}

export function mcpRemoveSlashCommand(name: string): string {
  return `/mcp remove ${mcpServerNameArg(name)}`;
}

export function mcpAddSlashCommand(name: string, transport: McpTransport, target: string): string {
  const targetArg = target.trim();
  if (!targetArg) {
    throw new Error('MCP server target is required.');
  }
  if (/[\r\n]/.test(targetArg)) {
    throw new Error('MCP server target must be a single line.');
  }
  return `/mcp add ${mcpServerNameArg(name)} ${transport} ${targetArg}`;
}

export function mcpConfigChangingSlashCommand(input: string): boolean {
  const parts = input
    .trim()
    .split(/\s+/)
    .map((part) => part.toLowerCase());
  return parts[0] === '/mcp' && (parts[1] === 'add' || parts[1] === 'remove');
}

function mcpServerNameArg(name: string): string {
  const target = name.trim();
  if (!target) {
    throw new Error('MCP server name is required.');
  }
  if (/\s/.test(target)) {
    throw new Error('MCP server name cannot contain whitespace.');
  }
  return target;
}

function mcpServerDescription(server: McpServerSummary): string | undefined {
  const parts = [
    server.transport,
    typeof server.toolCount === 'number' ? `${server.toolCount} tool(s)` : undefined,
    server.connected === true ? 'connected' : server.connected === false ? 'disconnected' : undefined,
  ].filter((part): part is string => typeof part === 'string' && part.trim().length > 0);
  return parts.length > 0 ? parts.join(' - ') : undefined;
}
