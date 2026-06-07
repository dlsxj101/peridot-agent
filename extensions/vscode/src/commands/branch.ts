// `/branch` command handlers for the extension host.
//
// Show / save / restore / fork / switch context branch snapshots by driving
// the corresponding `/branch …` daemon slash command and rendering the result
// into the sidebar transcript. Split out of `extension.ts`; the public handlers
// take the shared output channel + sidebar and reuse the host's runSlashCommand
// / refreshStatus helpers. The runBranchCommand / pickBranchSnapshot /
// ensureWorkspaceForBranch helpers stay private to this module.

import * as vscode from 'vscode';

import {
  branchListSlashCommand,
  branchPickerSlashCommand,
  branchRestoreSlashCommand,
  branchSaveSlashCommand,
  branchSnapshotChoices,
  branchSwitchSlashCommand,
  branchTreeSlashCommand,
  branchTurnSlashCommand,
  parseBranchSwitchInput,
  parseBranchTurnInput,
} from '../branchCommand';
import { refreshStatus, runSlashCommand } from '../extension';
import type { PeridotSidebarProvider } from '../sidebar';

export async function showBranchTurns(
  output: vscode.OutputChannel,
  sidebar: PeridotSidebarProvider,
): Promise<void> {
  await runBranchCommand(branchPickerSlashCommand(), output, sidebar, 'branch picker');
}

export async function showBranchSnapshots(
  output: vscode.OutputChannel,
  sidebar: PeridotSidebarProvider,
): Promise<void> {
  await runBranchCommand(branchListSlashCommand(), output, sidebar, 'branch list');
}

export async function saveBranchSnapshot(
  output: vscode.OutputChannel,
  sidebar: PeridotSidebarProvider,
): Promise<void> {
  if (!(await ensureWorkspaceForBranch(sidebar))) return;
  const name = await vscode.window.showInputBox({
    title: 'Peridot: Save Branch Snapshot',
    prompt: 'Save the current session context under a reusable snapshot name.',
    placeHolder: 'parser_checkpoint',
    ignoreFocusOut: true,
    validateInput: (value) => {
      try {
        branchSaveSlashCommand(value);
        return undefined;
      } catch (err) {
        return err instanceof Error ? err.message : String(err);
      }
    },
  });
  if (name === undefined) return;
  let command: string;
  try {
    command = branchSaveSlashCommand(name);
  } catch (err) {
    const message = err instanceof Error ? err.message : String(err);
    await vscode.window.showErrorMessage(vscode.l10n.t('Peridot branch save failed: {0}', message));
    return;
  }
  await runBranchCommand(command, output, sidebar, 'branch save', { refreshStatus: true });
}

export async function restoreBranchSnapshot(
  output: vscode.OutputChannel,
  sidebar: PeridotSidebarProvider,
): Promise<void> {
  const choice = await pickBranchSnapshot(output, sidebar);
  if (!choice) return;
  let command: string;
  try {
    command = branchRestoreSlashCommand(choice.name);
  } catch (err) {
    const message = err instanceof Error ? err.message : String(err);
    await vscode.window.showErrorMessage(vscode.l10n.t('Peridot branch restore failed: {0}', message));
    return;
  }
  await runBranchCommand(command, output, sidebar, 'branch restore');
}

export async function forkBranchAtTurn(
  output: vscode.OutputChannel,
  sidebar: PeridotSidebarProvider,
): Promise<void> {
  if (!(await ensureWorkspaceForBranch(sidebar))) return;
  const input = await vscode.window.showInputBox({
    title: 'Peridot: Fork Branch at Turn',
    prompt: 'Enter a past context turn id to fork from.',
    placeHolder: '12',
    ignoreFocusOut: true,
    validateInput: (value) => {
      try {
        parseBranchTurnInput(value);
        return undefined;
      } catch (err) {
        return err instanceof Error ? err.message : String(err);
      }
    },
  });
  if (input === undefined) return;
  let command: string;
  try {
    command = branchTurnSlashCommand(parseBranchTurnInput(input));
  } catch (err) {
    const message = err instanceof Error ? err.message : String(err);
    await vscode.window.showErrorMessage(vscode.l10n.t('Peridot branch fork failed: {0}', message));
    return;
  }
  await runBranchCommand(command, output, sidebar, 'branch fork');
}

export async function showBranchTree(
  output: vscode.OutputChannel,
  sidebar: PeridotSidebarProvider,
): Promise<void> {
  await runBranchCommand(branchTreeSlashCommand(), output, sidebar, 'branch tree');
}

export async function switchBranchLimb(
  output: vscode.OutputChannel,
  sidebar: PeridotSidebarProvider,
): Promise<void> {
  if (!(await ensureWorkspaceForBranch(sidebar))) return;
  const input = await vscode.window.showInputBox({
    title: 'Peridot: Switch Branch Limb',
    prompt: 'Enter the branch limb index shown by /branch tree.',
    placeHolder: '1',
    ignoreFocusOut: true,
    validateInput: (value) => {
      try {
        parseBranchSwitchInput(value);
        return undefined;
      } catch (err) {
        return err instanceof Error ? err.message : String(err);
      }
    },
  });
  if (input === undefined) return;
  let command: string;
  try {
    command = branchSwitchSlashCommand(parseBranchSwitchInput(input));
  } catch (err) {
    const message = err instanceof Error ? err.message : String(err);
    await vscode.window.showErrorMessage(vscode.l10n.t('Peridot branch switch failed: {0}', message));
    return;
  }
  await runBranchCommand(command, output, sidebar, 'branch switch');
}

async function runBranchCommand(
  command: string,
  output: vscode.OutputChannel,
  sidebar: PeridotSidebarProvider,
  label: string,
  options: { refreshStatus?: boolean } = {},
): Promise<void> {
  if (!(await ensureWorkspaceForBranch(sidebar))) return;
  await vscode.commands.executeCommand('peridot.chatView.focus');
  try {
    const result = await runSlashCommand(command, output, sidebar, sidebar.currentRunOptions());
    sidebar.appendCommandResult(result);
    if (options.refreshStatus) await refreshStatus(output, sidebar, { force: true });
  } catch (err) {
    const message = err instanceof Error ? err.message : String(err);
    output.appendLine(`[peridot] ${label} failed: ${message}`);
    sidebar.appendError(message);
    await vscode.window.showErrorMessage(vscode.l10n.t('Peridot {0} failed: {1}', label, message));
  }
}

async function pickBranchSnapshot(
  output: vscode.OutputChannel,
  sidebar: PeridotSidebarProvider,
): Promise<{ name: string } | undefined> {
  if (!(await ensureWorkspaceForBranch(sidebar))) return undefined;
  let choices = branchSnapshotChoices(sidebar.currentBranchSnapshots());
  if (choices.length === 0) {
    await refreshStatus(output, sidebar, { force: true });
    choices = branchSnapshotChoices(sidebar.currentBranchSnapshots());
  }
  if (choices.length === 0) {
    await vscode.window.showWarningMessage(vscode.l10n.t('No branch snapshots are saved for this workspace.'));
    return undefined;
  }
  if (choices.length === 1) return { name: choices[0].name };
  return vscode.window.showQuickPick(
    choices.map((choice) => ({ label: choice.label, name: choice.name })),
    {
      title: 'Peridot: Restore Branch Snapshot',
      placeHolder: 'Choose a saved branch snapshot',
      ignoreFocusOut: true,
    },
  );
}

async function ensureWorkspaceForBranch(sidebar: PeridotSidebarProvider): Promise<boolean> {
  if (vscode.workspace.workspaceFolders?.[0]?.uri.fsPath) return true;
  const message = 'Open a workspace folder before using branch commands.';
  sidebar.setWorkspaceProblem(message);
  await vscode.window.showWarningMessage(message);
  return false;
}
