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
];

test('filteredSlashCommands ranks prefixes before description matches', () => {
  const matches = filteredSlashCommands('/switch', commands);

  assert.equal(matches[0]?.name, '/session switch');
});

test('filteredSlashCommands includes dynamic skill slash commands', () => {
  const matches = filteredSlashCommands('/auto-f', commands);

  assert.deepEqual(matches.map((command) => command.name), ['/auto-fix-parser']);
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
});

test('slashArgumentContext filters skill-name arguments', () => {
  const context = slashArgumentContext('/skills show auto', commands);

  assert.equal(context?.command.name, '/skills show');
  assert.deepEqual(context?.options, ['auto-fix-parser']);
  assert.equal(slashArgumentContext('/skills use /auto-fix-parser', commands), undefined);
});

test('slashExactSelectionIsRunnable allows optional-arg exact commands only', () => {
  assert.equal(slashExactSelectionIsRunnable('/skills', commands, 0), true);
  assert.equal(slashExactSelectionIsRunnable('/reasoning', commands, 0), false);
});

test('slashPickerItemCount uses argument options when an argument picker is open', () => {
  assert.equal(slashPickerItemCount('/reasoning ', commands), 5);
});
