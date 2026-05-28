import test from 'node:test';
import assert from 'node:assert/strict';

import { mcpRemoveSlashCommand, mcpServerChoices, mcpTestSlashCommand } from '../src/mcpCommand';

test('mcpServerChoices lists configured servers without duplicates', () => {
  assert.deepEqual(
    mcpServerChoices([
      { name: 'github', transport: 'http', toolCount: 3, connected: true },
      { name: 'filesystem', transport: 'stdio' },
      { name: 'github', transport: 'stdio' },
      { name: '   ' },
    ]),
    [
      { name: 'filesystem', label: 'filesystem', description: 'stdio' },
      { name: 'github', label: 'github', description: 'http - 3 tool(s) - connected' },
    ],
  );
});

test('mcpTestSlashCommand builds parser-compatible test commands', () => {
  assert.equal(mcpTestSlashCommand(' github '), '/mcp test github');
  assert.throws(() => mcpTestSlashCommand('   '), /MCP server name/);
  assert.throws(() => mcpTestSlashCommand('bad name'), /whitespace/);
});

test('mcpRemoveSlashCommand builds parser-compatible remove commands', () => {
  assert.equal(mcpRemoveSlashCommand(' filesystem '), '/mcp remove filesystem');
  assert.throws(() => mcpRemoveSlashCommand('   '), /MCP server name/);
  assert.throws(() => mcpRemoveSlashCommand('bad name'), /whitespace/);
});
