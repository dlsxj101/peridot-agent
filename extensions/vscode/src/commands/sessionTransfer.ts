// Session export / import command handlers for the extension host.
//
// `exportSessionArtifacts` shells out to `peridot session export` to write a
// chosen session's attachments / notes / timeline to a folder; the import
// counterpart drives the `/session import` daemon slash command. Split out of
// `extension.ts`; both reuse the shared CLI helpers from ./cli and the host's
// exported session-list helpers.

import * as path from 'path';
import * as vscode from 'vscode';

import { fetchSessionList, normalizeDaemonSessions, refreshSessionList, runSlashCommand } from '../extension';
import {
  sessionExportChoices,
  sessionExportCommandResult,
  sessionExportDirectoryName,
} from '../sessionExportCommand';
import { sessionImportSlashCommand } from '../sessionImportCommand';
import type { PeridotSidebarProvider } from '../sidebar';
import type { DaemonSessionSummary } from '../types';
import { execPeridotCli, nonEmpty, parseJson, pathExists, sanitizePathSegment } from './cli';

export async function exportSessionArtifacts(
  output: vscode.OutputChannel,
  sidebar: PeridotSidebarProvider,
): Promise<void> {
  const folder = vscode.workspace.workspaceFolders?.[0]?.uri.fsPath;
  if (!folder) {
    const message = 'Open a workspace folder before exporting session artifacts.';
    sidebar.setWorkspaceProblem(message);
    await vscode.window.showWarningMessage(message);
    return;
  }
  let sessions: DaemonSessionSummary[] = [];
  try {
    sessions = normalizeDaemonSessions(await fetchSessionList(folder, output));
  } catch (err) {
    const message = err instanceof Error ? err.message : String(err);
    output.appendLine(`[peridot] session list fetch before export failed: ${message}`);
  }
  const choices = sessionExportChoices(sessions, sidebar.currentDaemonSessionId());
  if (choices.length === 0) {
    await vscode.window.showWarningMessage(vscode.l10n.t('Start, save, or import a Peridot session before exporting artifacts.'));
    return;
  }
  const target =
    choices.length === 1
      ? choices[0]
      : await vscode.window.showQuickPick(
          choices.map((choice) => ({
            label: choice.label,
            description: choice.description,
            detail: choice.detail ?? choice.id,
            id: choice.id,
          })),
          {
            title: 'Peridot: Export Session Artifacts',
            placeHolder: 'Choose a session to export',
            ignoreFocusOut: true,
          },
        );
  if (!target) return;
  const sessionId = target.id;
  const picked = await vscode.window.showOpenDialog({
    title: 'Peridot: Export Session Artifacts',
    canSelectFiles: false,
    canSelectFolders: true,
    canSelectMany: false,
    defaultUri: vscode.Uri.file(folder),
    openLabel: 'Export Here',
  });
  const base = picked?.[0];
  if (!base) return;
  const destination = path.join(base.fsPath, sessionExportDirectoryName(sessionId));
  let force = false;
  if (await pathExists(destination)) {
    const confirmed = await vscode.window.showWarningMessage(
      vscode.l10n.t('{0} already exists. Overwrite it?', destination),
      { modal: true },
      'Overwrite',
    );
    if (confirmed !== 'Overwrite') return;
    force = true;
  }
  await vscode.commands.executeCommand('peridot.chatView.focus');
  const args = [
    '--output',
    'json',
    'session',
    'export',
    sessionId,
    '--out',
    destination,
    '--artifact',
    'attachments',
    '--artifact',
    'notes',
    '--artifact',
    'timeline',
    ...(force ? ['--force'] : []),
  ];
  try {
    output.appendLine(`[peridot] exporting session artifacts: ${destination}`);
    const { stdout } = await vscode.window.withProgress(
      {
        location: vscode.ProgressLocation.Notification,
        title: 'Peridot: exporting session artifacts',
        cancellable: false,
      },
      async () => execPeridotCli(args, folder),
    );
    const payload = parseJson(stdout);
    sidebar.appendCommandResult(sessionExportCommandResult(payload, sessionId, destination));
    await vscode.commands.executeCommand('revealFileInOS', vscode.Uri.file(destination));
  } catch (err) {
    const message = err instanceof Error ? err.message : String(err);
    output.appendLine(`[peridot] session artifact export failed: ${message}`);
    sidebar.appendError(message);
    await vscode.window.showErrorMessage(vscode.l10n.t('Peridot export failed: {0}', message));
  }
}

export async function importSessionArtifacts(
  output: vscode.OutputChannel,
  sidebar: PeridotSidebarProvider,
): Promise<void> {
  const folder = vscode.workspace.workspaceFolders?.[0]?.uri.fsPath;
  if (!folder) {
    const message = 'Open a workspace folder before importing session artifacts.';
    sidebar.setWorkspaceProblem(message);
    await vscode.window.showWarningMessage(message);
    return;
  }
  const picked = await vscode.window.showOpenDialog({
    title: 'Peridot: Import Session Artifacts',
    canSelectFiles: false,
    canSelectFolders: true,
    canSelectMany: false,
    defaultUri: vscode.Uri.file(folder),
    openLabel: 'Import Session',
  });
  const source = picked?.[0];
  if (!source) return;
  const defaultId = sanitizePathSegment(path.basename(source.fsPath));
  const idInput = await vscode.window.showInputBox({
    title: 'Peridot: Import Session Artifacts',
    prompt: 'Imported session id. Leave empty to derive it from the selected folder name.',
    value: defaultId,
    ignoreFocusOut: true,
    validateInput: (value) =>
      /\s/.test(value.trim()) ? 'Session ids cannot contain whitespace.' : undefined,
  });
  if (idInput === undefined) return;
  const mode = await vscode.window.showQuickPick(
    [
      {
        label: 'Import',
        description: 'Keep any existing session with the same id',
        force: false,
      },
      {
        label: 'Import and overwrite',
        description: 'Replace an existing persisted session with the same id',
        force: true,
      },
    ],
    {
      title: 'Peridot: Import Session Artifacts',
      placeHolder: 'Choose how to handle existing sessions',
      ignoreFocusOut: true,
    },
  );
  if (!mode) return;
  const command = sessionImportSlashCommand({
    source: source.fsPath,
    id: nonEmpty(idInput),
    force: mode.force,
  });
  await vscode.commands.executeCommand('peridot.chatView.focus');
  try {
    output.appendLine(`[peridot] importing session artifacts: ${source.fsPath}`);
    const result = await vscode.window.withProgress(
      {
        location: vscode.ProgressLocation.Notification,
        title: 'Peridot: importing session artifacts',
        cancellable: false,
      },
      async () => runSlashCommand(command, output, sidebar, sidebar.currentRunOptions()),
    );
    sidebar.appendCommandResult(result);
    await refreshSessionList(output, sidebar);
  } catch (err) {
    const message = err instanceof Error ? err.message : String(err);
    output.appendLine(`[peridot] session artifact import failed: ${message}`);
    sidebar.appendError(message);
    await vscode.window.showErrorMessage(vscode.l10n.t('Peridot import failed: {0}', message));
  }
}
