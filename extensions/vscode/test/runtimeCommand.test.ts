import test from 'node:test';
import assert from 'node:assert/strict';

import {
  committeeSlashCommand,
  executionModeSlashCommand,
  modelSlashCommand,
  permissionSlashCommand,
  providerSlashCommand,
  reasoningSlashCommand,
} from '../src/runtimeCommand';

test('runtime command builders produce shared slash commands', () => {
  assert.equal(executionModeSlashCommand('plan'), '/plan');
  assert.equal(permissionSlashCommand('safe'), '/safe');
  assert.equal(reasoningSlashCommand('xhigh'), '/reasoning xhigh');
  assert.equal(providerSlashCommand(' openai-oauth '), '/provider openai-oauth');
  assert.equal(modelSlashCommand(' gpt-5.5 '), '/model gpt-5.5');
  assert.equal(committeeSlashCommand('full'), '/committee full');
});
