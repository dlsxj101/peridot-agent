import type { AgentsSummary, McpServerSummary } from './types';
import { isRecord } from './util';

export function agentsSummaryForLoadedEvent(
  event: Record<string, unknown>,
): AgentsSummary | undefined {
  const ruleCount = numberField(event, 'rule_count');
  const paths = Array.isArray(event.paths)
    ? event.paths
        .filter((path): path is string => typeof path === 'string')
        .map((path) => path.trim())
        .filter((path) => path.length > 0)
    : [];
  if (typeof ruleCount !== 'number' && paths.length === 0) return undefined;
  return {
    ruleCount: ruleCount ?? 0,
    paths,
  };
}

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

