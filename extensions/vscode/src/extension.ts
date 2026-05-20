// Peridot Agent — VS Code extension entry point.
//
// Bridge surface: sanity commands plus a first "Run Task" command that
// drives `session.start` and streams daemon notifications into an Output
// Channel. The WebView chat panel lands after this transport slice is stable.

import * as vscode from 'vscode';
import { PeridotDaemon, RpcNotification } from './daemon';
import { PeridotSidebarProvider } from './sidebar';

interface SessionStartResult {
  session_id: string;
}

interface DaemonEventParams {
  session_id?: string;
  event?: unknown;
}

interface ActiveRun {
  daemon: PeridotDaemon;
  sessionId?: string;
  disposeNotification: () => void;
  disposeExit: () => void;
}

let activeRun: ActiveRun | undefined;

export function activate(context: vscode.ExtensionContext) {
  const output = vscode.window.createOutputChannel('Peridot');
  context.subscriptions.push(output);
  let sidebar: PeridotSidebarProvider;
  sidebar = new PeridotSidebarProvider(context.extensionUri, {
    runTask: async (task: string): Promise<void> => runTask(task, output, sidebar),
    cancelTask: async (): Promise<void> => cancelTask(output),
  });
  context.subscriptions.push(
    vscode.window.registerWebviewViewProvider(PeridotSidebarProvider.viewType, sidebar),
  );

  // Sanity command — exists purely so a user can verify the
  // extension installed correctly without spawning the daemon.
  context.subscriptions.push(
    vscode.commands.registerCommand('peridot.hello', async () => {
      await vscode.window.showInformationMessage(
        'Hello from Peridot Agent — extension installed correctly.',
      );
    }),
  );

  // Round-trips the daemon to confirm the JSON-RPC pipeline works.
  // Surfaces both the extension and daemon versions to the operator
  // so version mismatches between the .vsix and the bundled binary
  // are visible immediately.
  context.subscriptions.push(
    vscode.commands.registerCommand('peridot.checkVersion', async () => {
      const folder = vscode.workspace.workspaceFolders?.[0]?.uri.fsPath;
      if (!folder) {
        vscode.window.showWarningMessage(
          'Open a workspace folder before checking the Peridot daemon.',
        );
        return;
      }
      try {
        const daemon = await PeridotDaemon.spawn(folder);
        try {
          const result = (await daemon.send('peridot.version')) as { version: string };
          const extensionVersion =
            vscode.extensions.getExtension('dlsxj101.peridot-vscode')?.packageJSON?.version ??
            'unknown';
          await vscode.window.showInformationMessage(
            `Peridot daemon ${result.version} (extension ${extensionVersion}).`,
          );
        } finally {
          await daemon.shutdown();
        }
      } catch (err) {
        const message = err instanceof Error ? err.message : String(err);
        await vscode.window.showErrorMessage(`Peridot daemon spawn failed: ${message}`);
      }
    }),
  );

  context.subscriptions.push(
    vscode.commands.registerCommand('peridot.runTask', async () => {
      await runTask(undefined, output, sidebar);
    }),
  );

  context.subscriptions.push(
    vscode.commands.registerCommand('peridot.cancelTask', async () => {
      await cancelTask(output);
    }),
  );
}

export async function deactivate() {
  if (activeRun) {
    await finishActiveRun();
  }
}

async function runTask(
  providedTask: string | undefined,
  output: vscode.OutputChannel,
  sidebar: PeridotSidebarProvider,
): Promise<void> {
  if (activeRun) {
    await vscode.window.showWarningMessage(
      'Peridot is already running a task. Cancel or wait for it to finish first.',
    );
    output.show(true);
    return;
  }

  const folder = vscode.workspace.workspaceFolders?.[0]?.uri.fsPath;
  if (!folder) {
    vscode.window.showWarningMessage('Open a workspace folder before running Peridot.');
    return;
  }

  const task =
    providedTask ??
    (await vscode.window.showInputBox({
      title: 'Peridot: Run Task',
      prompt: 'Describe the coding task for Peridot to run in this workspace.',
      ignoreFocusOut: true,
    }));
  if (!task || task.trim().length === 0) {
    return;
  }

  const trimmedTask = task.trim();
  output.clear();
  output.show(true);
  output.appendLine(`[peridot] starting daemon for ${folder}`);
  sidebar.resetForTask(trimmedTask, folder);

  let daemon: PeridotDaemon | undefined;
  let disposeNotification: (() => void) | undefined;
  let disposeExit: (() => void) | undefined;
  try {
    const spawned = await PeridotDaemon.spawn(folder);
    daemon = spawned;
    disposeNotification = daemon.onNotification((notification) => {
      void handleDaemonNotification(notification, output, sidebar);
    });
    disposeExit = daemon.onExit((exit) => {
      output.appendLine(
        `[peridot] daemon exited: code=${exit.code ?? 'null'} signal=${
          exit.signal ?? 'null'
        }`,
      );
      if (activeRun?.daemon === spawned) {
        sidebar.appendError('Daemon exited before the session finished.');
      }
      clearActiveRun(spawned);
    });
    const run: ActiveRun = { daemon, disposeNotification, disposeExit };
    activeRun = run;

    const result = (await daemon.send('session.start', {
      task: trimmedTask,
    })) as SessionStartResult;
    run.sessionId = result.session_id;
    output.appendLine(`[peridot] session started: ${result.session_id}`);
    sidebar.setSession(result.session_id);
  } catch (err) {
    disposeNotification?.();
    disposeExit?.();
    if (daemon) {
      await daemon.shutdown();
    }
    activeRun = undefined;
    const message = err instanceof Error ? err.message : String(err);
    output.appendLine(`[peridot] failed: ${message}`);
    sidebar.appendError(message);
    await vscode.window.showErrorMessage(`Peridot run failed: ${message}`);
  }
}

async function cancelTask(output: vscode.OutputChannel): Promise<void> {
  if (!activeRun) {
    await vscode.window.showInformationMessage('Peridot is not running a task.');
    return;
  }
  output.show(true);
  const run = activeRun;
  const sessionId = run.sessionId;
  if (!sessionId) {
    output.appendLine('[peridot] cancelling daemon before session id was assigned');
    await finishActiveRun(output);
    return;
  }
  try {
    const result = (await run.daemon.send('session.cancel', {
      session_id: sessionId,
    })) as { cancelled: boolean; session_id: string };
    output.appendLine(
      `[peridot] cancel requested for ${result.session_id}: ${result.cancelled}`,
    );
  } catch (err) {
    const message = err instanceof Error ? err.message : String(err);
    output.appendLine(`[peridot] cancel failed: ${message}`);
    await vscode.window.showErrorMessage(`Peridot cancel failed: ${message}`);
  }
}

async function handleDaemonNotification(
  notification: RpcNotification,
  output: vscode.OutputChannel,
  sidebar: PeridotSidebarProvider,
): Promise<void> {
  if (notification.method !== 'event') {
    output.appendLine(
      `[peridot] notification ${notification.method}: ${json(notification.params)}`,
    );
    sidebar.appendSystem(`Notification ${notification.method}`, json(notification.params));
    return;
  }

  const params: DaemonEventParams = isRecord(notification.params)
    ? (notification.params as DaemonEventParams)
    : {};
  const sessionId = params.session_id ?? 'unknown-session';
  const event = params.event;
  output.appendLine(formatEvent(sessionId, event));
  sidebar.appendNotification(params);

  if (isTerminalEvent(event)) {
    await finishActiveRun(output);
  }
}

async function finishActiveRun(output?: vscode.OutputChannel): Promise<void> {
  const run = activeRun;
  if (!run) {
    return;
  }
  activeRun = undefined;
  disposeRun(run);
  try {
    await run.daemon.shutdown();
  } catch (err) {
    const message = err instanceof Error ? err.message : String(err);
    output?.appendLine(`[peridot] daemon shutdown failed: ${message}`);
  }
}

function clearActiveRun(daemon: PeridotDaemon): void {
  const run = activeRun;
  if (!run || run.daemon !== daemon) {
    return;
  }
  activeRun = undefined;
  disposeRun(run);
}

function disposeRun(run: ActiveRun): void {
  run.disposeNotification();
  run.disposeExit();
}

function formatEvent(sessionId: string, event: unknown): string {
  if (!isRecord(event)) {
    return `[${sessionId}] event ${json(event)}`;
  }

  const kind = typeof event.kind === 'string' ? event.kind : 'unknown';
  switch (kind) {
    case 'started':
    case 'run_started':
      return `[${sessionId}] ${kind}: ${stringField(event, 'task')}`;
    case 'assistant_delta':
      return `[${sessionId}] assistant: ${stringField(event, 'delta')}`;
    case 'tool_started':
      return `[${sessionId}] tool started: ${stringField(event, 'name')}`;
    case 'tool_finished':
      return `[${sessionId}] tool finished: ${stringField(event, 'name')}`;
    case 'finished':
      return `[${sessionId}] finished: ${json(event)}`;
    case 'error':
      return `[${sessionId}] error: ${stringField(event, 'message')}`;
    default:
      return `[${sessionId}] ${kind}: ${json(event)}`;
  }
}

function isTerminalEvent(event: unknown): boolean {
  return isRecord(event) && (event.kind === 'finished' || event.kind === 'error');
}

function stringField(record: Record<string, unknown>, key: string): string {
  const value = record[key];
  return typeof value === 'string' ? value : json(value);
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null;
}

function json(value: unknown): string {
  try {
    const serialized = JSON.stringify(value);
    return serialized === undefined ? String(value) : serialized;
  } catch {
    return String(value);
  }
}
