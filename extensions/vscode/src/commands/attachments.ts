// Session attachment command handlers for the extension host.
//
// Attach a workspace file or a pasted inline image to the active session, list
// the current attachments, or detach one — by driving the matching `/attach`,
// `/attachments`, and `/detach` slash commands. Inline images are first written
// under `.peridot/attachments/<session>/` before being attached. Split out of
// `extension.ts`; the handlers reuse the host's exported runSlashCommand.

import * as fs from 'fs/promises';
import * as path from 'path';
import * as vscode from 'vscode';

import { runSlashCommand } from '../extension';
import { decodeInlineImageAttachment } from '../inlineImageAttachment';
import type { PeridotSidebarProvider } from '../sidebar';
import type { InlineImageAttachmentPayload } from '../types';

export async function attachFileToSession(
  output: vscode.OutputChannel,
  sidebar: PeridotSidebarProvider,
): Promise<void> {
  const folder = vscode.workspace.workspaceFolders?.[0]?.uri.fsPath;
  if (!folder) {
    const message = 'Open a workspace folder before attaching a file.';
    sidebar.setWorkspaceProblem(message);
    await vscode.window.showWarningMessage(message);
    return;
  }
  if (!sidebar.currentDaemonSessionId()) {
    await vscode.window.showWarningMessage(vscode.l10n.t('Start or select a Peridot session before attaching a file.'));
    return;
  }
  const picked = await vscode.window.showOpenDialog({
    title: 'Peridot: Attach File',
    canSelectFiles: true,
    canSelectFolders: false,
    canSelectMany: false,
    defaultUri: vscode.Uri.file(folder),
  });
  const file = picked?.[0];
  if (!file) return;
  const relative = path.relative(folder, file.fsPath).replace(/\\/g, '/');
  if (relative.startsWith('..') || path.isAbsolute(relative)) {
    await vscode.window.showWarningMessage(vscode.l10n.t('Peridot only attaches files inside the workspace.'));
    return;
  }
  await vscode.commands.executeCommand('peridot.chatView.focus');
  try {
    const result = await runSlashCommand(`/attach ${relative}`, output, sidebar, sidebar.currentRunOptions());
    sidebar.appendCommandResult(result);
  } catch (err) {
    const message = err instanceof Error ? err.message : String(err);
    output.appendLine(`[peridot] attach failed: ${message}`);
    sidebar.appendError(message);
    await vscode.window.showErrorMessage(vscode.l10n.t('Peridot attach failed: {0}', message));
  }
}

export async function attachInlineImageToSession(
  image: InlineImageAttachmentPayload,
  output: vscode.OutputChannel,
  sidebar: PeridotSidebarProvider,
): Promise<void> {
  const folder = vscode.workspace.workspaceFolders?.[0]?.uri.fsPath;
  if (!folder) {
    const message = 'Open a workspace folder before attaching an image.';
    sidebar.setWorkspaceProblem(message);
    await vscode.window.showWarningMessage(message);
    return;
  }
  if (!sidebar.currentDaemonSessionId()) {
    await vscode.window.showWarningMessage(vscode.l10n.t('Start or select a Peridot session before attaching an image.'));
    return;
  }
  await vscode.commands.executeCommand('peridot.chatView.focus');
  try {
    const attachment = decodeInlineImageAttachment(image);
    const sessionSegment = safeAttachmentDirectorySegment(
      sidebar.currentClientSessionId() ?? sidebar.currentDaemonSessionId() ?? 'session',
    );
    const relative = path
      .join(
        '.peridot',
        'attachments',
        sessionSegment,
        `${Date.now()}-${attachment.fileName}`,
      )
      .replace(/\\/g, '/');
    const absolute = path.join(folder, relative);
    await fs.mkdir(path.dirname(absolute), { recursive: true });
    await fs.writeFile(absolute, attachment.bytes, { flag: 'wx' });
    const result = await runSlashCommand(`/attach ${relative}`, output, sidebar, sidebar.currentRunOptions());
    sidebar.appendCommandResult(result);
  } catch (err) {
    const message = err instanceof Error ? err.message : String(err);
    output.appendLine(`[peridot] inline image attach failed: ${message}`);
    sidebar.appendError(message);
    await vscode.window.showErrorMessage(vscode.l10n.t('Peridot image attach failed: {0}', message));
  }
}

export async function showSessionAttachments(
  output: vscode.OutputChannel,
  sidebar: PeridotSidebarProvider,
): Promise<void> {
  if (!sidebar.currentDaemonSessionId()) {
    await vscode.window.showWarningMessage(vscode.l10n.t('Start or select a Peridot session before listing attachments.'));
    return;
  }
  await vscode.commands.executeCommand('peridot.chatView.focus');
  try {
    const result = await runSlashCommand('/attachments', output, sidebar, sidebar.currentRunOptions());
    sidebar.appendCommandResult(result);
  } catch (err) {
    const message = err instanceof Error ? err.message : String(err);
    output.appendLine(`[peridot] attachments failed: ${message}`);
    sidebar.appendError(message);
    await vscode.window.showErrorMessage(vscode.l10n.t('Peridot attachments failed: {0}', message));
  }
}

function safeAttachmentDirectorySegment(value: string): string {
  return value
    .replace(/[^A-Za-z0-9._-]+/g, '-')
    .replace(/^-+|-+$/g, '')
    .slice(0, 80) || 'session';
}

export async function detachAttachmentFromSession(
  attachmentPath: string,
  output: vscode.OutputChannel,
  sidebar: PeridotSidebarProvider,
): Promise<void> {
  const trimmed = attachmentPath.trim();
  if (!trimmed) return;
  if (!sidebar.currentDaemonSessionId()) {
    await vscode.window.showWarningMessage(vscode.l10n.t('Start or select a Peridot session before detaching a file.'));
    return;
  }
  const confirmed = await vscode.window.showWarningMessage(
    vscode.l10n.t('Detach {0} from this Peridot session context?', trimmed),
    { modal: true },
    'Detach',
  );
  if (confirmed !== 'Detach') return;
  await vscode.commands.executeCommand('peridot.chatView.focus');
  try {
    const result = await runSlashCommand(`/detach ${trimmed}`, output, sidebar, sidebar.currentRunOptions());
    sidebar.appendCommandResult(result);
  } catch (err) {
    const message = err instanceof Error ? err.message : String(err);
    output.appendLine(`[peridot] detach failed: ${message}`);
    sidebar.appendError(message);
    await vscode.window.showErrorMessage(vscode.l10n.t('Peridot detach failed: {0}', message));
  }
}
