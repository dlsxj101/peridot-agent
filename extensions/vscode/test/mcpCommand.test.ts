import test from 'node:test';
import assert from 'node:assert/strict';

import {
  mcpAddSlashCommand,
  mcpConfigChangingSlashCommand,
  mcpRemoveSlashCommand,
  mcpServersFromCommandResult,
  mcpServerChoices,
  mcpTestSlashCommand,
} from '../src/mcpCommand';

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

test('mcpAddSlashCommand builds parser-compatible add commands', () => {
  assert.equal(
    mcpAddSlashCommand(' local ', 'stdio', ' npx -y @modelcontextprotocol/server-filesystem . '),
    '/mcp add local stdio npx -y @modelcontextprotocol/server-filesystem .',
  );
  assert.equal(
    mcpAddSlashCommand('remote', 'http', ' https://example.com/mcp '),
    '/mcp add remote http https://example.com/mcp',
  );
  assert.throws(() => mcpAddSlashCommand('bad name', 'stdio', 'node server.js'), /whitespace/);
  assert.throws(() => mcpAddSlashCommand('local', 'stdio', '   '), /target/);
  assert.throws(() => mcpAddSlashCommand('local', 'stdio', 'node\nserver.js'), /single line/);
});

test('mcpConfigChangingSlashCommand detects add and remove only', () => {
  assert.equal(mcpConfigChangingSlashCommand('/mcp add local stdio node server.js'), true);
  assert.equal(mcpConfigChangingSlashCommand('  /mcp remove github  '), true);
  assert.equal(mcpConfigChangingSlashCommand('/mcp test github'), false);
  assert.equal(mcpConfigChangingSlashCommand('/mcp list'), false);
  assert.equal(mcpConfigChangingSlashCommand('/session list'), false);
});

test('mcpServersFromCommandResult normalizes inventory rows', () => {
  assert.deepEqual(
    mcpServersFromCommandResult({
      kind: 'mcp',
      items: [
        { label: ' github ', transport: 'http', tool_count: 3, connected: true },
        { label: 'local', transport: 'stdio', detail: 'node server.js' },
      ],
    }),
    [
      { name: 'github', transport: 'http', toolCount: 3, connected: true },
      { name: 'local', transport: 'stdio' },
    ],
  );
  assert.equal(mcpServersFromCommandResult({ kind: 'session_list', items: [] }), undefined);
});
