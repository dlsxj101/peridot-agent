// `/skill` command handlers for the extension host.
//
// Load / list / show / search / pin / archive / restore stored skills by
// driving the corresponding `/skills …` daemon slash command and rendering the
// result into the sidebar transcript. Split out of `extension.ts`; each handler
// takes the shared output channel + sidebar and reuses the host's
// `runSlashCommand` / `refreshSlashCatalog` execution helpers.

import * as vscode from 'vscode';

import { refreshSlashCatalog, runSlashCommand } from '../extension';
import type { PeridotSidebarProvider } from '../sidebar';

export async function showSkills(
  output: vscode.OutputChannel,
  sidebar: PeridotSidebarProvider,
): Promise<void> {
  const folder = vscode.workspace.workspaceFolders?.[0]?.uri.fsPath;
  if (!folder) {
    const message = 'Open a workspace folder before listing Peridot skills.';
    sidebar.setWorkspaceProblem(message);
    await vscode.window.showWarningMessage(message);
    return;
  }
  await vscode.commands.executeCommand('peridot.chatView.focus');
  try {
    const result = await vscode.window.withProgress(
      {
        location: vscode.ProgressLocation.Window,
        title: 'Peridot: loading skills',
      },
      async () => runSlashCommand('/skills', output, sidebar, sidebar.currentRunOptions()),
    );
    sidebar.appendCommandResult(result);
  } catch (err) {
    const message = err instanceof Error ? err.message : String(err);
    output.appendLine(`[peridot] skills failed: ${message}`);
    sidebar.appendError(message);
    await vscode.window.showErrorMessage(vscode.l10n.t('Peridot skills failed: {0}', message));
  }
}

export async function showArchivedSkills(
  output: vscode.OutputChannel,
  sidebar: PeridotSidebarProvider,
): Promise<void> {
  const folder = vscode.workspace.workspaceFolders?.[0]?.uri.fsPath;
  if (!folder) {
    const message = 'Open a workspace folder before listing archived Peridot skills.';
    sidebar.setWorkspaceProblem(message);
    await vscode.window.showWarningMessage(message);
    return;
  }
  await vscode.commands.executeCommand('peridot.chatView.focus');
  try {
    const result = await vscode.window.withProgress(
      {
        location: vscode.ProgressLocation.Window,
        title: 'Peridot: loading archived skills',
      },
      async () => runSlashCommand('/skills archived', output, sidebar, sidebar.currentRunOptions()),
    );
    sidebar.appendCommandResult(result);
  } catch (err) {
    const message = err instanceof Error ? err.message : String(err);
    output.appendLine(`[peridot] archived skills failed: ${message}`);
    sidebar.appendError(message);
    await vscode.window.showErrorMessage(
      vscode.l10n.t('Peridot archived skills failed: {0}', message),
    );
  }
}

export async function searchSkills(
  output: vscode.OutputChannel,
  sidebar: PeridotSidebarProvider,
): Promise<void> {
  const query = await vscode.window.showInputBox({
    title: 'Search Peridot Skills',
    prompt: 'Search active stored skills by name or body text',
    placeHolder: 'parser release rust',
    ignoreFocusOut: true,
  });
  const trimmed = query?.trim();
  if (!trimmed) return;
  const folder = vscode.workspace.workspaceFolders?.[0]?.uri.fsPath;
  if (!folder) {
    const message = 'Open a workspace folder before searching Peridot skills.';
    sidebar.setWorkspaceProblem(message);
    await vscode.window.showWarningMessage(message);
    return;
  }
  await vscode.commands.executeCommand('peridot.chatView.focus');
  try {
    const result = await runSlashCommand(
      `/skills search ${trimmed}`,
      output,
      sidebar,
      sidebar.currentRunOptions(),
    );
    sidebar.appendCommandResult(result);
  } catch (err) {
    const message = err instanceof Error ? err.message : String(err);
    output.appendLine(`[peridot] skills search failed: ${message}`);
    sidebar.appendError(message);
    await vscode.window.showErrorMessage(vscode.l10n.t('Peridot skill search failed: {0}', message));
  }
}

export async function searchArchivedSkills(
  output: vscode.OutputChannel,
  sidebar: PeridotSidebarProvider,
): Promise<void> {
  const query = await vscode.window.showInputBox({
    title: 'Search Archived Peridot Skills',
    prompt: 'Search archived stored skills by name or body text',
    placeHolder: 'parser release rust',
    ignoreFocusOut: true,
  });
  const trimmed = query?.trim();
  if (!trimmed) return;
  const folder = vscode.workspace.workspaceFolders?.[0]?.uri.fsPath;
  if (!folder) {
    const message = 'Open a workspace folder before searching archived Peridot skills.';
    sidebar.setWorkspaceProblem(message);
    await vscode.window.showWarningMessage(message);
    return;
  }
  await vscode.commands.executeCommand('peridot.chatView.focus');
  try {
    const result = await runSlashCommand(
      `/skills archived ${trimmed}`,
      output,
      sidebar,
      sidebar.currentRunOptions(),
    );
    sidebar.appendCommandResult(result);
  } catch (err) {
    const message = err instanceof Error ? err.message : String(err);
    output.appendLine(`[peridot] archived skills search failed: ${message}`);
    sidebar.appendError(message);
    await vscode.window.showErrorMessage(
      vscode.l10n.t('Peridot archived skill search failed: {0}', message),
    );
  }
}

export async function showSkill(
  skillName: string,
  output: vscode.OutputChannel,
  sidebar: PeridotSidebarProvider,
): Promise<void> {
  const name = skillName.trim().replace(/^\/+/, '');
  if (!name) return;
  const folder = vscode.workspace.workspaceFolders?.[0]?.uri.fsPath;
  if (!folder) {
    const message = 'Open a workspace folder before viewing Peridot skills.';
    sidebar.setWorkspaceProblem(message);
    await vscode.window.showWarningMessage(message);
    return;
  }
  await vscode.commands.executeCommand('peridot.chatView.focus');
  try {
    const result = await runSlashCommand(
      `/skills show ${name}`,
      output,
      sidebar,
      sidebar.currentRunOptions(),
    );
    sidebar.appendCommandResult(result);
  } catch (err) {
    const message = err instanceof Error ? err.message : String(err);
    output.appendLine(`[peridot] skill show failed: ${message}`);
    sidebar.appendError(message);
    await vscode.window.showErrorMessage(vscode.l10n.t('Peridot skill view failed: {0}', message));
  }
}

export async function useSkill(
  skillName: string,
  output: vscode.OutputChannel,
  sidebar: PeridotSidebarProvider,
): Promise<void> {
  const name = skillName.trim().replace(/^\/+/, '');
  if (!name) return;
  const folder = vscode.workspace.workspaceFolders?.[0]?.uri.fsPath;
  if (!folder) {
    const message = 'Open a workspace folder before using Peridot skills.';
    sidebar.setWorkspaceProblem(message);
    await vscode.window.showWarningMessage(message);
    return;
  }
  await vscode.commands.executeCommand('peridot.chatView.focus');
  try {
    const result = await runSlashCommand(
      `/skills use ${name}`,
      output,
      sidebar,
      sidebar.currentRunOptions(),
    );
    sidebar.appendCommandResult(result);
  } catch (err) {
    const message = err instanceof Error ? err.message : String(err);
    output.appendLine(`[peridot] skill use failed: ${message}`);
    sidebar.appendError(message);
    await vscode.window.showErrorMessage(vscode.l10n.t('Peridot skill use failed: {0}', message));
  }
}

export async function toggleSkillPin(
  skillName: string,
  pinned: boolean,
  output: vscode.OutputChannel,
  sidebar: PeridotSidebarProvider,
): Promise<void> {
  const name = skillName.trim().replace(/^\/+/, '');
  if (!name) return;
  const folder = vscode.workspace.workspaceFolders?.[0]?.uri.fsPath;
  if (!folder) {
    const message = 'Open a workspace folder before updating Peridot skills.';
    sidebar.setWorkspaceProblem(message);
    await vscode.window.showWarningMessage(message);
    return;
  }
  await vscode.commands.executeCommand('peridot.chatView.focus');
  try {
    const action = pinned ? 'pin' : 'unpin';
    const result = await runSlashCommand(
      `/skills ${action} ${name}`,
      output,
      sidebar,
      sidebar.currentRunOptions(),
    );
    sidebar.appendCommandResult(result);
  } catch (err) {
    const message = err instanceof Error ? err.message : String(err);
    output.appendLine(`[peridot] skill pin failed: ${message}`);
    sidebar.appendError(message);
    await vscode.window.showErrorMessage(vscode.l10n.t('Peridot skill update failed: {0}', message));
  }
}

export async function archiveSkill(
  skillName: string,
  output: vscode.OutputChannel,
  sidebar: PeridotSidebarProvider,
): Promise<void> {
  const name = skillName.trim().replace(/^\/+/, '');
  if (!name) return;
  const confirmed = await vscode.window.showWarningMessage(
    vscode.l10n.t('Archive Peridot skill {0}? It will be hidden from active skill lists.', name),
    { modal: true },
    'Archive',
  );
  if (confirmed !== 'Archive') return;
  const folder = vscode.workspace.workspaceFolders?.[0]?.uri.fsPath;
  if (!folder) {
    const message = 'Open a workspace folder before archiving Peridot skills.';
    sidebar.setWorkspaceProblem(message);
    await vscode.window.showWarningMessage(message);
    return;
  }
  await vscode.commands.executeCommand('peridot.chatView.focus');
  try {
    const result = await runSlashCommand(
      `/skills archive ${name}`,
      output,
      sidebar,
      sidebar.currentRunOptions(),
    );
    sidebar.appendCommandResult(result);
    await refreshSlashCatalog(output, sidebar);
  } catch (err) {
    const message = err instanceof Error ? err.message : String(err);
    output.appendLine(`[peridot] skill archive failed: ${message}`);
    sidebar.appendError(message);
    await vscode.window.showErrorMessage(vscode.l10n.t('Peridot skill archive failed: {0}', message));
  }
}

export async function restoreSkill(
  skillName: string,
  output: vscode.OutputChannel,
  sidebar: PeridotSidebarProvider,
): Promise<void> {
  const name = skillName.trim().replace(/^\/+/, '');
  if (!name) return;
  const folder = vscode.workspace.workspaceFolders?.[0]?.uri.fsPath;
  if (!folder) {
    const message = 'Open a workspace folder before restoring Peridot skills.';
    sidebar.setWorkspaceProblem(message);
    await vscode.window.showWarningMessage(message);
    return;
  }
  await vscode.commands.executeCommand('peridot.chatView.focus');
  try {
    const result = await runSlashCommand(
      `/skills restore ${name}`,
      output,
      sidebar,
      sidebar.currentRunOptions(),
    );
    sidebar.appendCommandResult(result);
    await refreshSlashCatalog(output, sidebar);
  } catch (err) {
    const message = err instanceof Error ? err.message : String(err);
    output.appendLine(`[peridot] skill restore failed: ${message}`);
    sidebar.appendError(message);
    await vscode.window.showErrorMessage(vscode.l10n.t('Peridot skill restore failed: {0}', message));
  }
}
