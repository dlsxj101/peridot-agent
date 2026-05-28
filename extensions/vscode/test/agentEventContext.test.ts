import test from 'node:test';
import assert from 'node:assert/strict';

import { agentsSummaryForLoadedEvent, mcpServersForStatusEvent } from '../src/agentEventContext';

test('agentsSummaryForLoadedEvent normalizes AGENTS.md load events', () => {
  assert.deepEqual(
    agentsSummaryForLoadedEvent({
      rule_count: 7,
      paths: [' AGENTS.md ', '', 4, '.peridot/AGENTS.md'],
    }),
    {
      ruleCount: 7,
      paths: ['AGENTS.md', '.peridot/AGENTS.md'],
    },
  );
});

test('agentsSummaryForLoadedEvent ignores unrelated events', () => {
  assert.equal(agentsSummaryForLoadedEvent({}), undefined);
});

test('mcpServersForStatusEvent normalizes daemon mcp status snapshots', () => {
  assert.deepEqual(
    mcpServersForStatusEvent({
      servers: [
        { name: ' filesystem ', tool_count: 4, connected: true },
        { name: 'github', transport: 'stdio', tool_count: 0, connected: false },
      ],
    }),
    [
      { name: 'filesystem', toolCount: 4, connected: true },
      { name: 'github', transport: 'stdio', toolCount: 0, connected: false },
    ],
  );
});

test('mcpServersForStatusEvent drops malformed entries', () => {
  assert.deepEqual(
    mcpServersForStatusEvent({
      servers: [{ name: '' }, { name: 'ok' }, null, { tool_count: 2 }],
    }),
    [{ name: 'ok' }],
  );
});

test('mcpServersForStatusEvent ignores events without a server snapshot', () => {
  assert.equal(mcpServersForStatusEvent({}), undefined);
});
