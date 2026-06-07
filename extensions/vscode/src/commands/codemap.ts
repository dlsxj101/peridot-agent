// Workspace code-map command handlers for the extension host.
//
// Scan / refresh / search / locate / outline / find-references against the
// daemon's semantic symbol index by driving the matching `/codemap …` slash
// command and rendering the result into the sidebar transcript. Split out of
// `extension.ts`; the handlers reuse the host's exported runSlashCommand and,
// for "locate", openWorkspaceFile to jump to the first matching definition.

import * as vscode from 'vscode';

import { openWorkspaceFile, runSlashCommand } from '../extension';
import type { PeridotSidebarProvider } from '../sidebar';

export async function showWorkspaceCodeMap(
  output: vscode.OutputChannel,
  sidebar: PeridotSidebarProvider,
  refresh: boolean,
  query?: string,
): Promise<void> {
  const folder = vscode.workspace.workspaceFolders?.[0]?.uri.fsPath;
  if (!folder) {
    const message = 'Open a workspace folder before scanning the code map.';
    sidebar.setWorkspaceProblem(message);
    await vscode.window.showWarningMessage(message);
    return;
  }
  await vscode.commands.executeCommand('peridot.chatView.focus');
  try {
    const command = query ? `/codemap find ${query}` : refresh ? '/codemap refresh' : '/codemap';
    const result = await vscode.window.withProgress(
      {
        location: vscode.ProgressLocation.Window,
        title: query
          ? 'Peridot: searching workspace code map'
          : refresh
            ? 'Peridot: refreshing workspace code map index'
            : 'Peridot: loading workspace code map',
      },
      async () =>
        runSlashCommand(
          command,
          output,
          sidebar,
          sidebar.currentRunOptions(),
        ),
    );
    sidebar.appendCommandResult(result);
  } catch (err) {
    const message = err instanceof Error ? err.message : String(err);
    output.appendLine(`[peridot] codemap failed: ${message}`);
    sidebar.appendError(message);
    await vscode.window.showErrorMessage(vscode.l10n.t('Peridot code map failed: {0}', message));
  }
}

export async function showWorkspaceCodeMapStatus(
  output: vscode.OutputChannel,
  sidebar: PeridotSidebarProvider,
): Promise<void> {
  const folder = vscode.workspace.workspaceFolders?.[0]?.uri.fsPath;
  if (!folder) {
    vscode.window.showWarningMessage(vscode.l10n.t('Open a workspace folder before checking the code map.'));
    return;
  }
  await vscode.commands.executeCommand('peridot.chatView.focus');
  try {
    const result = await runSlashCommand(
      '/codemap status',
      output,
      sidebar,
      sidebar.currentRunOptions(),
    );
    sidebar.appendCommandResult(result);
  } catch (err) {
    const message = err instanceof Error ? err.message : String(err);
    output.appendLine(`[peridot] codemap status failed: ${message}`);
    vscode.window.showErrorMessage(message);
  }
}

export async function searchWorkspaceCodeMap(
  output: vscode.OutputChannel,
  sidebar: PeridotSidebarProvider,
): Promise<void> {
  const query = await vscode.window.showInputBox({
    title: 'Search Workspace Code Map',
    prompt: 'Search indexed symbols, TODO markers, signatures, and paths',
    placeHolder: 'Runner TODO src/lib.rs',
    ignoreFocusOut: true,
  });
  const trimmed = query?.trim();
  if (!trimmed) return;
  await showWorkspaceCodeMap(output, sidebar, false, trimmed);
}

export async function locateWorkspaceCodeMapSymbol(
  output: vscode.OutputChannel,
  sidebar: PeridotSidebarProvider,
): Promise<void> {
  const query = await vscode.window.showInputBox({
    title: 'Locate Workspace Symbol',
    prompt: 'Open the first matching indexed symbol definition',
    placeHolder: 'Runner',
    ignoreFocusOut: true,
  });
  const trimmed = query?.trim();
  if (!trimmed) return;
  const folder = vscode.workspace.workspaceFolders?.[0]?.uri.fsPath;
  if (!folder) {
    const message = 'Open a workspace folder before locating a workspace symbol.';
    sidebar.setWorkspaceProblem(message);
    await vscode.window.showWarningMessage(message);
    return;
  }
  await vscode.commands.executeCommand('peridot.chatView.focus');
  try {
    const result = await vscode.window.withProgress(
      {
        location: vscode.ProgressLocation.Window,
        title: 'Peridot: locating workspace symbol',
      },
      async () =>
        runSlashCommand(
          `/codemap locate ${trimmed}`,
          output,
          sidebar,
          sidebar.currentRunOptions(),
        ),
    );
    sidebar.appendCommandResult(result);
    const first = result.items?.find((item) => typeof item.path === 'string');
    if (first?.path) {
      await openWorkspaceFile(
        first.path,
        output,
        first.line,
        first.column,
        { preview: true },
        folder,
      );
    } else {
      await vscode.window.showInformationMessage(vscode.l10n.t('No indexed symbol matched "{0}".', trimmed));
    }
  } catch (err) {
    const message = err instanceof Error ? err.message : String(err);
    output.appendLine(`[peridot] codemap locate failed: ${message}`);
    sidebar.appendError(message);
    await vscode.window.showErrorMessage(vscode.l10n.t('Peridot symbol locate failed: {0}', message));
  }
}

export async function outlineCurrentFile(
  output: vscode.OutputChannel,
  sidebar: PeridotSidebarProvider,
): Promise<void> {
  const editor = vscode.window.activeTextEditor;
  if (!editor) {
    await vscode.window.showWarningMessage(vscode.l10n.t('Open a source file before outlining it with Peridot.'));
    return;
  }
  const folder = vscode.workspace.workspaceFolders?.[0]?.uri.fsPath;
  if (!folder) {
    const message = 'Open a workspace folder before outlining a file.';
    sidebar.setWorkspaceProblem(message);
    await vscode.window.showWarningMessage(message);
    return;
  }
  const relativePath = vscode.workspace.asRelativePath(editor.document.uri, false);
  await vscode.commands.executeCommand('peridot.chatView.focus');
  try {
    const result = await vscode.window.withProgress(
      {
        location: vscode.ProgressLocation.Window,
        title: 'Peridot: outlining current file',
      },
      async () =>
        runSlashCommand(
          `/codemap outline ${relativePath}`,
          output,
          sidebar,
          sidebar.currentRunOptions(),
        ),
    );
    sidebar.appendCommandResult(result);
  } catch (err) {
    const message = err instanceof Error ? err.message : String(err);
    output.appendLine(`[peridot] codemap outline failed: ${message}`);
    sidebar.appendError(message);
    await vscode.window.showErrorMessage(vscode.l10n.t('Peridot file outline failed: {0}', message));
  }
}

export async function findWorkspaceSymbolReferences(
  output: vscode.OutputChannel,
  sidebar: PeridotSidebarProvider,
): Promise<void> {
  const query = await vscode.window.showInputBox({
    title: 'Find Workspace Symbol References',
    prompt: 'Find text references to an indexed symbol',
    placeHolder: 'Runner',
    ignoreFocusOut: true,
  });
  const trimmed = query?.trim();
  if (!trimmed) return;
  const folder = vscode.workspace.workspaceFolders?.[0]?.uri.fsPath;
  if (!folder) {
    const message = 'Open a workspace folder before finding symbol references.';
    sidebar.setWorkspaceProblem(message);
    await vscode.window.showWarningMessage(message);
    return;
  }
  await vscode.commands.executeCommand('peridot.chatView.focus');
  try {
    const result = await vscode.window.withProgress(
      {
        location: vscode.ProgressLocation.Window,
        title: 'Peridot: finding symbol references',
      },
      async () =>
        runSlashCommand(
          `/codemap refs ${trimmed}`,
          output,
          sidebar,
          sidebar.currentRunOptions(),
        ),
    );
    sidebar.appendCommandResult(result);
  } catch (err) {
    const message = err instanceof Error ? err.message : String(err);
    output.appendLine(`[peridot] codemap refs failed: ${message}`);
    sidebar.appendError(message);
    await vscode.window.showErrorMessage(vscode.l10n.t('Peridot symbol references failed: {0}', message));
  }
}
