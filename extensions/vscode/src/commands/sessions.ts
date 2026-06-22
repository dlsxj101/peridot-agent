// Persisted-session command handlers for the extension host.
//
// List / create / switch / close / count / inspect / rename / delete / search /
// prune / replay the workspace's persisted Peridot sessions by driving the
// matching `/session …` daemon slash command and rendering the result into the
// sidebar transcript. Split out of `extension.ts`; the public handlers take the
// shared output channel + sidebar and reuse the host's exported run/session
// lifecycle helpers (runSlashCommand, runTask, refreshSessionList,
// finishRunBySession, fetchSessionList, normalizeDaemonSessions). The
// inspectPersistedSession / pickPersistedSessionTarget helpers stay private.

import * as vscode from 'vscode';

import {
  fetchSessionList,
  finishRunBySession,
  normalizeDaemonSessions,
  refreshSessionList,
  runSlashCommand,
  runTask,
} from '../extension';
import { ensureWorkspaceFolder } from './cli';
import {
  sessionCloseSlashCommand,
  sessionCountSlashCommand,
  sessionDeleteSlashCommand,
  sessionLocateSlashCommand,
  sessionNewSlashCommand,
  sessionRenameSlashCommand,
  sessionResumeSlashCommand,
  sessionShowSlashCommand,
  sessionSwitchSlashCommand,
  sessionTargetChoices,
  type SessionTargetChoice,
} from '../sessionInspectCommand';
import { sessionListSlashCommand, sessionListStatusChoices } from '../sessionListCommand';
import {
  parsePruneOlderThanDaysInput,
  sessionPruneSlashCommand,
  sessionPruneStatusChoices,
} from '../sessionPruneCommand';
import {
  parseReplayLastInput,
  sessionReplayChoices,
  sessionReplaySlashCommand,
} from '../sessionReplayCommand';
import { sessionSearchSlashCommand } from '../sessionSearchCommand';
import type { PeridotSidebarProvider } from '../sidebar';
import type { DaemonSessionSummary } from '../types';

export async function showSessions(
  output: vscode.OutputChannel,
  sidebar: PeridotSidebarProvider,
): Promise<void> {
  const folder = await ensureWorkspaceFolder(sidebar, 'Open a workspace folder before listing Peridot sessions.');
  if (!folder) return;
  const status = await vscode.window.showQuickPick(
    sessionListStatusChoices().map((choice) => ({
      label: choice.label,
      description: choice.description,
      status: choice.status,
    })),
    {
      title: 'Peridot: Show Sessions',
      placeHolder: 'Choose which persisted sessions to show',
      ignoreFocusOut: true,
    },
  );
  if (!status) return;
  let command: string;
  try {
    command = sessionListSlashCommand(status.status);
  } catch (err) {
    const message = err instanceof Error ? err.message : String(err);
    await vscode.window.showErrorMessage(vscode.l10n.t('Peridot session list failed: {0}', message));
    return;
  }
  await vscode.commands.executeCommand('peridot.chatView.focus');
  try {
    output.appendLine(`[peridot] listing sessions: ${command}`);
    const result = await vscode.window.withProgress(
      {
        location: vscode.ProgressLocation.Notification,
        title: 'Peridot: listing sessions',
        cancellable: false,
      },
      async () => runSlashCommand(command, output, sidebar, sidebar.currentRunOptions()),
    );
    sidebar.reconcileDaemonSessions(Array.isArray(result.sessions) ? result.sessions : [], {
      pruneMissing: !status.status,
    });
    sidebar.appendCommandResult(result);
  } catch (err) {
    const message = err instanceof Error ? err.message : String(err);
    output.appendLine(`[peridot] session list failed: ${message}`);
    sidebar.appendError(message);
    await vscode.window.showErrorMessage(vscode.l10n.t('Peridot session list failed: {0}', message));
  }
}

export async function newPersistedSession(
  output: vscode.OutputChannel,
  sidebar: PeridotSidebarProvider,
): Promise<void> {
  const folder = await ensureWorkspaceFolder(sidebar, 'Open a workspace folder before creating Peridot sessions.');
  if (!folder) return;
  const task = await vscode.window.showInputBox({
    title: 'Peridot: New Session',
    prompt: 'Optional initial task. Leave empty to open an idle persisted session.',
    placeHolder: 'fix parser tests',
    ignoreFocusOut: true,
  });
  if (task === undefined) return;
  const command = sessionNewSlashCommand(task);
  await vscode.commands.executeCommand('peridot.chatView.focus');
  try {
    output.appendLine(`[peridot] creating session: ${command}`);
    const result = await vscode.window.withProgress(
      {
        location: vscode.ProgressLocation.Notification,
        title: 'Peridot: creating session',
        cancellable: false,
      },
      async () => runSlashCommand(command, output, sidebar, sidebar.currentRunOptions()),
    );
    sidebar.appendCommandResult(result);
    await refreshSessionList(output, sidebar);
    if (result.session_id) {
      sidebar.selectSession(result.session_id);
    }
    const startTask = result.kind === 'session_new' ? result.task?.trim() : undefined;
    if (startTask) {
      await runTask(startTask, output, sidebar, sidebar.currentRunOptions());
    }
  } catch (err) {
    const message = err instanceof Error ? err.message : String(err);
    output.appendLine(`[peridot] session new failed: ${message}`);
    sidebar.appendError(message);
    await vscode.window.showErrorMessage(vscode.l10n.t('Peridot session new failed: {0}', message));
  }
}

export async function switchPersistedSession(
  output: vscode.OutputChannel,
  sidebar: PeridotSidebarProvider,
): Promise<void> {
  const target = await pickPersistedSessionTarget(
    output,
    sidebar,
    'Peridot: Switch Session',
    'Choose a persisted session to switch to',
    'Save or import a Peridot session before switching sessions.',
  );
  if (!target) return;
  let command: string;
  try {
    command = sessionSwitchSlashCommand(target.id);
  } catch (err) {
    const message = err instanceof Error ? err.message : String(err);
    await vscode.window.showErrorMessage(vscode.l10n.t('Peridot session switch failed: {0}', message));
    return;
  }
  await vscode.commands.executeCommand('peridot.chatView.focus');
  try {
    output.appendLine(`[peridot] switching session: ${target.id}`);
    const result = await vscode.window.withProgress(
      {
        location: vscode.ProgressLocation.Notification,
        title: 'Peridot: switching session',
        cancellable: false,
      },
      async () => runSlashCommand(command, output, sidebar, sidebar.currentRunOptions()),
    );
    sidebar.appendCommandResult(result);
    await refreshSessionList(output, sidebar);
    if (result.session_id && result.switched === true) {
      sidebar.selectSession(result.session_id);
    }
  } catch (err) {
    const message = err instanceof Error ? err.message : String(err);
    output.appendLine(`[peridot] session switch failed: ${message}`);
    sidebar.appendError(message);
    await vscode.window.showErrorMessage(vscode.l10n.t('Peridot session switch failed: {0}', message));
  }
}

export async function closePersistedSession(
  output: vscode.OutputChannel,
  sidebar: PeridotSidebarProvider,
): Promise<void> {
  const target = await pickPersistedSessionTarget(
    output,
    sidebar,
    'Peridot: Close Session',
    'Choose a persisted session to close',
    'Save or import a Peridot session before closing sessions.',
  );
  if (!target) return;
  const label = target.label === target.id ? target.id : `${target.label} (${target.id})`;
  const confirmed = await vscode.window.showWarningMessage(
    vscode.l10n.t('Close Peridot session {0}? This cancels any live run and removes its persisted record.', label),
    { modal: true },
    'Close Session',
  );
  if (confirmed !== 'Close Session') return;
  let command: string;
  try {
    command = sessionCloseSlashCommand(target.id);
  } catch (err) {
    const message = err instanceof Error ? err.message : String(err);
    await vscode.window.showErrorMessage(vscode.l10n.t('Peridot session close failed: {0}', message));
    return;
  }
  await vscode.commands.executeCommand('peridot.chatView.focus');
  try {
    output.appendLine(`[peridot] closing session: ${target.id}`);
    const result = await vscode.window.withProgress(
      {
        location: vscode.ProgressLocation.Notification,
        title: 'Peridot: closing session',
        cancellable: false,
      },
      async () => runSlashCommand(command, output, sidebar, sidebar.currentRunOptions()),
    );
    sidebar.appendCommandResult(result);
    if (result.session_id && (result.cancelled === true || result.deleted === true)) {
      await finishRunBySession(result.session_id, output);
    }
    await refreshSessionList(output, sidebar);
  } catch (err) {
    const message = err instanceof Error ? err.message : String(err);
    output.appendLine(`[peridot] session close failed: ${message}`);
    sidebar.appendError(message);
    await vscode.window.showErrorMessage(vscode.l10n.t('Peridot session close failed: {0}', message));
  }
}

export async function showSessionCount(
  output: vscode.OutputChannel,
  sidebar: PeridotSidebarProvider,
): Promise<void> {
  const folder = await ensureWorkspaceFolder(sidebar, 'Open a workspace folder before counting Peridot sessions.');
  if (!folder) return;
  const command = sessionCountSlashCommand();
  await vscode.commands.executeCommand('peridot.chatView.focus');
  try {
    output.appendLine(`[peridot] counting sessions: ${command}`);
    const result = await vscode.window.withProgress(
      {
        location: vscode.ProgressLocation.Notification,
        title: 'Peridot: counting sessions',
        cancellable: false,
      },
      async () => runSlashCommand(command, output, sidebar, sidebar.currentRunOptions()),
    );
    sidebar.appendCommandResult(result);
  } catch (err) {
    const message = err instanceof Error ? err.message : String(err);
    output.appendLine(`[peridot] session count failed: ${message}`);
    sidebar.appendError(message);
    await vscode.window.showErrorMessage(vscode.l10n.t('Peridot session count failed: {0}', message));
  }
}

export async function showPersistedSessionDetails(
  output: vscode.OutputChannel,
  sidebar: PeridotSidebarProvider,
): Promise<void> {
  await inspectPersistedSession(
    output,
    sidebar,
    'Peridot: Show Session Details',
    'Choose a persisted session to inspect',
    'showing session details',
    sessionShowSlashCommand,
  );
}

export async function locatePersistedSessionDirectory(
  output: vscode.OutputChannel,
  sidebar: PeridotSidebarProvider,
): Promise<void> {
  await inspectPersistedSession(
    output,
    sidebar,
    'Peridot: Locate Session Directory',
    'Choose a persisted session to locate',
    'locating session directory',
    sessionLocateSlashCommand,
  );
}

export async function resumePersistedSession(
  output: vscode.OutputChannel,
  sidebar: PeridotSidebarProvider,
): Promise<void> {
  await inspectPersistedSession(
    output,
    sidebar,
    'Peridot: Resume Session',
    'Choose a persisted session to resume',
    'resuming session',
    sessionResumeSlashCommand,
    true,
  );
}

export async function renamePersistedSession(
  output: vscode.OutputChannel,
  sidebar: PeridotSidebarProvider,
): Promise<void> {
  const target = await pickPersistedSessionTarget(
    output,
    sidebar,
    'Peridot: Rename Session',
    'Choose a persisted session to rename',
    'Save or import a Peridot session before renaming sessions.',
  );
  if (!target) return;
  const title = await vscode.window.showInputBox({
    title: 'Peridot: Rename Session',
    prompt: 'Enter the new persisted session title.',
    value: target.label,
    ignoreFocusOut: true,
    validateInput: (value) => (value.trim().length === 0 ? 'Session title is required.' : undefined),
  });
  if (title === undefined) return;
  let command: string;
  try {
    command = sessionRenameSlashCommand(target.id, title);
  } catch (err) {
    const message = err instanceof Error ? err.message : String(err);
    await vscode.window.showErrorMessage(vscode.l10n.t('Peridot session rename failed: {0}', message));
    return;
  }
  await vscode.commands.executeCommand('peridot.chatView.focus');
  try {
    output.appendLine(`[peridot] renaming session: ${target.id}`);
    const result = await vscode.window.withProgress(
      {
        location: vscode.ProgressLocation.Notification,
        title: 'Peridot: renaming session',
        cancellable: false,
      },
      async () => runSlashCommand(command, output, sidebar, sidebar.currentRunOptions()),
    );
    sidebar.appendCommandResult(result);
    await refreshSessionList(output, sidebar);
  } catch (err) {
    const message = err instanceof Error ? err.message : String(err);
    output.appendLine(`[peridot] session rename failed: ${message}`);
    sidebar.appendError(message);
    await vscode.window.showErrorMessage(vscode.l10n.t('Peridot session rename failed: {0}', message));
  }
}

export async function deletePersistedSession(
  output: vscode.OutputChannel,
  sidebar: PeridotSidebarProvider,
): Promise<void> {
  const target = await pickPersistedSessionTarget(
    output,
    sidebar,
    'Peridot: Delete Session',
    'Choose a persisted session to delete',
    'Save or import a Peridot session before deleting sessions.',
  );
  if (!target) return;
  const label = target.label === target.id ? target.id : `${target.label} (${target.id})`;
  const confirmed = await vscode.window.showWarningMessage(
    vscode.l10n.t('Delete persisted Peridot session {0}? This cannot be undone.', label),
    { modal: true },
    'Delete Session',
  );
  if (confirmed !== 'Delete Session') return;
  let command: string;
  try {
    command = sessionDeleteSlashCommand(target.id);
  } catch (err) {
    const message = err instanceof Error ? err.message : String(err);
    await vscode.window.showErrorMessage(vscode.l10n.t('Peridot session delete failed: {0}', message));
    return;
  }
  await vscode.commands.executeCommand('peridot.chatView.focus');
  try {
    output.appendLine(`[peridot] deleting session: ${target.id}`);
    const result = await vscode.window.withProgress(
      {
        location: vscode.ProgressLocation.Notification,
        title: 'Peridot: deleting session',
        cancellable: false,
      },
      async () => runSlashCommand(command, output, sidebar, sidebar.currentRunOptions()),
    );
    sidebar.appendCommandResult(result);
    if (result.session_id && (result.cancelled === true || result.deleted === true)) {
      await finishRunBySession(result.session_id, output);
    }
    await refreshSessionList(output, sidebar);
  } catch (err) {
    const message = err instanceof Error ? err.message : String(err);
    output.appendLine(`[peridot] session delete failed: ${message}`);
    sidebar.appendError(message);
    await vscode.window.showErrorMessage(vscode.l10n.t('Peridot session delete failed: {0}', message));
  }
}

async function inspectPersistedSession(
  output: vscode.OutputChannel,
  sidebar: PeridotSidebarProvider,
  title: string,
  placeHolder: string,
  progressLabel: string,
  buildCommand: (target: string) => string,
  runReturnedTask = false,
): Promise<void> {
  const target = await pickPersistedSessionTarget(
    output,
    sidebar,
    title,
    placeHolder,
    'Save or import a Peridot session before inspecting sessions.',
  );
  if (!target) return;
  let command: string;
  try {
    command = buildCommand(target.id);
  } catch (err) {
    const message = err instanceof Error ? err.message : String(err);
    await vscode.window.showErrorMessage(vscode.l10n.t('{0} failed: {1}', title, message));
    return;
  }
  await vscode.commands.executeCommand('peridot.chatView.focus');
  try {
    output.appendLine(`[peridot] ${progressLabel}: ${target.id}`);
    const result = await vscode.window.withProgress(
      {
        location: vscode.ProgressLocation.Notification,
        title: `${title}: ${progressLabel}`,
        cancellable: false,
      },
      async () => runSlashCommand(command, output, sidebar, sidebar.currentRunOptions()),
    );
    sidebar.appendCommandResult(result);
    if (runReturnedTask && result.kind === 'start_task') {
      const task = result.task?.trim();
      if (task) {
        await runTask(task, output, sidebar, sidebar.currentRunOptions());
      }
    }
  } catch (err) {
    const message = err instanceof Error ? err.message : String(err);
    output.appendLine(`[peridot] ${progressLabel} failed: ${message}`);
    sidebar.appendError(message);
    await vscode.window.showErrorMessage(vscode.l10n.t('{0} failed: {1}', title, message));
  }
}

async function pickPersistedSessionTarget(
  output: vscode.OutputChannel,
  sidebar: PeridotSidebarProvider,
  title: string,
  placeHolder: string,
  emptyMessage: string,
): Promise<SessionTargetChoice | undefined> {
  const folder = await ensureWorkspaceFolder(sidebar, 'Open a workspace folder before selecting Peridot sessions.');
  if (!folder) return undefined;
  let sessions: DaemonSessionSummary[] = [];
  try {
    sessions = normalizeDaemonSessions(await fetchSessionList(folder, output));
  } catch (err) {
    const message = err instanceof Error ? err.message : String(err);
    output.appendLine(`[peridot] session list fetch before picker failed: ${message}`);
  }
  const choices = sessionTargetChoices(sessions);
  if (choices.length === 0) {
    await vscode.window.showWarningMessage(emptyMessage);
    return undefined;
  }
  if (choices.length === 1) return choices[0];
  return vscode.window.showQuickPick(
    choices.map((choice) => ({
      label: choice.label,
      description: choice.description,
      detail: choice.detail ?? choice.id,
      id: choice.id,
    })),
    {
      title,
      placeHolder,
      ignoreFocusOut: true,
    },
  );
}

export async function searchSessions(
  output: vscode.OutputChannel,
  sidebar: PeridotSidebarProvider,
): Promise<void> {
  const folder = await ensureWorkspaceFolder(sidebar, 'Open a workspace folder before searching Peridot sessions.');
  if (!folder) return;
  const query = await vscode.window.showInputBox({
    title: 'Peridot: Search Sessions',
    prompt: 'Search persisted session transcripts.',
    placeHolder: 'parser failure',
    ignoreFocusOut: true,
    validateInput: (value) => (value.trim().length === 0 ? 'Search query is required.' : undefined),
  });
  if (query === undefined) return;
  let command: string;
  try {
    command = sessionSearchSlashCommand(query);
  } catch (err) {
    const message = err instanceof Error ? err.message : String(err);
    await vscode.window.showErrorMessage(vscode.l10n.t('Peridot session search failed: {0}', message));
    return;
  }
  await vscode.commands.executeCommand('peridot.chatView.focus');
  try {
    output.appendLine(`[peridot] searching sessions: ${command}`);
    const result = await vscode.window.withProgress(
      {
        location: vscode.ProgressLocation.Notification,
        title: 'Peridot: searching sessions',
        cancellable: false,
      },
      async () => runSlashCommand(command, output, sidebar, sidebar.currentRunOptions()),
    );
    sidebar.appendCommandResult(result);
  } catch (err) {
    const message = err instanceof Error ? err.message : String(err);
    output.appendLine(`[peridot] session search failed: ${message}`);
    sidebar.appendError(message);
    await vscode.window.showErrorMessage(vscode.l10n.t('Peridot session search failed: {0}', message));
  }
}

export async function pruneSessions(
  output: vscode.OutputChannel,
  sidebar: PeridotSidebarProvider,
): Promise<void> {
  const folder = await ensureWorkspaceFolder(sidebar, 'Open a workspace folder before pruning sessions.');
  if (!folder) return;
  const status = await vscode.window.showQuickPick(
    sessionPruneStatusChoices().map((choice) => ({
      label: choice.label,
      description: choice.description,
      status: choice.status,
    })),
    {
      title: 'Peridot: Prune Sessions',
      placeHolder: 'Choose which session status to match',
      ignoreFocusOut: true,
    },
  );
  if (!status) return;
  const daysInput = await vscode.window.showInputBox({
    title: 'Peridot: Prune Sessions',
    prompt: 'Only match sessions older than this many days. Leave empty for no age filter.',
    placeHolder: 'no age filter',
    ignoreFocusOut: true,
    validateInput: (value) => {
      try {
        parsePruneOlderThanDaysInput(value);
        return undefined;
      } catch (err) {
        return err instanceof Error ? err.message : String(err);
      }
    },
  });
  if (daysInput === undefined) return;
  let olderThanDays: number | undefined;
  let previewCommand: string;
  let pruneCommand: string;
  try {
    olderThanDays = parsePruneOlderThanDaysInput(daysInput);
    const options = { status: status.status, olderThanDays };
    previewCommand = sessionPruneSlashCommand({ ...options, dryRun: true });
    pruneCommand = sessionPruneSlashCommand(options);
  } catch (err) {
    const message = err instanceof Error ? err.message : String(err);
    await vscode.window.showErrorMessage(vscode.l10n.t('Peridot session prune failed: {0}', message));
    return;
  }
  await vscode.commands.executeCommand('peridot.chatView.focus');
  try {
    output.appendLine(`[peridot] previewing session prune: ${previewCommand}`);
    const preview = await vscode.window.withProgress(
      {
        location: vscode.ProgressLocation.Notification,
        title: 'Peridot: previewing session prune',
        cancellable: false,
      },
      async () => runSlashCommand(previewCommand, output, sidebar, sidebar.currentRunOptions()),
    );
    sidebar.appendCommandResult(preview);
    const total = typeof preview.total === 'number' ? preview.total : 0;
    if (total <= 0) {
      await vscode.window.showInformationMessage(vscode.l10n.t('No persisted sessions match those prune filters.'));
      return;
    }
    const confirmed = await vscode.window.showWarningMessage(
      vscode.l10n.t('Remove {0} persisted Peridot session(s)? This cannot be undone.', total),
      { modal: true },
      'Prune Sessions',
    );
    if (confirmed !== 'Prune Sessions') return;
    output.appendLine(`[peridot] pruning sessions: ${pruneCommand}`);
    const result = await vscode.window.withProgress(
      {
        location: vscode.ProgressLocation.Notification,
        title: 'Peridot: pruning sessions',
        cancellable: false,
      },
      async () => runSlashCommand(pruneCommand, output, sidebar, sidebar.currentRunOptions()),
    );
    sidebar.appendCommandResult(result);
    await refreshSessionList(output, sidebar);
  } catch (err) {
    const message = err instanceof Error ? err.message : String(err);
    output.appendLine(`[peridot] session prune failed: ${message}`);
    sidebar.appendError(message);
    await vscode.window.showErrorMessage(vscode.l10n.t('Peridot session prune failed: {0}', message));
  }
}

export async function replaySessionTimeline(
  output: vscode.OutputChannel,
  sidebar: PeridotSidebarProvider,
): Promise<void> {
  const folder = await ensureWorkspaceFolder(sidebar, 'Open a workspace folder before replaying session timelines.');
  if (!folder) return;
  let sessions: DaemonSessionSummary[] = [];
  try {
    sessions = normalizeDaemonSessions(await fetchSessionList(folder, output));
  } catch (err) {
    const message = err instanceof Error ? err.message : String(err);
    output.appendLine(`[peridot] session list fetch before replay failed: ${message}`);
  }
  const choices = sessionReplayChoices(sessions);
  if (choices.length === 0) {
    await vscode.window.showWarningMessage(vscode.l10n.t('Save or import a Peridot session before replaying timelines.'));
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
            title: 'Peridot: Replay Session Timeline',
            placeHolder: 'Choose a persisted session to replay',
            ignoreFocusOut: true,
          },
        );
  if (!target) return;
  const lastInput = await vscode.window.showInputBox({
    title: 'Peridot: Replay Session Timeline',
    prompt: 'Timeline entries to show. Leave empty for the full replay.',
    placeHolder: 'all',
    ignoreFocusOut: true,
    validateInput: (value) => {
      try {
        parseReplayLastInput(value);
        return undefined;
      } catch (err) {
        return err instanceof Error ? err.message : String(err);
      }
    },
  });
  if (lastInput === undefined) return;
  let command: string;
  try {
    command = sessionReplaySlashCommand(target.id, parseReplayLastInput(lastInput));
  } catch (err) {
    const message = err instanceof Error ? err.message : String(err);
    await vscode.window.showErrorMessage(vscode.l10n.t('Peridot replay failed: {0}', message));
    return;
  }
  await vscode.commands.executeCommand('peridot.chatView.focus');
  try {
    output.appendLine(`[peridot] replaying session timeline: ${target.id}`);
    const result = await vscode.window.withProgress(
      {
        location: vscode.ProgressLocation.Notification,
        title: 'Peridot: replaying session timeline',
        cancellable: false,
      },
      async () => runSlashCommand(command, output, sidebar, sidebar.currentRunOptions()),
    );
    sidebar.appendCommandResult(result);
  } catch (err) {
    const message = err instanceof Error ? err.message : String(err);
    output.appendLine(`[peridot] session replay failed: ${message}`);
    sidebar.appendError(message);
    await vscode.window.showErrorMessage(vscode.l10n.t('Peridot replay failed: {0}', message));
  }
}
