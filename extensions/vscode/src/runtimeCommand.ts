import type { Permission, ReasoningEffort, RunOptions } from './types';

export type ExecutionModeChoice = RunOptions['mode'];
export type CommitteeModeChoice = 'off' | 'planner' | 'full';

export const EXECUTION_MODE_CHOICES: Array<{
  label: string;
  description: string;
  mode: ExecutionModeChoice;
}> = [
  { label: 'Execute', description: 'Run coding tasks with tools enabled', mode: 'execute' },
  { label: 'Plan', description: 'Ask for an implementation plan without edits', mode: 'plan' },
  { label: 'Goal', description: 'Track progress against a goal objective', mode: 'goal' },
];

export const PERMISSION_CHOICES: Array<{
  label: string;
  description: string;
  permission: Permission;
}> = [
  { label: 'Auto', description: 'Ask before risky or mutating actions', permission: 'auto' },
  { label: 'Safe', description: 'Read-only mode unless explicit approval is granted', permission: 'safe' },
  { label: 'Yolo', description: 'Allow actions without interactive approval', permission: 'yolo' },
];

export const REASONING_CHOICES: Array<{
  label: string;
  description: string;
  effort: ReasoningEffort;
}> = [
  { label: 'Off', description: 'Disable reasoning effort', effort: 'off' },
  { label: 'Low', description: 'Use light reasoning', effort: 'low' },
  { label: 'Medium', description: 'Use balanced reasoning', effort: 'medium' },
  { label: 'High', description: 'Use deeper reasoning', effort: 'high' },
  { label: 'XHigh', description: 'Use maximum reasoning effort', effort: 'xhigh' },
];

export const PROVIDER_CHOICES: Array<{
  label: string;
  description: string;
  provider: string;
}> = [
  { label: 'ChatGPT OAuth', description: 'openai-oauth', provider: 'openai-oauth' },
  { label: 'OpenAI API', description: 'openai-api', provider: 'openai-api' },
  { label: 'OpenRouter API', description: 'openrouter-api', provider: 'openrouter-api' },
  { label: 'Claude API', description: 'claude-api', provider: 'claude-api' },
];

export const COMMITTEE_CHOICES: Array<{
  label: string;
  description: string;
  mode: CommitteeModeChoice;
}> = [
  { label: 'Off', description: 'Single executor agent', mode: 'off' },
  { label: 'Planner', description: 'Planner preflight, then executor', mode: 'planner' },
  { label: 'Full', description: 'Planner, executor, and reviewer', mode: 'full' },
];

export function executionModeSlashCommand(mode: ExecutionModeChoice): string {
  return `/${mode}`;
}

export function permissionSlashCommand(permission: Permission): string {
  return `/${permission}`;
}

export function reasoningSlashCommand(effort: ReasoningEffort): string {
  return `/reasoning ${effort}`;
}

export function providerSlashCommand(provider: string): string {
  return `/provider ${provider.trim()}`;
}

export function modelSlashCommand(model: string): string {
  return `/model ${model.trim()}`;
}

export function committeeSlashCommand(mode: CommitteeModeChoice): string {
  return `/committee ${mode}`;
}
