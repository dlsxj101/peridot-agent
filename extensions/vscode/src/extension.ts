// Peridot Agent — VS Code extension entry point.
//
// Bridge surface: sidebar chat, daemon status checks, login handoff, and
// task execution over JSON-RPC.

import * as childProcess from 'child_process';
import * as vscode from 'vscode';
import { PeridotDaemon, RpcNotification } from './daemon';
import { resolvePeridotBinary } from './peridotBin';
import {
  ApprovalResponse,
  AskUserAnswer,
  PeridotSidebarProvider,
  RunOptions,
} from './sidebar';

interface SessionStartResult {
  session_id: string;
}

interface DaemonEventParams {
  session_id?: string;
  event?: unknown;
}

interface DaemonStatusResult {
  version: string;
  project_root: string;
  provider: string;
  model: string;
  reasoning_effort?: string;
  mode?: string;
  permission?: string;
  auth?: {
    configured?: boolean;
    account_configured?: boolean;
    method?: string;
    source?: string;
  };
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
    runTask: async (task: string, options: RunOptions): Promise<void> =>
      runTask(task, output, sidebar, options),
    cancelTask: async (): Promise<void> => cancelTask(output),
    loginOpenAi: async (): Promise<void> => loginOpenAi(output, sidebar),
    refreshStatus: async (): Promise<void> => refreshStatus(output, sidebar),
    respondAskUser: async (requestId: string, answer: AskUserAnswer): Promise<void> =>
      respondAskUser(requestId, answer, output, sidebar),
    respondApproval: async (decision: ApprovalResponse): Promise<void> =>
      respondApproval(decision, output, sidebar),
  });
  context.subscriptions.push(
    vscode.window.registerWebviewViewProvider(PeridotSidebarProvider.viewType, sidebar),
  );
  context.subscriptions.push(
    vscode.workspace.onDidChangeWorkspaceFolders(() => {
      void refreshStatus(output, sidebar);
    }),
  );
  void refreshStatus(output, sidebar);

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
          await refreshStatus(output, sidebar);
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

  context.subscriptions.push(
    vscode.commands.registerCommand('peridot.loginOpenAi', async () => {
      await loginOpenAi(output, sidebar);
    }),
  );

  context.subscriptions.push(
    vscode.commands.registerCommand('peridot.refreshStatus', async () => {
      await refreshStatus(output, sidebar);
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
  options: RunOptions = { mode: 'execute', permission: 'auto' },
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
    sidebar.setWorkspaceProblem('Open a workspace folder before running Peridot.');
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
      mode: options.mode,
      permission: options.permission,
      ...(options.model ? { model: options.model } : {}),
    })) as SessionStartResult;
    run.sessionId = result.session_id;
    output.appendLine(`[peridot] session started: ${result.session_id}`);
    sidebar.setSession(result.session_id);
    void refreshStatus(output, sidebar);
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

async function refreshStatus(
  output: vscode.OutputChannel,
  sidebar: PeridotSidebarProvider,
): Promise<void> {
  const folder = vscode.workspace.workspaceFolders?.[0]?.uri.fsPath;
  if (!folder) {
    sidebar.setContext({
      status: 'No workspace',
      problem: 'Open a workspace folder to run Peridot.',
      running: Boolean(activeRun),
    });
    return;
  }

  let daemon: PeridotDaemon | undefined;
  try {
    daemon = await PeridotDaemon.spawn(folder);
    const result = (await daemon.send('peridot.status')) as DaemonStatusResult;
    const extensionVersion =
      vscode.extensions.getExtension('dlsxj101.peridot-vscode')?.packageJSON?.version ??
      'unknown';
    sidebar.setContext({
      workspace: result.project_root,
      provider: result.provider,
      model: result.model,
      mode: result.mode,
      permission: result.permission,
      daemonVersion: result.version,
      extensionVersion,
      authConfigured: Boolean(result.auth?.configured),
      authMethod: result.auth?.method,
      authSource: result.auth?.source,
      status: activeRun ? 'Running' : 'Idle',
      running: Boolean(activeRun),
    });
  } catch (err) {
    const message = err instanceof Error ? err.message : String(err);
    output.appendLine(`[peridot] status failed: ${message}`);
    sidebar.setContext({
      workspace: folder,
      status: activeRun ? 'Running' : 'Needs attention',
      problem: message,
      running: Boolean(activeRun),
    });
  } finally {
    if (daemon) {
      await daemon.shutdown();
    }
  }
}

async function loginOpenAi(
  output: vscode.OutputChannel,
  sidebar: PeridotSidebarProvider,
): Promise<void> {
  if (activeRun) {
    await vscode.window.showWarningMessage(
      'Cancel or wait for the current Peridot task before logging in.',
    );
    return;
  }

  const folder = vscode.workspace.workspaceFolders?.[0]?.uri.fsPath;
  output.show(true);
  sidebar.appendSystem('Starting ChatGPT login');
  sidebar.setContext({ status: 'Logging in', running: false });

  try {
    const binary = await resolvePeridotBinary();
    output.appendLine(`[peridot] login openai-oauth via ${binary}`);
    await runProcess(
      binary,
      ['login', 'openai-oauth'],
      folder,
      output,
    );
    sidebar.appendSystem('ChatGPT login completed');
    await refreshStatus(output, sidebar);
    await vscode.window.showInformationMessage('Peridot ChatGPT login completed.');
  } catch (err) {
    const message = err instanceof Error ? err.message : String(err);
    output.appendLine(`[peridot] login failed: ${message}`);
    sidebar.appendError(`ChatGPT login failed: ${message}`);
    await vscode.window.showErrorMessage(`Peridot login failed: ${message}`);
  }
}

function runProcess(
  command: string,
  args: string[],
  cwd: string | undefined,
  output: vscode.OutputChannel,
): Promise<void> {
  return new Promise((resolve, reject) => {
    const child = childProcess.spawn(command, args, {
      cwd,
      stdio: ['ignore', 'pipe', 'pipe'],
    });
    child.stdout.on('data', (chunk: Buffer) => {
      output.append(chunk.toString());
    });
    child.stderr.on('data', (chunk: Buffer) => {
      output.append(chunk.toString());
    });
    child.on('error', reject);
    child.on('exit', (code, signal) => {
      if (code === 0) {
        resolve();
      } else {
        reject(new Error(`process exited with code=${code ?? 'null'} signal=${signal ?? 'null'}`));
      }
    });
  });
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

async function respondAskUser(
  requestId: string,
  answer: AskUserAnswer,
  output: vscode.OutputChannel,
  sidebar: PeridotSidebarProvider,
): Promise<void> {
  if (!activeRun) {
    const message = 'No active Peridot run can receive this response.';
    sidebar.appendError(message);
    await vscode.window.showWarningMessage(message);
    return;
  }
  try {
    const result = (await activeRun.daemon.send('interaction.respond', {
      request_id: requestId,
      answer,
    })) as { accepted?: boolean; request_id?: string };
    output.appendLine(
      `[peridot] interaction response ${result.request_id ?? requestId}: ${
        result.accepted ? 'accepted' : 'not accepted'
      }`,
    );
    if (!result.accepted) {
      sidebar.appendError('Peridot did not accept that response. The run may have already moved on.');
    }
  } catch (err) {
    const message = err instanceof Error ? err.message : String(err);
    output.appendLine(`[peridot] interaction response failed: ${message}`);
    sidebar.appendError(`Interaction response failed: ${message}`);
  }
}

async function respondApproval(
  decision: ApprovalResponse,
  output: vscode.OutputChannel,
  sidebar: PeridotSidebarProvider,
): Promise<void> {
  if (!activeRun?.sessionId) {
    const message = 'No active Peridot run can receive this approval decision.';
    sidebar.appendError(message);
    await vscode.window.showWarningMessage(message);
    return;
  }
  try {
    const result = (await activeRun.daemon.send('approval.respond', {
      session_id: activeRun.sessionId,
      approved: decision.approved,
      scope: decision.scope,
      tool_name: decision.toolName,
      reason: decision.reason,
      parameters: decision.parameters,
    })) as { accepted?: boolean; resumed?: boolean; session_id?: string; message?: string };
    output.appendLine(
      `[peridot] approval ${result.session_id ?? activeRun.sessionId}: ${
        result.accepted ? 'accepted' : 'not accepted'
      }${result.resumed ? ' resumed' : ''}`,
    );
    if (!result.accepted) {
      sidebar.appendError(result.message ?? 'Peridot did not accept that approval decision.');
      return;
    }
    if (!decision.approved) {
      await finishActiveRun(output);
      void refreshStatus(output, sidebar);
    }
  } catch (err) {
    const message = err instanceof Error ? err.message : String(err);
    output.appendLine(`[peridot] approval response failed: ${message}`);
    sidebar.appendError(`Approval response failed: ${message}`);
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
    void refreshStatus(output, sidebar);
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
  return (
    isRecord(event) &&
    (event.kind === 'finished' || event.kind === 'error' || event.kind === 'approval_denied')
  );
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
