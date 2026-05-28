import test from 'node:test';
import assert from 'node:assert/strict';

import { mcpServersForStatusEvent } from '../src/agentEventContext';

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
