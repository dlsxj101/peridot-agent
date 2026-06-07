// Session-note command handlers for the extension host.
//
// Add / show / clear operator notes on the active persisted session by driving
// the matching `/session note(s) …` daemon slash command and rendering the
// result into the sidebar transcript. Split out of `extension.ts`; the public
// handlers take the shared output channel + sidebar and reuse the host's
// now-exported runSlashCommand helper. The ensureActiveNotesSession guard stays
// private to this module.

import * as vscode from 'vscode';

import { runSlashCommand } from '../extension';
import {
  parseNotesLastInput,
  sessionNoteSlashCommand,
  sessionNotesClearSlashCommand,
  sessionNotesSlashCommand,
} from '../sessionNotesCommand';
import type { PeridotSidebarProvider } from '../sidebar';

export async function addSessionNote(
  output: vscode.OutputChannel,
  sidebar: PeridotSidebarProvider,
): Promise<void> {
  if (!(await ensureActiveNotesSession(sidebar))) return;
  const note = await vscode.window.showInputBox({
    title: 'Peridot: Add Session Note',
    prompt: 'Add an operator note to the active Peridot session.',
    placeHolder: 'checkpoint: verified replay output',
    ignoreFocusOut: true,
  });
  if (note === undefined) return;
  let command: string;
  try {
    command = sessionNoteSlashCommand(note);
  } catch (err) {
    const message = err instanceof Error ? err.message : String(err);
    await vscode.window.showErrorMessage(vscode.l10n.t('Peridot note failed: {0}', message));
    return;
  }
  await vscode.commands.executeCommand('peridot.chatView.focus');
  try {
    const result = await runSlashCommand(command, output, sidebar, sidebar.currentRunOptions());
    sidebar.appendCommandResult(result);
  } catch (err) {
    const message = err instanceof Error ? err.message : String(err);
    output.appendLine(`[peridot] note failed: ${message}`);
    sidebar.appendError(message);
    await vscode.window.showErrorMessage(vscode.l10n.t('Peridot note failed: {0}', message));
  }
}

export async function showSessionNotes(
  output: vscode.OutputChannel,
  sidebar: PeridotSidebarProvider,
): Promise<void> {
  if (!(await ensureActiveNotesSession(sidebar))) return;
  const lastInput = await vscode.window.showInputBox({
    title: 'Peridot: Show Session Notes',
    prompt: 'Notes to show. Leave empty for all notes.',
    placeHolder: 'all',
    ignoreFocusOut: true,
    validateInput: (value) => {
      try {
        parseNotesLastInput(value);
        return undefined;
      } catch (err) {
        return err instanceof Error ? err.message : String(err);
      }
    },
  });
  if (lastInput === undefined) return;
  let command: string;
  try {
    command = sessionNotesSlashCommand(parseNotesLastInput(lastInput));
  } catch (err) {
    const message = err instanceof Error ? err.message : String(err);
    await vscode.window.showErrorMessage(vscode.l10n.t('Peridot notes failed: {0}', message));
    return;
  }
  await vscode.commands.executeCommand('peridot.chatView.focus');
  try {
    const result = await runSlashCommand(command, output, sidebar, sidebar.currentRunOptions());
    sidebar.appendCommandResult(result);
  } catch (err) {
    const message = err instanceof Error ? err.message : String(err);
    output.appendLine(`[peridot] notes failed: ${message}`);
    sidebar.appendError(message);
    await vscode.window.showErrorMessage(vscode.l10n.t('Peridot notes failed: {0}', message));
  }
}

export async function clearSessionNotes(
  output: vscode.OutputChannel,
  sidebar: PeridotSidebarProvider,
): Promise<void> {
  if (!(await ensureActiveNotesSession(sidebar))) return;
  const confirmed = await vscode.window.showWarningMessage(
    vscode.l10n.t('Clear all notes for the active Peridot session?'),
    { modal: true },
    'Clear Notes',
  );
  if (confirmed !== 'Clear Notes') return;
  await vscode.commands.executeCommand('peridot.chatView.focus');
  try {
    const result = await runSlashCommand(
      sessionNotesClearSlashCommand(),
      output,
      sidebar,
      sidebar.currentRunOptions(),
    );
    sidebar.appendCommandResult(result);
  } catch (err) {
    const message = err instanceof Error ? err.message : String(err);
    output.appendLine(`[peridot] notes clear failed: ${message}`);
    sidebar.appendError(message);
    await vscode.window.showErrorMessage(vscode.l10n.t('Peridot notes clear failed: {0}', message));
  }
}

async function ensureActiveNotesSession(sidebar: PeridotSidebarProvider): Promise<boolean> {
  if (sidebar.currentDaemonSessionId()) return true;
  await vscode.window.showWarningMessage(
    vscode.l10n.t('Start, save, or select a persisted Peridot session before using session notes.'),
  );
  return false;
}
