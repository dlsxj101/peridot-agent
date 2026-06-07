// Context-inspection command handlers for the extension host.
//
// Scan workspace TODO markers, inspect the top of the active session's context
// budget, and show the working-tree diff — by driving the `/todos`,
// `/context top`, and `/diff` slash commands and rendering the result into the
// sidebar transcript. Split out of `extension.ts`; the handlers reuse the
// host's exported runSlashCommand.

import * as vscode from 'vscode';

import { runSlashCommand } from '../extension';
import type { PeridotSidebarProvider } from '../sidebar';

export async function showWorkspaceTodos(
  output: vscode.OutputChannel,
  sidebar: PeridotSidebarProvider,
): Promise<void> {
  const folder = vscode.workspace.workspaceFolders?.[0]?.uri.fsPath;
  if (!folder) {
    const message = 'Open a workspace folder before scanning TODO markers.';
    sidebar.setWorkspaceProblem(message);
    await vscode.window.showWarningMessage(message);
    return;
  }
  await vscode.commands.executeCommand('peridot.chatView.focus');
  try {
    const result = await vscode.window.withProgress(
      {
        location: vscode.ProgressLocation.Notification,
        title: 'Peridot: scanning TODO markers',
        cancellable: false,
      },
      async () => runSlashCommand('/todos', output, sidebar, sidebar.currentRunOptions()),
    );
    sidebar.appendCommandResult(result);
  } catch (err) {
    const message = err instanceof Error ? err.message : String(err);
    output.appendLine(`[peridot] todos failed: ${message}`);
    sidebar.appendError(message);
    await vscode.window.showErrorMessage(vscode.l10n.t('Peridot TODO scan failed: {0}', message));
  }
}

export async function showContextTop(
  output: vscode.OutputChannel,
  sidebar: PeridotSidebarProvider,
): Promise<void> {
  if (!sidebar.currentDaemonSessionId()) {
    await vscode.window.showWarningMessage(
      vscode.l10n.t('Start, save, or select a Peridot session before inspecting context.'),
    );
    return;
  }
  await vscode.commands.executeCommand('peridot.chatView.focus');
  try {
    const result = await runSlashCommand(
      '/context top',
      output,
      sidebar,
      sidebar.currentRunOptions(),
    );
    sidebar.appendCommandResult(result);
  } catch (err) {
    const message = err instanceof Error ? err.message : String(err);
    output.appendLine(`[peridot] context top failed: ${message}`);
    sidebar.appendError(message);
    await vscode.window.showErrorMessage(vscode.l10n.t('Peridot context inspection failed: {0}', message));
  }
}

export async function showWorkingTreeDiff(
  output: vscode.OutputChannel,
  sidebar: PeridotSidebarProvider,
): Promise<void> {
  const folder = vscode.workspace.workspaceFolders?.[0]?.uri.fsPath;
  if (!folder) {
    const message = 'Open a workspace folder before showing the working tree diff.';
    sidebar.setWorkspaceProblem(message);
    await vscode.window.showWarningMessage(message);
    return;
  }
  await vscode.commands.executeCommand('peridot.chatView.focus');
  try {
    const result = await runSlashCommand('/diff', output, sidebar, sidebar.currentRunOptions());
    sidebar.appendCommandResult(result);
  } catch (err) {
    const message = err instanceof Error ? err.message : String(err);
    output.appendLine(`[peridot] diff failed: ${message}`);
    sidebar.appendError(message);
    await vscode.window.showErrorMessage(vscode.l10n.t('Peridot diff failed: {0}', message));
  }
}
