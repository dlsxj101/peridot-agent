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
    name: '/session new',
    description: 'create session',
    argHint: '[task]',
  },
  {
    name: '/session switch',
    description: 'switch session',
    argHint: '<id|title>',
  },
  {
    name: '/session close',
    description: 'close session',
    argHint: '<id|title>',
  },
  {
    name: '/session delete',
    description: 'delete session',
    argHint: '<id|title>',
  },
  {
    name: '/session rename',
    description: 'rename session',
    argHint: '<id|title> <new-title>',
  },
  {
    name: '/session save',
    description: 'save session',
  },
  {
    name: '/session list',
    description: 'list sessions',
  },
  {
    name: '/session count',
    description: 'count sessions',
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
    name: '/think',
    description: 'increase reasoning effort',
    argHint: '[off|low|medium|high|xhigh]',
    argOptions: ['off', 'low', 'medium', 'high', 'xhigh'],
  },
  {
    name: '/fast',
    description: 'toggle fast service tier',
    argHint: '[on|off|toggle]',
    argOptions: ['on', 'off', 'toggle'],
  },
  {
    name: '/autofix',
    description: 'toggle or configure autofix',
    argHint: '[on|off|<N>]',
    argOptions: ['on', 'off'],
  },
  {
    name: '/codemap',
    description: 'show code map',
    argHint: '[status|refresh|find <query>|locate <symbol>|outline <path>|refs <symbol>]',
    argOptions: ['status', 'refresh', 'find', 'locate', 'outline', 'refs'],
  },
  {
    name: '/branch turn',
    description: 'fork at a turn id',
    argHint: '<turn-id>',
  },
  {
    name: '/context',
    description: 'show largest context entries',
  },
  {
    name: '/context top',
    description: 'show largest context entries',
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

test('filteredSlashCommands includes context alias and context top', () => {
  const matches = filteredSlashCommands('/context', commands);

  assert.deepEqual(matches.map((command) => command.name), ['/context', '/context top']);
  assert.equal(slashExactSelectionIsRunnable('/context', commands, 0), true);
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
  assert.equal(codemap?.appendSpace, true);

  const locate = slashArgumentContext('/codemap locate', commands);
  assert.equal(locate?.command.name, '/codemap');
  assert.deepEqual(locate?.options, ['locate']);
  assert.equal(locate?.appendSpace, true);
  assert.equal(slashArgumentContext('/codemap locate ', commands), undefined);

  const mixedCodemap = slashArgumentContext('/codemap r', commands);
  assert.equal(mixedCodemap?.command.name, '/codemap');
  assert.deepEqual(mixedCodemap?.options, ['refresh', 'refs']);
  assert.equal(mixedCodemap?.appendSpace, undefined);

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

test('slashArgumentContext leaves query room after skills search', () => {
  const partial = slashArgumentContext('/skills se', commands);

  assert.equal(partial?.command.name, '/skills');
  assert.deepEqual(partial?.options, ['search']);
  assert.equal(partial?.appendSpace, true);

  const exact = slashArgumentContext('/skills search', commands);
  assert.equal(exact?.command.name, '/skills');
  assert.deepEqual(exact?.options, ['search']);
  assert.equal(exact?.appendSpace, true);
  assert.equal(slashArgumentContext('/skills search ', commands), undefined);
});

test('slashArgumentContext leaves name room after skills management subcommands', () => {
  const partial = slashArgumentContext('/skills sh', commands);

  assert.equal(partial?.command.name, '/skills');
  assert.deepEqual(partial?.options, ['show']);
  assert.equal(partial?.appendSpace, true);

  const exact = slashArgumentContext('/skills rest', commands);
  assert.equal(exact?.command.name, '/skills');
  assert.deepEqual(exact?.options, ['restore']);
  assert.equal(exact?.appendSpace, true);

  const skillPicker = slashArgumentContext('/skills restore', commands);
  assert.equal(skillPicker?.command.name, '/skills restore');
  assert.deepEqual(skillPicker?.options, ['old-parser']);
  assert.deepEqual(slashArgumentContext('/skills restore ', commands)?.options, ['old-parser']);
  assert.equal(slashArgumentContext('/skills list', commands), undefined);
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

test('slashArgumentContext leaves argument room after session subcommands', () => {
  const partial = slashArgumentContext('/session sw', commands);

  assert.equal(partial?.command.name, '/session');
  assert.deepEqual(partial?.options, ['switch']);
  assert.equal(partial?.appendSpace, true);

  const rename = slashArgumentContext('/session ren', commands);
  assert.equal(rename?.command.name, '/session');
  assert.deepEqual(rename?.options, ['rename']);
  assert.equal(rename?.appendSpace, true);

  assert.equal(slashArgumentContext('/session rename ', commands), undefined);
  assert.equal(slashArgumentContext('/session s', commands), undefined);
  assert.deepEqual(
    filteredSlashCommands('/session s', commands).map((command) => command.name),
    ['/session save', '/session switch'],
  );
  assert.equal(slashArgumentContext('/session save', commands), undefined);
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

test('slashArgumentContext leaves argument room after branch subcommands', () => {
  const turn = slashArgumentContext('/branch tu', commands);

  assert.equal(turn?.command.name, '/branch');
  assert.deepEqual(turn?.options, ['turn']);
  assert.equal(turn?.appendSpace, true);

  const exact = slashArgumentContext('/branch switch', commands);
  assert.equal(exact?.command.name, '/branch');
  assert.deepEqual(exact?.options, ['switch']);
  assert.equal(exact?.appendSpace, true);
  assert.equal(slashArgumentContext('/branch switch ', commands), undefined);
  assert.equal(slashArgumentContext('/branch tree', commands), undefined);
});

test('slashArgumentContext filters goal and notes subcommands', () => {
  assert.equal(slashArgumentContext('/goal', commands), undefined);
  const goal = slashArgumentContext('/goal p', commands);

  assert.equal(goal?.command.name, '/goal');
  assert.deepEqual(goal?.options, ['pause']);
  assert.equal(goal?.appendSpace, false);
  assert.deepEqual(
    slashArgumentContext('/goal ', commands)?.options,
    ['pause', 'resume', 'clear', 'status'],
  );
  assert.equal(slashArgumentContext('/goal pause', commands), undefined);
  assert.equal(slashArgumentContext('/goal fix tests', commands), undefined);

  assert.equal(slashArgumentContext('/notes', commands), undefined);
  const notes = slashArgumentContext('/notes l', commands);

  assert.equal(notes?.command.name, '/notes');
  assert.deepEqual(notes?.options, ['last']);
  assert.equal(notes?.appendSpace, true);
  assert.deepEqual(slashArgumentContext('/notes last', commands)?.options, ['last']);
  assert.equal(slashArgumentContext('/notes last ', commands), undefined);
});

test('slashArgumentContext filters think alias arguments', () => {
  assert.equal(slashArgumentContext('/think', commands), undefined);

  const hard = slashArgumentContext('/think h', commands);
  assert.equal(hard?.command.name, '/think');
  assert.deepEqual(hard?.options, ['hard', 'harder', 'high']);
  assert.equal(hard?.appendSpace, false);

  assert.deepEqual(slashArgumentContext('/think st', commands)?.options, ['stop']);
  assert.equal(slashArgumentContext('/think hard', commands), undefined);
  assert.equal(slashArgumentContext('/think fix tests', commands), undefined);
});

test('slashArgumentContext filters fast and autofix alias arguments', () => {
  assert.equal(slashArgumentContext('/fast', commands), undefined);
  assert.deepEqual(slashArgumentContext('/fast st', commands)?.options, ['standard']);
  assert.equal(slashArgumentContext('/fast standard', commands), undefined);
  assert.deepEqual(slashArgumentContext('/fast t', commands)?.options, ['toggle', 'true']);

  assert.equal(slashArgumentContext('/autofix', commands), undefined);
  assert.deepEqual(slashArgumentContext('/autofix f', commands)?.options, ['false']);
  assert.equal(slashArgumentContext('/autofix false', commands), undefined);
  assert.equal(slashArgumentContext('/autofix 5', commands), undefined);
});

test('slashArgumentContext filters export artifact arguments across tokens', () => {
  assert.equal(slashArgumentContext('/export', commands), undefined);
  const first = slashArgumentContext('/export a', commands);

  assert.equal(first?.command.name, '/export');
  assert.deepEqual(first?.options, ['attachments']);
  assert.equal(first?.appendSpace, true);
  assert.equal(slashArgumentContext('/export attachments', commands), undefined);

  const remaining = slashArgumentContext('/export attachments ', commands);
  assert.equal(remaining?.command.name, '/export attachments');
  assert.deepEqual(remaining?.options, ['notes', 'timeline', 'full']);
  assert.equal(remaining?.appendSpace, true);

  const filtered = slashArgumentContext('/export attachments n', commands);
  assert.equal(filtered?.command.name, '/export attachments');
  assert.deepEqual(filtered?.options, ['notes']);
  assert.equal(slashArgumentContext('/export attachments bad', commands), undefined);
});

test('slashExactSelectionIsRunnable allows optional-arg exact commands only', () => {
  assert.equal(slashExactSelectionIsRunnable('/skills', commands, 0), true);
  assert.equal(slashExactSelectionIsRunnable('/reasoning', commands, 0), false);
});

test('slashPickerItemCount uses argument options when an argument picker is open', () => {
  assert.equal(slashPickerItemCount('/reasoning ', commands), 5);
  assert.equal(slashPickerItemCount('/skills se', commands), 1);
  assert.equal(slashPickerItemCount('/skills sh', commands), 1);
  assert.equal(slashPickerItemCount('/session sw', commands), 1);
  assert.equal(slashPickerItemCount('/session switch ', commands, sessions), 2);
  assert.equal(slashPickerItemCount('/mcp test ', commands, [], mcpServers), 2);
  assert.equal(slashPickerItemCount('/model ', commands, [], [], modelSuggestions), 2);
  assert.equal(slashPickerItemCount('/branch restore ', commands, [], [], [], branchSnapshots), 2);
  assert.equal(slashPickerItemCount('/branch tu', commands), 1);
  assert.equal(slashPickerItemCount('/codemap loc', commands), 1);
  assert.equal(slashPickerItemCount('/goal ', commands), 4);
  assert.equal(slashPickerItemCount('/notes l', commands), 1);
  assert.equal(slashPickerItemCount('/think h', commands), 3);
  assert.equal(slashPickerItemCount('/fast t', commands), 2);
  assert.equal(slashPickerItemCount('/autofix f', commands), 1);
  assert.equal(slashPickerItemCount('/export attachments ', commands), 3);
});
