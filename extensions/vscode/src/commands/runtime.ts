// Runtime-control command handlers for the extension host.
//
// QuickPick / input-box pickers that adjust how the *next* Peridot task runs —
// execution mode, permission policy, reasoning effort, provider, model,
// committee mode — plus the context-management one-shots (compact / rewind /
// undo). Split out of `extension.ts`; each handler drives the matching daemon
// slash command through the private runSharedSlashCommand helper, which focuses
// the chat view and forwards to the sidebar.

import * as vscode from 'vscode';

import {
  COMMITTEE_CHOICES,
  EXECUTION_MODE_CHOICES,
  PERMISSION_CHOICES,
  PROVIDER_CHOICES,
  REASONING_CHOICES,
  committeeSlashCommand,
  executionModeSlashCommand,
  modelSlashCommand,
  permissionSlashCommand,
  providerSlashCommand,
  reasoningSlashCommand,
} from '../runtimeCommand';
import type { PeridotSidebarProvider } from '../sidebar';

export async function setExecutionMode(sidebar: PeridotSidebarProvider): Promise<void> {
  const current = sidebar.currentRunOptions().mode;
  const picked = await vscode.window.showQuickPick(
    EXECUTION_MODE_CHOICES.map((choice) => ({
      ...choice,
      picked: choice.mode === current,
    })),
    {
      title: 'Peridot: Set Execution Mode',
      placeHolder: 'Choose how the next Peridot task should run',
      ignoreFocusOut: true,
    },
  );
  if (!picked) return;
  await runSharedSlashCommand(executionModeSlashCommand(picked.mode), sidebar);
}

export async function setPermissionMode(sidebar: PeridotSidebarProvider): Promise<void> {
  const current = sidebar.currentRunOptions().permission;
  const picked = await vscode.window.showQuickPick(
    PERMISSION_CHOICES.map((choice) => ({
      ...choice,
      picked: choice.permission === current,
    })),
    {
      title: 'Peridot: Set Permission Mode',
      placeHolder: 'Choose the approval policy for future tool calls',
      ignoreFocusOut: true,
    },
  );
  if (!picked) return;
  await runSharedSlashCommand(permissionSlashCommand(picked.permission), sidebar);
}

export async function setReasoningEffort(sidebar: PeridotSidebarProvider): Promise<void> {
  const current = sidebar.currentRunOptions().reasoningEffort ?? sidebar.currentContext().reasoningEffort;
  const picked = await vscode.window.showQuickPick(
    REASONING_CHOICES.map((choice) => ({
      ...choice,
      picked: choice.effort === current,
    })),
    {
      title: 'Peridot: Set Reasoning Effort',
      placeHolder: 'Choose reasoning effort for future model calls',
      ignoreFocusOut: true,
    },
  );
  if (!picked) return;
  await runSharedSlashCommand(reasoningSlashCommand(picked.effort), sidebar);
}

export async function switchRuntimeProvider(sidebar: PeridotSidebarProvider): Promise<void> {
  const current = sidebar.currentContext().provider;
  const picked = await vscode.window.showQuickPick(
    PROVIDER_CHOICES.map((choice) => ({
      ...choice,
      picked: choice.provider === current,
    })),
    {
      title: 'Peridot: Switch Runtime Provider',
      placeHolder: 'Choose the provider for this session',
      ignoreFocusOut: true,
    },
  );
  if (!picked) return;
  await runSharedSlashCommand(providerSlashCommand(picked.provider), sidebar);
}

export async function setRuntimeModel(sidebar: PeridotSidebarProvider): Promise<void> {
  const context = sidebar.currentContext();
  const current = sidebar.currentRunOptions().model ?? context.model ?? '';
  const suggestions = context.modelSuggestions?.filter((model) => model.trim().length > 0) ?? [];
  const model = await vscode.window.showInputBox({
    title: 'Peridot: Set Runtime Model',
    prompt: 'Model override for this Peridot session.',
    value: current,
    placeHolder: suggestions.length > 0 ? suggestions.join(', ') : 'model name',
    ignoreFocusOut: true,
    validateInput: (value) =>
      value.trim().length === 0 ? 'Enter a model name for /model.' : undefined,
  });
  if (model === undefined) return;
  await runSharedSlashCommand(modelSlashCommand(model), sidebar);
}

export async function setCommitteeMode(sidebar: PeridotSidebarProvider): Promise<void> {
  const current = sidebar.currentContext().committeeMode ?? 'off';
  const picked = await vscode.window.showQuickPick(
    COMMITTEE_CHOICES.map((choice) => ({
      ...choice,
      picked: choice.mode === current,
    })),
    {
      title: 'Peridot: Set Committee Mode',
      placeHolder: 'Choose whether planner/reviewer roles wrap the executor',
      ignoreFocusOut: true,
    },
  );
  if (!picked) return;
  await runSharedSlashCommand(committeeSlashCommand(picked.mode), sidebar);
}

export async function compactContext(sidebar: PeridotSidebarProvider): Promise<void> {
  await runSharedSlashCommand('/compact', sidebar);
}

export async function rewindSession(sidebar: PeridotSidebarProvider): Promise<void> {
  await runSharedSlashCommand('/rewind', sidebar);
}

export async function undoLastChange(sidebar: PeridotSidebarProvider): Promise<void> {
  const confirmation = await vscode.window.showWarningMessage(
    vscode.l10n.t('Undo the latest Peridot file checkpoint in this workspace?'),
    { modal: true },
    'Undo',
  );
  if (confirmation !== 'Undo') return;
  await runSharedSlashCommand('/undo', sidebar);
}

async function runSharedSlashCommand(
  command: string,
  sidebar: PeridotSidebarProvider,
): Promise<void> {
  if (!vscode.workspace.workspaceFolders?.[0]?.uri.fsPath) {
    const message = 'Open a workspace folder before running Peridot session commands.';
    sidebar.setWorkspaceProblem(message);
    await vscode.window.showWarningMessage(message);
    return;
  }
  await vscode.commands.executeCommand('peridot.chatView.focus');
  await sidebar.executeSlashCommand(command);
}
