import type { McpServerSummary } from './types';

export interface McpServerChoice {
  name: string;
  label: string;
  description?: string;
}

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
  const target = name.trim();
  if (!target) {
    throw new Error('MCP server name is required.');
  }
  if (/\s/.test(target)) {
    throw new Error('MCP server name cannot contain whitespace.');
  }
  return `/mcp test ${target}`;
}

function mcpServerDescription(server: McpServerSummary): string | undefined {
  const parts = [
    server.transport,
    typeof server.toolCount === 'number' ? `${server.toolCount} tool(s)` : undefined,
    server.connected === true ? 'connected' : server.connected === false ? 'disconnected' : undefined,
  ].filter((part): part is string => typeof part === 'string' && part.trim().length > 0);
  return parts.length > 0 ? parts.join(' - ') : undefined;
}
