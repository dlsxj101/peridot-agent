import test from 'node:test';
import assert from 'node:assert/strict';

import type { SlashCommandSpec } from '../src/types';
import {
  filteredSlashCommands,
  slashArgumentContext,
  slashArgumentOptions,
  slashExactSelectionIsRunnable,
  slashPickerItemCount,
} from '../webview/slashAutocomplete';

const commands: SlashCommandSpec[] = [
  {
    name: '/plan',
    description: 'switch to plan mode',
  },
  {
    name: '/session switch',
    description: 'switch session',
    argHint: '<id|title>',
  },
  {
    name: '/reasoning',
    description: 'set reasoning effort',
    argHint: '<off|low|medium|high|xhigh>',
    argOptions: ['off', 'low', 'medium', 'high', 'xhigh'],
  },
  {
    name: '/provider',
    description: 'switch provider',
    argHint: '<claude-api|openai-api|openrouter-api|openai-oauth>',
    argOptions: ['claude-api', 'openai-api', 'openrouter-api', 'openai-oauth'],
  },
  {
    name: '/codemap',
    description: 'show code map',
    argHint: '[status|refresh|find <query>|locate <symbol>|outline <path>|refs <symbol>]',
    argOptions: ['status', 'refresh', 'find', 'locate', 'outline', 'refs'],
  },
  {
    name: '/skills',
    description: 'list stored skills',
  },
  {
    name: '/status',
    description: 'show local status',
  },
  {
    name: '/auto-fix-parser',
    description: 'repair parser tests',
    category: 'skill',
  },
  {
    name: '/old-parser',
    description: 'archived parser skill',
    category: 'skill',
    archived: true,
  },
];

const sessions = [
  { id: 's-1', title: 'parser cleanup' },
  { id: 's-2', title: 'release prep' },
];

const mcpServers = [{ name: 'filesystem' }, { name: 'github' }];
const modelSuggestions = ['claude-sonnet-4-6', 'gpt-5.1-codex'];
const branchSnapshots = ['parser-snapshot', 'release-branch'];

test('filteredSlashCommands ranks prefixes before description matches', () => {
  const matches = filteredSlashCommands('/switch', commands);

  assert.equal(matches[0]?.name, '/session switch');
});

test('filteredSlashCommands includes dynamic skill slash commands', () => {
  const matches = filteredSlashCommands('/auto-f', commands);

  assert.deepEqual(matches.map((command) => command.name), ['/auto-fix-parser']);
  assert.deepEqual(filteredSlashCommands('/old', commands).map((command) => command.name), []);
});

test('filteredSlashCommands includes status alias commands', () => {
  const matches = filteredSlashCommands('/sta', commands);

  assert.deepEqual(matches.map((command) => command.name), ['/status']);
});

test('slashArgumentOptions prefers structured argOptions over placeholder hints', () => {
  const reasoning = commands.find((command) => command.name === '/reasoning');
  assert.ok(reasoning);

  assert.deepEqual(slashArgumentOptions(reasoning), ['off', 'low', 'medium', 'high', 'xhigh']);
});

test('slashArgumentOptions drops placeholder-only hint arms', () => {
  const sessionSwitch = commands.find((command) => command.name === '/session switch');
  assert.ok(sessionSwitch);

  assert.deepEqual(slashArgumentOptions(sessionSwitch), []);
});

test('slashArgumentContext filters finite options and closes after exact option', () => {
  const context = slashArgumentContext('/reasoning x', commands);

  assert.equal(context?.command.name, '/reasoning');
  assert.deepEqual(context?.options, ['xhigh']);
  assert.equal(slashArgumentContext('/reasoning xhigh', commands), undefined);

  const providers = slashArgumentContext('/provider open', commands);
  assert.equal(providers?.command.name, '/provider');
  assert.deepEqual(providers?.options, ['openai-api', 'openrouter-api', 'openai-oauth']);
  assert.equal(slashArgumentContext('/provider openai-oauth', commands), undefined);

  const codemap = slashArgumentContext('/codemap l', commands);
  assert.equal(codemap?.command.name, '/codemap');
  assert.deepEqual(codemap?.options, ['locate']);
  assert.equal(slashArgumentContext('/codemap locate', commands), undefined);

  assert.equal(slashArgumentContext('/mcp add local', commands), undefined);
  const mcpTransport = slashArgumentContext('/mcp add local h', commands);
  assert.equal(mcpTransport?.command.name, '/mcp add local');
  assert.deepEqual(mcpTransport?.options, ['http']);
  assert.equal(mcpTransport?.appendSpace, true);
  assert.equal(slashArgumentContext('/mcp add local http', commands), undefined);
  assert.equal(slashArgumentContext('/mcp add local http http://localhost', commands), undefined);
});

test('slashArgumentContext filters skill-name arguments', () => {
  const context = slashArgumentContext('/skills show auto', commands);

  assert.equal(context?.command.name, '/skills show');
  assert.deepEqual(context?.options, ['auto-fix-parser']);
  assert.equal(slashArgumentContext('/skills use /auto-fix-parser', commands), undefined);
  assert.deepEqual(slashArgumentContext('/skills restore old', commands)?.options, ['old-parser']);
  assert.equal(slashArgumentContext('/skills restore auto', commands), undefined);
  assert.equal(slashArgumentContext('/skills archive old', commands), undefined);
});

test('slashArgumentContext filters session target arguments', () => {
  const context = slashArgumentContext('/session switch release', commands, sessions);

  assert.equal(context?.command.name, '/session switch');
  assert.deepEqual(context?.options, ['s-2']);
  assert.equal(context?.appendSpace, false);
  assert.equal(slashArgumentContext('/session switch s-2', commands, sessions), undefined);

  const rename = slashArgumentContext('/session rename parser', commands, sessions);
  assert.equal(rename?.command.name, '/session rename');
  assert.deepEqual(rename?.options, ['s-1']);
  assert.equal(rename?.appendSpace, true);
  assert.equal(slashArgumentContext('/session rename s-1 new title', commands, sessions), undefined);
});

test('slashArgumentContext filters mcp server arguments', () => {
  const context = slashArgumentContext('/mcp test g', commands, [], mcpServers);

  assert.equal(context?.command.name, '/mcp test');
  assert.deepEqual(context?.options, ['github']);
  assert.equal(context?.appendSpace, undefined);
  assert.equal(slashArgumentContext('/mcp test github', commands, [], mcpServers), undefined);
  assert.equal(
    slashArgumentContext('/mcp remove github extra', commands, [], mcpServers),
    undefined,
  );
  assert.deepEqual(
    slashArgumentContext('/mcp remove ', commands, [], mcpServers)?.options,
    ['filesystem', 'github'],
  );
});

test('slashArgumentContext filters model-name arguments', () => {
  const context = slashArgumentContext('/model g', commands, [], [], modelSuggestions);

  assert.equal(context?.command.name, '/model');
  assert.deepEqual(context?.options, ['gpt-5.1-codex']);
  assert.equal(context?.appendSpace, undefined);
  assert.equal(
    slashArgumentContext('/model gpt-5.1-codex', commands, [], [], modelSuggestions),
    undefined,
  );
  assert.deepEqual(
    slashArgumentContext('/subagent model ', commands, [], [], modelSuggestions)?.options,
    ['claude-sonnet-4-6', 'gpt-5.1-codex', 'reset'],
  );
});

test('slashArgumentContext filters branch snapshot arguments', () => {
  const context = slashArgumentContext(
    '/branch restore rel',
    commands,
    [],
    [],
    [],
    branchSnapshots,
  );

  assert.equal(context?.command.name, '/branch restore');
  assert.deepEqual(context?.options, ['release-branch']);
  assert.equal(context?.appendSpace, undefined);
  assert.equal(
    slashArgumentContext('/branch restore release-branch', commands, [], [], [], branchSnapshots),
    undefined,
  );
  assert.equal(
    slashArgumentContext('/branch restore release-branch extra', commands, [], [], [], branchSnapshots),
    undefined,
  );
  assert.deepEqual(
    slashArgumentContext('/branch restore ', commands, [], [], [], branchSnapshots)?.options,
    ['parser-snapshot', 'release-branch'],
  );
});

test('slashExactSelectionIsRunnable allows optional-arg exact commands only', () => {
  assert.equal(slashExactSelectionIsRunnable('/skills', commands, 0), true);
  assert.equal(slashExactSelectionIsRunnable('/reasoning', commands, 0), false);
});

test('slashPickerItemCount uses argument options when an argument picker is open', () => {
  assert.equal(slashPickerItemCount('/reasoning ', commands), 5);
  assert.equal(slashPickerItemCount('/session switch ', commands, sessions), 2);
  assert.equal(slashPickerItemCount('/mcp test ', commands, [], mcpServers), 2);
  assert.equal(slashPickerItemCount('/model ', commands, [], [], modelSuggestions), 2);
  assert.equal(slashPickerItemCount('/branch restore ', commands, [], [], [], branchSnapshots), 2);
});
