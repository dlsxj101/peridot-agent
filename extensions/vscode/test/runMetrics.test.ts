import test from 'node:test';
import assert from 'node:assert/strict';

import { runMetricChips } from '../webview/runMetrics';

test('runMetricChips summarizes executor tokens and aggregate cost', () => {
  const chips = runMetricChips({
    usage: {
      inputTokens: 1200,
      outputTokens: 300,
      cacheReadTokens: 500,
      costUsd: 0.0123,
    },
    committee: {
      planner: { tokens: 200, costUsd: 0.003 },
      reviewer: { tokens: 100, costUsd: 0.001 },
    },
  });

  assert.equal(chips[0]?.label, 'Tokens');
  assert.equal(chips[0]?.value, '2K');
  assert.equal(chips[1]?.label, 'Cost');
  assert.equal(chips[1]?.value, '$0.016');
  assert.match(chips[1]?.title ?? '', /committee \$0\.0040/);
});

test('runMetricChips marks budget and turn pressure', () => {
  const chips = runMetricChips({
    budget: {
      costUsed: 0.92,
      costLimit: 1,
      turnsUsed: 8,
      turnsLimit: 10,
    },
  });

  assert.deepEqual(
    chips.map((chip) => [chip.label, chip.value, chip.tone]),
    [
      ['Budget', '92%', 'critical'],
      ['Turns', '8/10', 'warn'],
    ],
  );
});

test('runMetricChips summarizes aggregate session usage', () => {
  const chips = runMetricChips(
    {
      usage: {
        inputTokens: 400,
        outputTokens: 100,
        costUsd: 0.01,
      },
    },
    [
      {
        id: 'active',
        title: 'Active',
        status: 'running',
        running: true,
        active: true,
        total_tokens: 300,
        total_cost_usd: 0.005,
      },
      {
        id: 'done',
        title: 'Done',
        status: 'done',
        running: false,
        active: false,
        total_tokens: 4500,
        total_cost_usd: 0.09,
      },
    ],
  );

  const aggregate = chips.find((chip) => chip.label === 'All');
  assert.equal(aggregate?.value, '$0.100');
  assert.equal(aggregate?.tone, 'muted');
  assert.match(aggregate?.title ?? '', /5,000 tokens/);
  assert.match(aggregate?.title ?? '', /2 sessions/);
});

test('runMetricChips omits empty hud metrics', () => {
  assert.deepEqual(runMetricChips({}), []);
});
