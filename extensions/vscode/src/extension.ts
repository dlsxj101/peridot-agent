// Peridot Agent — VS Code extension entry point.
//
// Bridge surface: sidebar chat, daemon status checks, login handoff, and
// task execution over JSON-RPC.

import * as childProcess from 'child_process';
import * as vscode from 'vscode';
import { PeridotDaemon, RpcNotification } from './daemon';
import { resetBinaryCache, resolvePeridotBinary } from './peridotBin';
import { peridotChildEnv } from './processEnv';
import { StatusCache } from './statusCache';
import type { CommandResultView, SlashCommandSpec } from './types';
import {
  PeridotSidebarProvider,
  type ApprovalResponse,
  type AskUserAnswer,
  type ProviderChoice,
  type RunOptions,
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

interface SlashCommandCatalogResult {
  commands?: Array<{
    name?: string;
    description?: string;
    arg_hint?: string | null;
    category?: string;
  }>;
}

interface ActiveRun {
  daemon: PeridotDaemon;
  clientSessionId: string;
  sessionId?: string;
  disposeNotification: () => void;
  disposeExit: () => void;
}

let activeRun: ActiveRun | undefined;
let statusCache: StatusCache<DaemonStatusResult> | undefined;
let cachedFolder: string | undefined;

const OPENAI_OAUTH_DEFAULT_MODEL = 'gpt-5.5';
const OPENAI_OAUTH_BASE_URL = 'https://chatgpt.com/backend-api/codex';

export function activate(context: vscode.ExtensionContext) {
  const output = vscode.window.createOutputChannel('Peridot');
  context.subscriptions.push(output);
  const sidebar: PeridotSidebarProvider = new PeridotSidebarProvider(context.extensionUri, context.workspaceState, {
    runTask: async (task: string, options: RunOptions): Promise<void> =>
      runTask(task, output, sidebar, options),
    runSlashCommand: async (command: string, options: RunOptions): Promise<CommandResultView> =>
      runSlashCommand(command, output, sidebar, options),
    cancelTask: async (): Promise<void> => cancelTask(output, sidebar),
    clearSession: async (): Promise<void> => clearExtensionSession(output),
    loginOpenAi: async (): Promise<void> => loginOpenAi(output, sidebar),
    refreshStatus: async (): Promise<void> => refreshStatus(output, sidebar, { force: true }),
    respondAskUser: async (requestId: string, answer: AskUserAnswer): Promise<void> =>
      respondAskUser(requestId, answer, output, sidebar),
    respondApproval: async (decision: ApprovalResponse): Promise<void> =>
      respondApproval(decision, output, sidebar),
    openFile: async (relativePath: string, line?: number, column?: number): Promise<void> =>
      openWorkspaceFile(relativePath, output, line, column),
    registerProvider: async (
      provider: ProviderChoice,
      params: Record<string, string>,
    ): Promise<void> => registerProvider(provider, params, output, sidebar),
  });
  context.subscriptions.push(
    vscode.window.registerWebviewViewProvider(PeridotSidebarProvider.viewType, sidebar, {
      webviewOptions: { retainContextWhenHidden: true },
    }),
  );
  context.subscriptions.push(
    vscode.workspace.onDidChangeWorkspaceFolders(() => {
      invalidateStatusCache();
      void refreshStatus(output, sidebar, { force: true });
    }),
  );
  context.subscriptions.push(
    vscode.workspace.onDidChangeConfiguration((event) => {
      if (event.affectsConfiguration('peridot.binaryPath')) {
        resetBinaryCache();
        invalidateStatusCache();
        void refreshStatus(output, sidebar, { force: true });
      }
    }),
  );
  void refreshStatus(output, sidebar);

  context.subscriptions.push(
    vscode.commands.registerCommand('peridot.hello', async () => {
      await vscode.window.showInformationMessage(
        'Hello from Peridot Agent — extension installed correctly.',
      );
    }),
  );

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
          await refreshStatus(output, sidebar, { force: true });
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
      await cancelTask(output, sidebar);
    }),
  );

  context.subscriptions.push(
    vscode.commands.registerCommand('peridot.loginOpenAi', async () => {
      await loginOpenAi(output, sidebar);
    }),
  );

  context.subscriptions.push(
    vscode.commands.registerCommand('peridot.refreshStatus', async () => {
      await refreshStatus(output, sidebar, { force: true });
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
  output.appendLine(`[peridot] starting daemon for ${folder}`);
  const prepared = sidebar.prepareForTask(trimmedTask, folder);

  let daemon: PeridotDaemon | undefined;
  let disposeNotification: (() => void) | undefined;
  let disposeExit: (() => void) | undefined;
  try {
    const spawned = await PeridotDaemon.spawn(folder);
    daemon = spawned;
    disposeNotification = daemon.onNotification((notification) => {
      void handleDaemonNotification(notification, output, sidebar, prepared.clientSessionId);
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
    const run: ActiveRun = {
      daemon,
      clientSessionId: prepared.clientSessionId,
      disposeNotification,
      disposeExit,
    };
    activeRun = run;

    const result = (await daemon.send('session.start', {
      task: trimmedTask,
      mode: options.mode,
      permission: options.permission,
      ...(prepared.continueSessionId ? { session_id: prepared.continueSessionId } : {}),
      ...(options.model ? { model: options.model } : {}),
      ...(options.reasoningEffort ? { reasoning_effort: options.reasoningEffort } : {}),
      ...(options.serviceTier ? { service_tier: options.serviceTier } : {}),
    })) as SessionStartResult;
    run.sessionId = result.session_id;
    output.appendLine(`[peridot] session started: ${result.session_id}`);
    sidebar.setSession(result.session_id);
    void refreshStatus(output, sidebar, { force: true });
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

async function runSlashCommand(
  command: string,
  output: vscode.OutputChannel,
  sidebar: PeridotSidebarProvider,
  _options: RunOptions,
): Promise<CommandResultView> {
  const folder = vscode.workspace.workspaceFolders?.[0]?.uri.fsPath;
  if (!folder) {
    throw new Error('Open a workspace folder before running Peridot commands.');
  }
  const sessionId = activeRun?.sessionId ?? sidebar.currentDaemonSessionId();
  const params = {
    command,
    ...(sessionId ? { session_id: sessionId } : {}),
  };
  if (activeRun?.daemon) {
    output.appendLine(`[peridot] session.command ${command}`);
    return asCommandResult(await activeRun.daemon.send('session.command', params));
  }

  output.appendLine(`[peridot] session.command (spawn) ${command}`);
  const daemon = await PeridotDaemon.spawn(folder);
  try {
    return asCommandResult(await daemon.send('session.command', params));
  } finally {
    await daemon.shutdown();
  }
}

function asCommandResult(value: unknown): CommandResultView {
  if (typeof value === 'object' && value !== null) {
    return value as CommandResultView;
  }
  return {
    kind: 'message',
    title: 'Command',
    message: String(value),
    severity: 'info',
  };
}

interface RefreshOptions {
  force?: boolean;
}

async function refreshStatus(
  output: vscode.OutputChannel,
  sidebar: PeridotSidebarProvider,
  options: RefreshOptions = {},
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

  if (folder !== cachedFolder) {
    invalidateStatusCache();
    cachedFolder = folder;
  }
  if (!statusCache) {
    statusCache = new StatusCache<DaemonStatusResult>(() => fetchStatus(folder, output));
  }

  try {
    const result = await statusCache.get(options.force ?? false);
    void fetchSlashCatalog(folder, output)
      .then((commands) => sidebar.setSlashCommands(commands))
      .catch((err) => {
        const message = err instanceof Error ? err.message : String(err);
        output.appendLine(`[peridot] slash catalog failed: ${message}`);
      });
    const extensionVersion =
      vscode.extensions.getExtension('dlsxj101.peridot-vscode')?.packageJSON?.version ??
      'unknown';
    sidebar.setContext({
      workspace: result.project_root,
      provider: result.provider,
      model: result.model,
      reasoningEffort: result.reasoning_effort,
      serviceTier: sidebar.currentRunOptions().serviceTier,
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
  }
}

async function fetchSlashCatalog(
  folder: string,
  output: vscode.OutputChannel,
): Promise<SlashCommandSpec[]> {
  if (activeRun?.daemon) {
    return normalizeSlashCatalog(
      (await activeRun.daemon.send('session.command_catalog')) as SlashCommandCatalogResult,
    );
  }
  output.appendLine(`[peridot] slash catalog fetch (spawn) for ${folder}`);
  const daemon = await PeridotDaemon.spawn(folder);
  try {
    return normalizeSlashCatalog(
      (await daemon.send('session.command_catalog')) as SlashCommandCatalogResult,
    );
  } finally {
    await daemon.shutdown();
  }
}

function normalizeSlashCatalog(result: SlashCommandCatalogResult): SlashCommandSpec[] {
  const commands = Array.isArray(result.commands) ? result.commands : [];
  return commands
    .filter((entry) => typeof entry.name === 'string' && typeof entry.description === 'string')
    .map((entry) => ({
      name: entry.name as string,
      description: entry.description as string,
      ...(typeof entry.arg_hint === 'string' ? { argHint: entry.arg_hint } : {}),
      ...(typeof entry.category === 'string' ? { category: entry.category } : {}),
    }));
}

async function fetchStatus(
  folder: string,
  output: vscode.OutputChannel,
): Promise<DaemonStatusResult> {
  // Reuse the long-lived daemon when a session is active so we don't
  // double-spawn just to read context.
  if (activeRun?.daemon) {
    return (await activeRun.daemon.send('peridot.status')) as DaemonStatusResult;
  }
  output.appendLine(`[peridot] status fetch (spawn) for ${folder}`);
  const daemon = await PeridotDaemon.spawn(folder);
  try {
    return (await daemon.send('peridot.status')) as DaemonStatusResult;
  } finally {
    await daemon.shutdown();
  }
}

function invalidateStatusCache(): void {
  statusCache?.invalidate();
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
      chatGptLoginProcessOptions(output, sidebar),
    );
    await configureChatGptDefaults(binary, folder, output);
    sidebar.appendSystem('ChatGPT login completed');
    invalidateStatusCache();
    await refreshStatus(output, sidebar, { force: true });
    await vscode.window.showInformationMessage('Peridot ChatGPT login completed.');
  } catch (err) {
    const message = err instanceof Error ? err.message : String(err);
    output.appendLine(`[peridot] login failed: ${message}`);
    sidebar.appendError(`ChatGPT login failed: ${message}`);
    await vscode.window.showErrorMessage(`Peridot login failed: ${message}`);
  }
}

interface RunProcessOptions {
  env?: NodeJS.ProcessEnv;
  onStdoutLine?: (line: string) => void;
}

function runProcess(
  command: string,
  args: string[],
  cwd: string | undefined,
  output: vscode.OutputChannel,
  options: RunProcessOptions = {},
): Promise<void> {
  return new Promise((resolve, reject) => {
    const child = childProcess.spawn(command, args, {
      cwd,
      env: { ...peridotChildEnv(), ...options.env },
      stdio: ['ignore', 'pipe', 'pipe'],
    });
    // Capture stderr separately so we can surface the last few lines on
    // failure — the Output channel doesn't auto-show anymore and most
    // users won't think to open it.
    let stderrBuf = '';
    let stdoutLineBuf = '';
    child.stdout.on('data', (chunk: Buffer) => {
      const text = chunk.toString();
      output.append(text);
      if (options.onStdoutLine) {
        stdoutLineBuf += text;
        const lines = stdoutLineBuf.split(/\r?\n/);
        stdoutLineBuf = lines.pop() ?? '';
        for (const line of lines) options.onStdoutLine(line);
      }
    });
    child.stderr.on('data', (chunk: Buffer) => {
      const text = chunk.toString();
      stderrBuf += text;
      output.append(text);
    });
    child.on('error', reject);
    child.on('exit', (code, signal) => {
      if (options.onStdoutLine && stdoutLineBuf.length > 0) {
        options.onStdoutLine(stdoutLineBuf);
        stdoutLineBuf = '';
      }
      if (code === 0) {
        resolve();
        return;
      }
      // Tail the last ~6 non-blank stderr lines into the error so the
      // sidebar surface explains *what* failed, not just *that* it did.
      const tail = stderrBuf
        .split(/\r?\n/)
        .map((line) => line.trim())
        .filter((line) => line.length > 0)
        .slice(-6)
        .join('\n');
      const exitLabel = `process exited with code=${code ?? 'null'} signal=${signal ?? 'null'}`;
      reject(new Error(tail ? `${exitLabel}\n${tail}` : exitLabel));
    });
  });
}

// `peridot env set <KEY>` reads the value from stdin when no positional
// value is provided — preferred for API keys so they never appear in argv
// or the Output channel.
function runProcessWithStdin(
  command: string,
  args: string[],
  cwd: string | undefined,
  output: vscode.OutputChannel,
  stdinValue: string,
): Promise<void> {
  return new Promise((resolve, reject) => {
    const child = childProcess.spawn(command, args, {
      cwd,
      env: peridotChildEnv(),
      stdio: ['pipe', 'pipe', 'pipe'],
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
    child.stdin.write(stdinValue);
    child.stdin.end();
  });
}

function extractOpenAiAuthUrl(line: string): string | undefined {
  const match = line.match(/https:\/\/auth\.openai\.com\/oauth\/authorize[^\s]+/);
  return match?.[0];
}

function chatGptLoginProcessOptions(
  output: vscode.OutputChannel,
  sidebar: PeridotSidebarProvider,
): RunProcessOptions {
  let surfacedUrl: string | undefined;
  return {
    env: { PERIDOT_DISABLE_BROWSER_OPEN: '1' },
    onStdoutLine: (line) => {
      const authUrl = extractOpenAiAuthUrl(line);
      if (!authUrl) return;
      if (surfacedUrl !== authUrl) {
        surfacedUrl = authUrl;
        sidebar.appendSystem('Opening ChatGPT login in your browser');
        sidebar.appendAuthLink(authUrl);
      }
      output.appendLine(`[peridot] opening ChatGPT login URL via Cursor: ${authUrl}`);
      void vscode.env.openExternal(vscode.Uri.parse(authUrl)).then((opened) => {
        if (!opened) {
          output.appendLine(`[peridot] Cursor could not open browser: ${authUrl}`);
          void vscode.window.showWarningMessage(
            'Peridot could not open the ChatGPT login page automatically. Use the login link shown in the Peridot chat.',
          );
        }
      });
    },
  };
}

async function cancelTask(
  output: vscode.OutputChannel,
  sidebar?: PeridotSidebarProvider,
): Promise<void> {
  if (!activeRun) {
    await vscode.window.showInformationMessage('Peridot is not running a task.');
    return;
  }
  const run = activeRun;
  const sessionId = run.sessionId;
  if (!sessionId) {
    output.appendLine('[peridot] cancelling daemon before session id was assigned');
    await finishActiveRun(output);
    sidebar?.markIdle('Cancelled');
    return;
  }
  try {
    const result = (await run.daemon.send('session.cancel', {
      session_id: sessionId,
    })) as { cancelled: boolean; session_id: string };
    output.appendLine(
      `[peridot] cancel requested for ${result.session_id}: ${result.cancelled}`,
    );
    if (result.cancelled) {
      sidebar?.appendSystem('Cancelled');
      sidebar?.markIdle('Cancelled');
      await finishActiveRun(output);
    }
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
  const sessionId = decision.sessionId ?? activeRun?.sessionId;
  if (!sessionId || !activeRun?.daemon) {
    const message = 'No active Peridot run can receive this approval decision.';
    sidebar.appendError(message);
    await vscode.window.showWarningMessage(message);
    return;
  }
  try {
    const result = (await activeRun.daemon.send('approval.respond', {
      session_id: sessionId,
      approved: decision.approved,
      scope: decision.scope,
      tool_name: decision.toolName,
      reason: decision.reason,
      parameters: decision.parameters,
    })) as { accepted?: boolean; resumed?: boolean; session_id?: string; message?: string };
    output.appendLine(
      `[peridot] approval ${result.session_id ?? sessionId}: ${
        result.accepted ? 'accepted' : 'not accepted'
      }${result.resumed ? ' resumed' : ''}`,
    );
    if (!result.accepted) {
      sidebar.appendError(result.message ?? 'Peridot did not accept that approval decision.');
      return;
    }
    if (!decision.approved) {
      await finishActiveRun(output);
      void refreshStatus(output, sidebar, { force: true });
    }
  } catch (err) {
    const message = err instanceof Error ? err.message : String(err);
    output.appendLine(`[peridot] approval response failed: ${message}`);
    sidebar.appendError(`Approval response failed: ${message}`);
  }
}

async function registerProvider(
  provider: ProviderChoice,
  params: Record<string, string>,
  output: vscode.OutputChannel,
  sidebar: PeridotSidebarProvider,
): Promise<void> {
  if (activeRun) {
    await vscode.window.showWarningMessage(
      'Cancel or wait for the current Peridot task before switching providers.',
    );
    return;
  }
  const folder = vscode.workspace.workspaceFolders?.[0]?.uri.fsPath;
  if (!folder) {
    const message = 'Open a workspace folder before configuring a provider.';
    output.appendLine(`[peridot] ${message}`);
    sidebar.setWorkspaceProblem(message);
    sidebar.appendError(message);
    await vscode.window.showWarningMessage(message);
    return;
  }
  sidebar.setAuthBusy(true, undefined);
  try {
    const binary = await resolvePeridotBinary();
    switch (provider) {
      case 'chatgpt':
        // ChatGPT login goes through the dedicated OAuth flow which writes
        // its own auth file. Still flip auth.primary so the daemon picks
        // openai-oauth on the next status read, and reset the model away
        // from any prior Claude/OpenRouter selection.
        await runProcess(
          binary,
          ['login', 'openai-oauth'],
          folder,
          output,
          chatGptLoginProcessOptions(output, sidebar),
        );
        await configureChatGptDefaults(binary, folder, output);
        break;
      case 'claude': {
        const apiKey = (params.apiKey ?? '').trim();
        if (!apiKey) throw new Error('Anthropic API key is required.');
        await runProcessWithStdin(
          binary,
          ['env', 'set', 'ANTHROPIC_API_KEY'],
          folder,
          output,
          apiKey,
        );
        // Reset api.base_url so a previously-configured local-LLM URL
        // doesn't leak into the Anthropic path. The provider catalog
        // already falls back to api.anthropic.com when the canonical
        // default is in place.
        await runProcess(
          binary,
          ['config', 'set', 'api.base_url', 'https://api.openai.com'],
          folder,
          output,
        );
        await runProcess(
          binary,
          ['config', 'set', 'auth.primary', 'claude-api'],
          folder,
          output,
        );
        if (params.model && params.model.trim().length > 0) {
          await runProcess(
            binary,
            ['config', 'set', 'models.main', params.model.trim()],
            folder,
            output,
          );
        }
        break;
      }
      case 'openai': {
        const apiKey = (params.apiKey ?? '').trim();
        if (!apiKey) throw new Error('OpenAI API key is required.');
        await runProcessWithStdin(
          binary,
          ['env', 'set', 'OPENAI_API_KEY'],
          folder,
          output,
          apiKey,
        );
        await runProcess(
          binary,
          ['config', 'set', 'api.base_url', 'https://openrouter.ai/api'],
          folder,
          output,
        );
        await runProcess(
          binary,
          ['config', 'set', 'auth.primary', 'openai-api'],
          folder,
          output,
        );
        if (params.model && params.model.trim().length > 0) {
          await runProcess(
            binary,
            ['config', 'set', 'models.main', params.model.trim()],
            folder,
            output,
          );
        }
        break;
      }
      case 'openrouter': {
        const apiKey = (params.apiKey ?? '').trim();
        if (!apiKey) throw new Error('OpenRouter API key is required.');
        await runProcessWithStdin(
          binary,
          ['env', 'set', 'OPENROUTER_API_KEY'],
          folder,
          output,
          apiKey,
        );
        await runProcess(
          binary,
          ['config', 'set', 'api.base_url', 'https://api.anthropic.com'],
          folder,
          output,
        );
        await runProcess(
          binary,
          ['config', 'set', 'auth.primary', 'openrouter-api'],
          folder,
          output,
        );
        if (params.model && params.model.trim().length > 0) {
          await runProcess(
            binary,
            ['config', 'set', 'models.main', params.model.trim()],
            folder,
            output,
          );
        }
        break;
      }
      case 'localLlm': {
        const apiKey = (params.apiKey ?? '').trim() || 'local';
        const baseUrl = (params.baseUrl ?? '').trim();
        if (!baseUrl) throw new Error('Local LLM endpoint URL is required.');
        await runProcessWithStdin(binary, ['env', 'set', 'OPENAI_API_KEY'], folder, output, apiKey);
        await runProcess(
          binary,
          ['config', 'set', 'api.base_url', baseUrl],
          folder,
          output,
        );
        await runProcess(
          binary,
          ['config', 'set', 'auth.primary', 'openai-api'],
          folder,
          output,
        );
        if (params.model && params.model.trim().length > 0) {
          await runProcess(
            binary,
            ['config', 'set', 'models.main', params.model.trim()],
            folder,
            output,
          );
        }
        break;
      }
    }
    invalidateStatusCache();
    await refreshStatus(output, sidebar, { force: true });
    sidebar.setAuthBusy(false, '');
    sidebar.appendSystem(`Configured ${provider}.`);
  } catch (err) {
    const message = err instanceof Error ? err.message : String(err);
    output.appendLine(`[peridot] registerProvider ${provider} failed: ${message}`);
    sidebar.setAuthBusy(false, message);
  }
}

async function configureChatGptDefaults(
  binary: string,
  folder: string | undefined,
  output: vscode.OutputChannel,
): Promise<void> {
  await runProcess(binary, ['config', 'set', 'auth.primary', 'openai-oauth'], folder, output);
  await runProcess(binary, ['config', 'set', 'api.base_url', OPENAI_OAUTH_BASE_URL], folder, output);
  await runProcess(binary, ['config', 'set', 'models.main', OPENAI_OAUTH_DEFAULT_MODEL], folder, output);
}

async function openWorkspaceFile(
  relativePath: string,
  output: vscode.OutputChannel,
  line?: number,
  column?: number,
  openOptions?: { beside?: boolean; preview?: boolean },
): Promise<void> {
  const folder = vscode.workspace.workspaceFolders?.[0];
  if (!folder) {
    output.appendLine(`[peridot] openFile ignored — no workspace open: ${relativePath}`);
    return;
  }
  try {
    const uri = relativePath.startsWith('/') || /^[A-Za-z]:[\\/]/.test(relativePath)
      ? vscode.Uri.file(relativePath)
      : vscode.Uri.joinPath(folder.uri, relativePath);
    const selectionOptions =
      typeof line === 'number'
        ? {
            selection: new vscode.Range(
              Math.max(0, line - 1),
              Math.max(0, (column ?? 1) - 1),
              Math.max(0, line - 1),
              Math.max(0, (column ?? 1) - 1),
            ),
          }
        : undefined;
    const document = await vscode.workspace.openTextDocument(uri);
    await vscode.window.showTextDocument(document, {
      ...selectionOptions,
      preview: openOptions?.preview ?? false,
      viewColumn:
        openOptions?.beside && vscode.window.activeTextEditor
          ? vscode.ViewColumn.Beside
          : vscode.ViewColumn.Active,
    });
  } catch (err) {
    const message = err instanceof Error ? err.message : String(err);
    output.appendLine(`[peridot] openFile failed: ${message}`);
  }
}

async function clearExtensionSession(output: vscode.OutputChannel): Promise<void> {
  if (activeRun?.sessionId) {
    try {
      await activeRun.daemon.send('session.cancel', { session_id: activeRun.sessionId });
    } catch (err) {
      const message = err instanceof Error ? err.message : String(err);
      output.appendLine(`[peridot] clear cancel failed: ${message}`);
    }
  }
  await finishActiveRun(output);
}

async function handleDaemonNotification(
  notification: RpcNotification,
  output: vscode.OutputChannel,
  sidebar: PeridotSidebarProvider,
  clientSessionId?: string,
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
  sidebar.appendNotificationFor(clientSessionId, params);
  const planDocumentPath = planDocumentPathFromEvent(event);
  if (planDocumentPath) {
    await openWorkspaceFile(planDocumentPath, output, undefined, undefined, {
      beside: true,
      preview: false,
    });
  }

  if (isTerminalEvent(event)) {
    await finishActiveRun(output);
    void refreshStatus(output, sidebar, { force: true });
    drainQueue(output, sidebar);
  }
}

function drainQueue(output: vscode.OutputChannel, sidebar: PeridotSidebarProvider): void {
  if (!sidebar.hasQueue()) return;
  const next = sidebar.takeNextQueued();
  if (!next) return;
  output.appendLine(`[peridot] auto-dispatching next queued task (${next.id})`);
  // Run the next queued task on a microtask boundary so the current
  // terminal event finishes propagating to the webview first. Reuses the
  // same RunOptions the operator picked for the previous turn.
  setTimeout(() => {
    void runTask(next.text, output, sidebar, sidebar.currentRunOptions());
  }, 50);
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

function planDocumentPathFromEvent(event: unknown): string | undefined {
  if (!isRecord(event) || event.kind !== 'tool_finished') return undefined;
  const name = typeof event.name === 'string' ? event.name : '';
  if (name !== 'file_write' && name !== 'file_patch') return undefined;
  const output = isRecord(event.output) ? event.output : undefined;
  const path = typeof output?.path === 'string' ? output.path : undefined;
  if (!path || !isPlanDocumentPath(path)) return undefined;
  return path;
}

function isPlanDocumentPath(path: string): boolean {
  const normalized = path.replace(/\\/g, '/').toLowerCase();
  const basename = normalized.split('/').pop() ?? normalized;
  if (basename === 'todo.md' || basename === 'todo.markdown') return false;
  if (!basename.endsWith('.md') && !basename.endsWith('.markdown')) return false;
  return (
    basename.includes('plan') ||
    normalized.includes('/plans/') ||
    normalized.includes('/planning/')
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
