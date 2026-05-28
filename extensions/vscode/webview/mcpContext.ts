import type { McpServerSummary } from '../src/types';

export interface McpContextPill {
  label: string;
  tone: 'mute' | 'warn';
  title: string;
}

export function mcpContextPill(servers: McpServerSummary[] | undefined): McpContextPill | undefined {
  const normalized = (servers ?? [])
    .map((server) => ({
      name: server.name.trim(),
      transport: server.transport?.trim(),
      toolCount: server.toolCount,
      connected: server.connected,
    }))
    .filter((server) => server.name.length > 0);
  if (normalized.length === 0) return undefined;

  const knownConnections = normalized.filter((server) => typeof server.connected === 'boolean');
  const connected = normalized.filter((server) => server.connected === true).length;
  const disconnected = normalized.filter((server) => server.connected === false).length;
  const totalTools = normalized.reduce((total, server) => total + (server.toolCount ?? 0), 0);
  const hasDisconnected = disconnected > 0;
  const status = knownConnections.length > 0 ? `${connected}/${knownConnections.length} up` : `${normalized.length} configured`;
  const tools = totalTools > 0 ? ` · ${totalTools} tools` : '';

  return {
    label: `MCP ${status}${tools}`,
    tone: hasDisconnected ? 'warn' : 'mute',
    title: normalized.map(mcpServerTitleLine).join('\n'),
  };
}

function mcpServerTitleLine(server: {
  name: string;
  transport?: string;
  toolCount?: number;
  connected?: boolean;
}): string {
  const details = [
    server.transport,
    typeof server.toolCount === 'number' ? `${server.toolCount} tools` : undefined,
    server.connected === true ? 'connected' : server.connected === false ? 'disconnected' : undefined,
  ].filter(Boolean);
  return details.length > 0 ? `${server.name}: ${details.join(', ')}` : server.name;
}
