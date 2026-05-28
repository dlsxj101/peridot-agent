import type { McpServerSummary } from './types';

export function mcpServersForStatusEvent(
  event: Record<string, unknown>,
): McpServerSummary[] | undefined {
  if (!Array.isArray(event.servers)) return undefined;
  const servers = event.servers
    .map((entry) => {
      if (!isRecord(entry)) return undefined;
      const name = textField(entry, 'name')?.trim();
      if (!name) return undefined;
      const server: McpServerSummary = { name };
      const transport = textField(entry, 'transport')?.trim();
      const toolCount = numberField(entry, 'tool_count');
      const connected = booleanField(entry, 'connected');
      if (transport) server.transport = transport;
      if (typeof toolCount === 'number') server.toolCount = toolCount;
      if (typeof connected === 'boolean') server.connected = connected;
      return server;
    })
    .filter((server): server is McpServerSummary => Boolean(server));
  return servers;
}

function textField(record: Record<string, unknown>, key: string): string | undefined {
  const value = record[key];
  return typeof value === 'string' ? value : undefined;
}

function numberField(record: Record<string, unknown>, key: string): number | undefined {
  const value = record[key];
  return typeof value === 'number' && Number.isFinite(value) ? value : undefined;
}

function booleanField(record: Record<string, unknown>, key: string): boolean | undefined {
  const value = record[key];
  return typeof value === 'boolean' ? value : undefined;
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null;
}
