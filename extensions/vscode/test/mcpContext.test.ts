import test from 'node:test';
import assert from 'node:assert/strict';

import { mcpContextPill } from '../webview/mcpContext';

test('mcpContextPill summarizes configured servers and tools', () => {
  assert.deepEqual(
    mcpContextPill([
      { name: 'filesystem', transport: 'stdio', toolCount: 4, connected: true },
      { name: 'github', transport: 'http', toolCount: 7, connected: true },
    ]),
    {
      label: 'MCP 2/2 up · 11 tools',
      tone: 'mute',
      title: 'filesystem: stdio, 4 tools, connected\ngithub: http, 7 tools, connected',
    },
  );
});

test('mcpContextPill warns when a known server is disconnected', () => {
  const pill = mcpContextPill([
    { name: 'filesystem', connected: true },
    { name: 'github', connected: false },
  ]);

  assert.equal(pill?.label, 'MCP 1/2 up');
  assert.equal(pill?.tone, 'warn');
  assert.match(pill?.title ?? '', /github: disconnected/);
});

test('mcpContextPill omits empty inventory', () => {
  assert.equal(mcpContextPill(undefined), undefined);
  assert.equal(mcpContextPill([{ name: '   ' }]), undefined);
});
