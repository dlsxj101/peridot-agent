// Peridot Agent — VS Code extension entry point.
//
// Bridge surface: sidebar chat, daemon status checks, login handoff, and
// task execution over JSON-RPC.

import * as childProcess from 'child_process';
import * as path from 'path';
import * as vscode from 'vscode';
import { EXPECTED_AGENT_RUN_EVENT_SCHEMA_VERSION, PeridotDaemon, RpcNotification } from './daemon';
import { resetBinaryCache, resolvePeridotBinary } from './peridotBin';
import { peridotChildEnv } from './processEnv';
import { SettingsPanelManager } from './settingsPanel';
import { StatusCache } from './statusCache';
import type { CommandResultView, DaemonSessionSummary, SlashCommandSpec } from './types';
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
  worktree_cleanup?: WorktreeCleanupResult;
}

interface WorktreeCleanupResult {
  suspended_sessions?: string[];
  removed_worktrees?: WorktreeCleanupItem[];
  preserved_worktrees?: WorktreeCleanupItem[];
  missing_worktrees?: WorktreeCleanupItem[];
  errors?: Array<{ session_id?: string; path?: string; message?: string }>;
}

interface WorktreeCleanupItem {
  session_id?: string;
  path?: string;
  branch?: string;
  reason?: string;
  changed_files?: number;
}

interface SlashCommandCatalogResult {
  commands?: Array<{
    name?: string;
    description?: string;
    arg_hint?: string | null;
    arg_options?: unknown;
    category?: string;
    surfaces?: unknown;
  }>;
}

interface SkillsListResult {
  skills?: Array<{
    name?: string;
    description?: string;
    scope?: string;
  }>;
}

interface SessionListResult {
  sessions?: DaemonSessionSummary[];
}

interface ActiveRun {
  daemon: PeridotDaemon;
  clientSessionId: string;
  sessionId?: string;
}

interface WorkspaceRun {
  folder: string;
  daemon: PeridotDaemon;
  disposeNotification: () => void;
  disposeExit: () => void;
  activeRuns: Map<string, ActiveRun>;
  keepAlive: boolean;
}

let workspaceRun: WorkspaceRun | undefined;
let statusCache: StatusCache<DaemonStatusResult> | undefined;
let cachedFolder: string | undefined;
/**
 * Module-level reference to the active sidebar provider. Set during
 * `activate()`. Helpers that need to reach the sidebar from outside the
 * activate() closure (e.g., the standalone `generateSessionTitle` helper)
 * read this instead of being passed the sidebar through every callsite.
 */
let activeSidebar: PeridotSidebarProvider | undefined;

const OPENAI_OAUTH_DEFAULT_MODEL = 'gpt-5.5';
const OPENAI_OAUTH_BASE_URL = 'https://chatgpt.com/backend-api/codex';
const CLAUDE_API_BASE_URL = 'https://api.anthropic.com';
const OPENAI_API_BASE_URL = 'https://api.openai.com';
const OPENROUTER_API_BASE_URL = 'https://openrouter.ai/api';

export function activate(context: vscode.ExtensionContext) {
  const output = vscode.window.createOutputChannel('Peridot');
  context.subscriptions.push(output);
  const sidebar: PeridotSidebarProvider = new PeridotSidebarProvider(context.extensionUri, context.workspaceState, {
    runTask: async (task: string, options: RunOptions): Promise<void> =>
      runTask(task, output, sidebar, options),
    runSlashCommand: async (command: string, options: RunOptions): Promise<CommandResultView> =>
      runSlashCommand(command, output, sidebar, options),
    cancelTask: async (): Promise<void> => cancelTask(output, sidebar),
    clearSession: async (options?: { skipDaemonCancel?: boolean }): Promise<void> =>
      clearExtensionSession(output, sidebar, options?.skipDaemonCancel === true),
    loginOpenAi: async (): Promise<void> => loginOpenAi(output, sidebar),
    refreshStatus: async (): Promise<void> => refreshStatus(output, sidebar, { force: true }),
    showCodeMap: async (): Promise<void> => showWorkspaceCodeMap(output, sidebar, false),
    showCodeMapStatus: async (): Promise<void> => showWorkspaceCodeMapStatus(output, sidebar),
    refreshCodeMap: async (): Promise<void> => showWorkspaceCodeMap(output, sidebar, true),
    searchCodeMap: async (): Promise<void> => searchWorkspaceCodeMap(output, sidebar),
    outlineCurrentFile: async (): Promise<void> => outlineCurrentFile(output, sidebar),
    findSymbolReferences: async (): Promise<void> => findWorkspaceSymbolReferences(output, sidebar),
    showSkills: async (): Promise<void> => showSkills(output, sidebar),
    showArchivedSkills: async (): Promise<void> => showArchivedSkills(output, sidebar),
    searchSkills: async (): Promise<void> => searchSkills(output, sidebar),
    searchArchivedSkills: async (): Promise<void> => searchArchivedSkills(output, sidebar),
    showSkill: async (name: string): Promise<void> => showSkill(name, output, sidebar),
    useSkill: async (name: string): Promise<void> => useSkill(name, output, sidebar),
    toggleSkillPin: async (name: string, pinned: boolean): Promise<void> =>
      toggleSkillPin(name, pinned, output, sidebar),
    archiveSkill: async (name: string): Promise<void> => archiveSkill(name, output, sidebar),
    restoreSkill: async (name: string): Promise<void> => restoreSkill(name, output, sidebar),
    attachFile: async (): Promise<void> => attachFileToSession(output, sidebar),
    detachAttachment: async (path: string): Promise<void> =>
      detachAttachmentFromSession(path, output, sidebar),
    showAttachments: async (): Promise<void> => showSessionAttachments(output, sidebar),
    exportSessionArtifacts: async (): Promise<void> => exportSessionArtifacts(output, sidebar),
    showPrStatus: async (): Promise<void> => showGitHubPrStatus(output, sidebar),
    shipChanges: async (): Promise<void> => shipChangesToPr(output, sidebar),
    mergePr: async (): Promise<void> => mergeGitHubPr(output, sidebar),
    respondAskUser: async (requestId: string, answer: AskUserAnswer): Promise<void> =>
      respondAskUser(requestId, answer, output, sidebar),
    respondApproval: async (decision: ApprovalResponse): Promise<void> =>
      respondApproval(decision, output, sidebar),
    openFile: async (relativePath: string, line?: number, column?: number, projectRoot?: string): Promise<void> =>
      openWorkspaceFile(relativePath, output, line, column, undefined, projectRoot),
    openPath: async (targetPath: string): Promise<void> => {
      await vscode.commands.executeCommand('revealFileInOS', vscode.Uri.file(targetPath));
    },
    registerProvider: async (
      provider: ProviderChoice,
      params: Record<string, string>,
    ): Promise<void> => registerProvider(provider, params, output, sidebar),
    deleteSession: async (clientSessionId: string, daemonSessionId?: string): Promise<void> =>
      deleteExtensionSession(clientSessionId, daemonSessionId, output),
    finishDaemonSession: async (daemonSessionId: string): Promise<void> =>
      finishRunBySession(daemonSessionId, output),
    copyText: async (text: string): Promise<void> => vscode.env.clipboard.writeText(text),
    generateSessionTitle: async (task: string): Promise<string | null> =>
      generateSessionTitle(task, output),
  });
  activeSidebar = sidebar;
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
  const memoryWatcher = vscode.workspace.createFileSystemWatcher('**/.peridot/memory.db');
  const refreshFromMemory = () => {
    void refreshSessionList(output, sidebar).catch((err: unknown) => {
      const message = err instanceof Error ? err.message : String(err);
      output.appendLine(`[peridot] session list refresh failed: ${message}`);
    });
  };
  context.subscriptions.push(
    memoryWatcher,
    memoryWatcher.onDidChange(refreshFromMemory),
    memoryWatcher.onDidCreate(refreshFromMemory),
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

  context.subscriptions.push(
    vscode.commands.registerCommand('peridot.showCodeMap', async () => {
      await showWorkspaceCodeMap(output, sidebar, false);
    }),
  );

  context.subscriptions.push(
    vscode.commands.registerCommand('peridot.showCodeMapStatus', async () => {
      await showWorkspaceCodeMapStatus(output, sidebar);
    }),
  );

  context.subscriptions.push(
    vscode.commands.registerCommand('peridot.refreshCodeMap', async () => {
      await showWorkspaceCodeMap(output, sidebar, true);
    }),
  );

  context.subscriptions.push(
    vscode.commands.registerCommand('peridot.searchCodeMap', async () => {
      await searchWorkspaceCodeMap(output, sidebar);
    }),
  );

  context.subscriptions.push(
    vscode.commands.registerCommand('peridot.locateCodeMapSymbol', async () => {
      await locateWorkspaceCodeMapSymbol(output, sidebar);
    }),
  );

  context.subscriptions.push(
    vscode.commands.registerCommand('peridot.outlineCurrentFile', async () => {
      await outlineCurrentFile(output, sidebar);
    }),
  );

  context.subscriptions.push(
    vscode.commands.registerCommand('peridot.findSymbolReferences', async () => {
      await findWorkspaceSymbolReferences(output, sidebar);
    }),
  );

  context.subscriptions.push(
    vscode.commands.registerCommand('peridot.showSkills', async () => {
      await showSkills(output, sidebar);
    }),
  );

  context.subscriptions.push(
    vscode.commands.registerCommand('peridot.showArchivedSkills', async () => {
      await showArchivedSkills(output, sidebar);
    }),
  );

  context.subscriptions.push(
    vscode.commands.registerCommand('peridot.searchSkills', async () => {
      await searchSkills(output, sidebar);
    }),
  );

  context.subscriptions.push(
    vscode.commands.registerCommand('peridot.searchArchivedSkills', async () => {
      await searchArchivedSkills(output, sidebar);
    }),
  );

  context.subscriptions.push(
    vscode.commands.registerCommand('peridot.attachFile', async () => {
      await attachFileToSession(output, sidebar);
    }),
  );

  context.subscriptions.push(
    vscode.commands.registerCommand('peridot.showAttachments', async () => {
      await showSessionAttachments(output, sidebar);
    }),
  );

  context.subscriptions.push(
    vscode.commands.registerCommand('peridot.exportSessionArtifacts', async () => {
      await exportSessionArtifacts(output, sidebar);
    }),
  );

  context.subscriptions.push(
    vscode.commands.registerCommand('peridot.prStatus', async () => {
      await showGitHubPrStatus(output, sidebar);
    }),
  );

  context.subscriptions.push(
    vscode.commands.registerCommand('peridot.shipChanges', async () => {
      await shipChangesToPr(output, sidebar);
    }),
  );

  context.subscriptions.push(
    vscode.commands.registerCommand('peridot.mergePr', async () => {
      await mergeGitHubPr(output, sidebar);
    }),
  );

  // Editor-area settings panel. Uses the workspace daemon to read+write
  // `.peridot/config.toml`. If no daemon is running yet (user hasn't
  // started a task), spawning one just to read settings is fine —
  // they'd hit the same cost on their first task anyway.
  const settingsPanel = new SettingsPanelManager(context.extensionUri, output, async () => {
    const folder = vscode.workspace.workspaceFolders?.[0]?.uri.fsPath;
    if (!folder) {
      void vscode.window.showErrorMessage(
        'Open a folder before editing Peridot settings — the daemon needs a project root.',
      );
      return null;
    }
    try {
      const ws = await ensureWorkspaceRun(folder, output, sidebar);
      return ws.daemon;
    } catch (err) {
      const message = err instanceof Error ? err.message : String(err);
      output.appendLine(`[peridot] settings: failed to reach daemon: ${message}`);
      return null;
    }
  });

  context.subscriptions.push(
    vscode.commands.registerCommand('peridot.openSettings', async () => {
      await settingsPanel.open();
    }),
  );
}

export async function deactivate() {
  if (workspaceRun) {
    await finishWorkspaceRun();
  }
}

async function ensureWorkspaceRun(
  folder: string,
  output: vscode.OutputChannel,
  sidebar: PeridotSidebarProvider,
): Promise<WorkspaceRun> {
  if (workspaceRun && workspaceRun.folder === folder) {
    return workspaceRun;
  }
  if (workspaceRun) {
    await finishWorkspaceRun(output);
  }
  const daemon = await PeridotDaemon.spawn(folder);
  const run: WorkspaceRun = {
    folder,
    daemon,
    activeRuns: new Map(),
    disposeNotification: () => undefined,
    disposeExit: () => undefined,
    keepAlive: false,
  };
  run.disposeNotification = daemon.onNotification((notification) => {
    void handleDaemonNotification(notification, output, sidebar);
  });
  daemon.onHandshake((handshake) => {
    output.appendLine(
      `[peridot] daemon handshake: schema_version=${handshake.schemaVersion} daemon_version=${handshake.daemonVersion}`,
    );
    if (handshake.schemaVersion !== EXPECTED_AGENT_RUN_EVENT_SCHEMA_VERSION) {
      const message =
        `Peridot daemon and VS Code extension are out of sync. ` +
        `Extension expects AgentRunEvent schema v${EXPECTED_AGENT_RUN_EVENT_SCHEMA_VERSION}, ` +
        `but daemon ${handshake.daemonVersion} reports v${handshake.schemaVersion}. ` +
        `Some events may not render correctly until both sides are updated.`;
      output.appendLine(`[peridot] ${message}`);
      void vscode.window.showWarningMessage(message);
    }
  });
  run.disposeExit = daemon.onExit((exit) => {
    output.appendLine(
      `[peridot] daemon exited: code=${exit.code ?? 'null'} signal=${exit.signal ?? 'null'}`,
    );
    const failedRuns = Array.from(run.activeRuns.values());
    if (workspaceRun?.daemon === daemon) {
      workspaceRun = undefined;
    }
    disposeWorkspaceRun(run);
    for (const active of failedRuns) {
      sidebar.markSessionFailed(active.clientSessionId, 'Daemon exited before the session finished.');
    }
    void refreshStatus(output, sidebar, { force: true });
  });
  workspaceRun = run;
  void subscribeSessionList(run, output, sidebar);
  return run;
}

async function subscribeSessionList(
  run: WorkspaceRun,
  output: vscode.OutputChannel,
  sidebar: PeridotSidebarProvider,
): Promise<void> {
  try {
    const result = (await run.daemon.send('session.subscribe_list')) as SessionListResult;
    run.keepAlive = true;
    sidebar.reconcileDaemonSessions(normalizeDaemonSessions(result));
  } catch (err) {
    const message = err instanceof Error ? err.message : String(err);
    output.appendLine(`[peridot] session.subscribe_list failed: ${message}`);
  }
}

function activeRunCount(): number {
  return workspaceRun?.activeRuns.size ?? 0;
}

function currentActiveRun(sidebar: PeridotSidebarProvider): ActiveRun | undefined {
  if (!workspaceRun) return undefined;
  const clientSessionId = sidebar.currentClientSessionId();
  if (clientSessionId) {
    const byClient = workspaceRun.activeRuns.get(clientSessionId);
    if (byClient) return byClient;
  }
  const daemonSessionId = sidebar.currentDaemonSessionId();
  return daemonSessionId ? runForDaemonSession(daemonSessionId) : undefined;
}

function singleActiveRun(): ActiveRun | undefined {
  if (!workspaceRun || workspaceRun.activeRuns.size !== 1) return undefined;
  return workspaceRun.activeRuns.values().next().value as ActiveRun | undefined;
}

function runForDaemonSession(sessionId: string | undefined): ActiveRun | undefined {
  if (!workspaceRun || !sessionId) return undefined;
  return Array.from(workspaceRun.activeRuns.values()).find((run) => run.sessionId === sessionId);
}

function runForAskUserRequest(requestId: string): ActiveRun | undefined {
  const marker = ':ask-user:';
  const index = requestId.indexOf(marker);
  return index > 0 ? runForDaemonSession(requestId.slice(0, index)) : singleActiveRun();
}

function runForApproval(
  decision: ApprovalResponse,
  sidebar: PeridotSidebarProvider,
): ActiveRun | undefined {
  return runForDaemonSession(decision.sessionId) ?? currentActiveRun(sidebar) ?? singleActiveRun();
}

/**
 * Ask the daemon to LLM-generate a short title for the current session.
 *
 * Resolves to the LLM's title string, or `null` if the daemon reports an
 * error / no workspace is open / the LLM returns empty. The sidebar treats
 * `null` as "fall back to 'No title'" — never to the raw truncated task.
 *
 * Fire-and-forget from the caller's perspective: this never throws. We
 * deliberately don't share the workspace daemon's lifetime semantics here —
 * if no workspace daemon exists yet, we just return `null` and let the
 * placeholder remain. By the time the second turn runs there will be one.
 */
async function generateSessionTitle(
  task: string,
  output: vscode.OutputChannel,
): Promise<string | null> {
  const folder = vscode.workspace.workspaceFolders?.[0]?.uri.fsPath;
  if (!folder) {
    return null;
  }
  try {
    const workspace = await ensureWorkspaceRun(folder, output, sidebarForGenerateTitle());
    const result = (await workspace.daemon.send('session.generate_title', {
      task,
    })) as { title?: string | null };
    const title = result?.title?.trim();
    return title && title.length > 0 ? title : null;
  } catch (err) {
    const message = err instanceof Error ? err.message : String(err);
    output.appendLine(`[peridot] session.generate_title failed: ${message}`);
    return null;
  }
}

/**
 * Sidebar lookup helper used by `generateSessionTitle`. ensureWorkspaceRun
 * needs a sidebar reference for its exit-listener callback, but the
 * sidebar instance is held by the outer activate() closure. We resolve it
 * through the module-level binding `activeSidebar` set during activation.
 */
function sidebarForGenerateTitle(): PeridotSidebarProvider {
  if (!activeSidebar) {
    throw new Error('Peridot sidebar is not active');
  }
  return activeSidebar;
}

async function runTask(
  providedTask: string | undefined,
  output: vscode.OutputChannel,
  sidebar: PeridotSidebarProvider,
  options: RunOptions = { mode: 'execute', permission: 'auto' },
): Promise<void> {
  const currentClientSessionId = sidebar.currentClientSessionId();
  if (currentClientSessionId && workspaceRun?.activeRuns.has(currentClientSessionId)) {
    await vscode.window.showWarningMessage(
      'This Peridot session is already running. Switch to a new session to start another task.',
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

  try {
    const workspace = await ensureWorkspaceRun(folder, output, sidebar);
    const run: ActiveRun = {
      daemon: workspace.daemon,
      clientSessionId: prepared.clientSessionId,
    };
    workspace.activeRuns.set(prepared.clientSessionId, run);

    const result = (await workspace.daemon.send('session.start', {
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
    sidebar.setSessionFor(prepared.clientSessionId, result.session_id);
    void refreshStatus(output, sidebar, { force: true });
  } catch (err) {
    workspaceRun?.activeRuns.delete(prepared.clientSessionId);
    await shutdownWorkspaceDaemonIfIdle(output);
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
  const liveRun = currentActiveRun(sidebar);
  const sessionId = liveRun?.sessionId ?? sidebar.currentDaemonSessionId();
  const params = {
    command,
    surface: 'vscode',
    ...(sessionId ? { session_id: sessionId } : {}),
  };
  if (liveRun?.daemon) {
    output.appendLine(`[peridot] session.command ${command}`);
    return asCommandResult(await liveRun.daemon.send('session.command', params));
  }
  if (workspaceRun?.daemon) {
    output.appendLine(`[peridot] session.command (workspace) ${command}`);
    return asCommandResult(await workspaceRun.daemon.send('session.command', params));
  }

  output.appendLine(`[peridot] session.command (spawn) ${command}`);
  const daemon = await PeridotDaemon.spawn(folder);
  try {
    return asCommandResult(await daemon.send('session.command', params));
  } finally {
    await daemon.shutdown();
  }
}

async function showWorkspaceCodeMap(
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
    await vscode.window.showErrorMessage(`Peridot code map failed: ${message}`);
  }
}

async function showGitHubPrStatus(
  output: vscode.OutputChannel,
  sidebar: PeridotSidebarProvider,
): Promise<void> {
  const folder = vscode.workspace.workspaceFolders?.[0]?.uri.fsPath;
  if (!folder) {
    const message = 'Open a workspace folder before checking GitHub PR status.';
    sidebar.setWorkspaceProblem(message);
    await vscode.window.showWarningMessage(message);
    return;
  }
  await vscode.commands.executeCommand('peridot.chatView.focus');
  try {
    const { stdout, stderr } = await vscode.window.withProgress(
      {
        location: vscode.ProgressLocation.Window,
        title: 'Peridot: checking GitHub PR status',
      },
      async () => execFile('gh', ['pr', 'status'], folder),
    );
    sidebar.appendCommandResult({
      kind: 'pr_status',
      title: 'GitHub PR Status',
      message: (stdout || stderr || 'No PR status output.').trim(),
      severity: 'info',
      command: 'gh pr status',
    });
  } catch (err) {
    const message = err instanceof Error ? err.message : String(err);
    output.appendLine(`[peridot] gh pr status failed: ${message}`);
    sidebar.appendError(message);
    await vscode.window.showErrorMessage(`GitHub PR status failed: ${message}`);
  }
}

async function showWorkspaceCodeMapStatus(
  output: vscode.OutputChannel,
  sidebar: PeridotSidebarProvider,
): Promise<void> {
  const folder = vscode.workspace.workspaceFolders?.[0]?.uri.fsPath;
  if (!folder) {
    vscode.window.showWarningMessage('Open a workspace folder before checking the code map.');
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

async function searchWorkspaceCodeMap(
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

async function locateWorkspaceCodeMapSymbol(
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
      await vscode.window.showInformationMessage(`No indexed symbol matched "${trimmed}".`);
    }
  } catch (err) {
    const message = err instanceof Error ? err.message : String(err);
    output.appendLine(`[peridot] codemap locate failed: ${message}`);
    sidebar.appendError(message);
    await vscode.window.showErrorMessage(`Peridot symbol locate failed: ${message}`);
  }
}

async function outlineCurrentFile(
  output: vscode.OutputChannel,
  sidebar: PeridotSidebarProvider,
): Promise<void> {
  const editor = vscode.window.activeTextEditor;
  if (!editor) {
    await vscode.window.showWarningMessage('Open a source file before outlining it with Peridot.');
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
    await vscode.window.showErrorMessage(`Peridot file outline failed: ${message}`);
  }
}

async function findWorkspaceSymbolReferences(
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
    await vscode.window.showErrorMessage(`Peridot symbol references failed: ${message}`);
  }
}

async function attachFileToSession(
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
    await vscode.window.showWarningMessage('Start or select a Peridot session before attaching a file.');
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
    await vscode.window.showWarningMessage('Peridot only attaches files inside the workspace.');
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
    await vscode.window.showErrorMessage(`Peridot attach failed: ${message}`);
  }
}

async function showSessionAttachments(
  output: vscode.OutputChannel,
  sidebar: PeridotSidebarProvider,
): Promise<void> {
  if (!sidebar.currentDaemonSessionId()) {
    await vscode.window.showWarningMessage('Start or select a Peridot session before listing attachments.');
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
    await vscode.window.showErrorMessage(`Peridot attachments failed: ${message}`);
  }
}

async function showSkills(
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
    await vscode.window.showErrorMessage(`Peridot skills failed: ${message}`);
  }
}

async function showArchivedSkills(
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
    await vscode.window.showErrorMessage(`Peridot archived skills failed: ${message}`);
  }
}

async function searchSkills(
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
    await vscode.window.showErrorMessage(`Peridot skill search failed: ${message}`);
  }
}

async function searchArchivedSkills(
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
    await vscode.window.showErrorMessage(`Peridot archived skill search failed: ${message}`);
  }
}

async function showSkill(
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
    await vscode.window.showErrorMessage(`Peridot skill view failed: ${message}`);
  }
}

async function useSkill(
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
    await vscode.window.showErrorMessage(`Peridot skill use failed: ${message}`);
  }
}

async function toggleSkillPin(
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
    await vscode.window.showErrorMessage(`Peridot skill update failed: ${message}`);
  }
}

async function archiveSkill(
  skillName: string,
  output: vscode.OutputChannel,
  sidebar: PeridotSidebarProvider,
): Promise<void> {
  const name = skillName.trim().replace(/^\/+/, '');
  if (!name) return;
  const confirmed = await vscode.window.showWarningMessage(
    `Archive Peridot skill ${name}? It will be hidden from active skill lists.`,
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
  } catch (err) {
    const message = err instanceof Error ? err.message : String(err);
    output.appendLine(`[peridot] skill archive failed: ${message}`);
    sidebar.appendError(message);
    await vscode.window.showErrorMessage(`Peridot skill archive failed: ${message}`);
  }
}

async function restoreSkill(
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
  } catch (err) {
    const message = err instanceof Error ? err.message : String(err);
    output.appendLine(`[peridot] skill restore failed: ${message}`);
    sidebar.appendError(message);
    await vscode.window.showErrorMessage(`Peridot skill restore failed: ${message}`);
  }
}

async function detachAttachmentFromSession(
  attachmentPath: string,
  output: vscode.OutputChannel,
  sidebar: PeridotSidebarProvider,
): Promise<void> {
  const path = attachmentPath.trim();
  if (!path) return;
  if (!sidebar.currentDaemonSessionId()) {
    await vscode.window.showWarningMessage('Start or select a Peridot session before detaching a file.');
    return;
  }
  const confirmed = await vscode.window.showWarningMessage(
    `Detach ${path} from this Peridot session context?`,
    { modal: true },
    'Detach',
  );
  if (confirmed !== 'Detach') return;
  await vscode.commands.executeCommand('peridot.chatView.focus');
  try {
    const result = await runSlashCommand(`/detach ${path}`, output, sidebar, sidebar.currentRunOptions());
    sidebar.appendCommandResult(result);
  } catch (err) {
    const message = err instanceof Error ? err.message : String(err);
    output.appendLine(`[peridot] detach failed: ${message}`);
    sidebar.appendError(message);
    await vscode.window.showErrorMessage(`Peridot detach failed: ${message}`);
  }
}

async function exportSessionArtifacts(
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
  const sessionId = sidebar.currentDaemonSessionId();
  if (!sessionId) {
    await vscode.window.showWarningMessage('Start or select a Peridot session before exporting artifacts.');
    return;
  }
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
  const destination = path.join(base.fsPath, `peridot-session-${sanitizePathSegment(sessionId)}`);
  let force = false;
  if (await pathExists(destination)) {
    const confirmed = await vscode.window.showWarningMessage(
      `${destination} already exists. Overwrite it?`,
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
    const artifacts = exportedArtifactsFromPayload(payload);
    sidebar.appendCommandResult({
      kind: 'session_export',
      title: 'Session Artifact Export',
      command: 'peridot session export',
      message: `Exported ${artifacts.length} artifact files to ${destination}`,
      items: [
        { label: 'Destination', detail: destination, source: 'directory' },
        ...artifacts.map((artifact) => ({
          label: artifact.path,
          detail: `${artifact.class} · ${artifact.count} entries`,
          source: 'artifact',
        })),
      ],
    });
    await vscode.commands.executeCommand('revealFileInOS', vscode.Uri.file(destination));
  } catch (err) {
    const message = err instanceof Error ? err.message : String(err);
    output.appendLine(`[peridot] session artifact export failed: ${message}`);
    sidebar.appendError(message);
    await vscode.window.showErrorMessage(`Peridot export failed: ${message}`);
  }
}

async function shipChangesToPr(
  output: vscode.OutputChannel,
  sidebar: PeridotSidebarProvider,
): Promise<void> {
  const folder = vscode.workspace.workspaceFolders?.[0]?.uri.fsPath;
  if (!folder) {
    const message = 'Open a workspace folder before shipping changes.';
    sidebar.setWorkspaceProblem(message);
    await vscode.window.showWarningMessage(message);
    return;
  }
  const options = await promptShipOptions();
  if (!options) return;

  const previewArgs = buildShipArgs(options, true);
  await vscode.commands.executeCommand('peridot.chatView.focus');
  try {
    const preview = await vscode.window.withProgress(
      {
        location: vscode.ProgressLocation.Window,
        title: 'Peridot: previewing ship plan',
      },
      async () => execPeridotCli(previewArgs, folder),
    );
    const previewJson = parseJson(preview.stdout);
    const confirmed = await vscode.window.showWarningMessage(
      shipPreviewText(previewJson),
      { modal: true },
      'Ship Changes',
    );
    if (confirmed !== 'Ship Changes') {
      sidebar.appendCommandResult(shipResultView(previewJson, 'Ship Preview', '/ship --dry-run'));
      return;
    }

    const result = await vscode.window.withProgress(
      {
        location: vscode.ProgressLocation.Notification,
        title: 'Peridot: shipping changes to PR',
      },
      async () => execPeridotCli(buildShipArgs(options, false), folder),
    );
    sidebar.appendCommandResult(shipResultView(parseJson(result.stdout), 'Ship Changes', 'peridot ship'));
    void refreshStatus(output, sidebar, { force: true });
  } catch (err) {
    const message = err instanceof Error ? err.message : String(err);
    output.appendLine(`[peridot] ship failed: ${message}`);
    sidebar.appendError(message);
    await vscode.window.showErrorMessage(`Peridot ship failed: ${message}`);
  }
}

async function mergeGitHubPr(
  output: vscode.OutputChannel,
  sidebar: PeridotSidebarProvider,
): Promise<void> {
  const folder = vscode.workspace.workspaceFolders?.[0]?.uri.fsPath;
  if (!folder) {
    const message = 'Open a workspace folder before merging a GitHub PR.';
    sidebar.setWorkspaceProblem(message);
    await vscode.window.showWarningMessage(message);
    return;
  }
  const options = await promptMergeOptions();
  if (!options) return;
  const args = buildMergeArgs(options);
  const confirmed = await vscode.window.showWarningMessage(
    mergePreviewText(options),
    { modal: true },
    'Merge PR',
  );
  if (confirmed !== 'Merge PR') return;

  await vscode.commands.executeCommand('peridot.chatView.focus');
  try {
    const { stdout, stderr } = await vscode.window.withProgress(
      {
        location: vscode.ProgressLocation.Notification,
        title: 'Peridot: merging GitHub PR',
      },
      async () => execFile('gh', args, folder),
    );
    sidebar.appendCommandResult({
      kind: 'pr_merge',
      title: 'GitHub PR Merge',
      message: (stdout || stderr || 'PR merge completed.').trim(),
      severity: 'info',
      command: `gh ${args.join(' ')}`,
    });
    void refreshStatus(output, sidebar, { force: true });
  } catch (err) {
    const message = err instanceof Error ? err.message : String(err);
    output.appendLine(`[peridot] gh pr merge failed: ${message}`);
    sidebar.appendError(message);
    await vscode.window.showErrorMessage(`GitHub PR merge failed: ${message}`);
  }
}

interface ShipPromptOptions {
  branch?: string;
  message?: string;
  prTitle?: string;
  prBody?: string;
  draft: boolean;
  noPr: boolean;
}

async function promptShipOptions(): Promise<ShipPromptOptions | undefined> {
  const message = await vscode.window.showInputBox({
    title: 'Peridot: Ship Changes',
    prompt: 'Commit message. Leave empty for the Peridot default.',
    ignoreFocusOut: true,
  });
  if (message === undefined) return undefined;
  const branch = await vscode.window.showInputBox({
    title: 'Peridot: Ship Changes',
    prompt: 'Target branch. Leave empty for peridot/ship-<time>.',
    ignoreFocusOut: true,
  });
  if (branch === undefined) return undefined;
  const prTitle = await vscode.window.showInputBox({
    title: 'Peridot: Ship Changes',
    prompt: 'PR title. Leave empty to use the commit message.',
    ignoreFocusOut: true,
  });
  if (prTitle === undefined) return undefined;
  const prBody = await vscode.window.showInputBox({
    title: 'Peridot: Ship Changes',
    prompt: 'PR body. Leave empty for the Peridot default.',
    ignoreFocusOut: true,
  });
  if (prBody === undefined) return undefined;
  const prMode = await vscode.window.showQuickPick(
    [
      { label: 'Draft PR', description: 'Recommended', value: 'draft' },
      { label: 'Ready PR', description: 'Open as ready for review', value: 'ready' },
      { label: 'No PR', description: 'Commit and push only', value: 'none' },
    ],
    {
      title: 'Peridot: Ship Changes',
      placeHolder: 'Choose how Peridot should handle the pull request.',
      ignoreFocusOut: true,
    },
  );
  if (!prMode) return undefined;
  return {
    branch: nonEmpty(branch),
    message: nonEmpty(message),
    prTitle: nonEmpty(prTitle),
    prBody: nonEmpty(prBody),
    draft: prMode.value === 'draft',
    noPr: prMode.value === 'none',
  };
}

function buildShipArgs(options: ShipPromptOptions, dryRun: boolean): string[] {
  const args = ['--output', 'json', 'ship'];
  if (dryRun) args.push('--dry-run');
  if (options.branch) args.push('--branch', options.branch);
  if (options.message) args.push('--message', options.message);
  if (options.prTitle) args.push('--pr-title', options.prTitle);
  if (options.prBody) args.push('--pr-body', options.prBody);
  if (options.draft) args.push('--draft');
  if (options.noPr) args.push('--no-pr');
  return args;
}

interface MergePromptOptions {
  pr?: string;
  method: 'merge' | 'squash' | 'rebase';
  keepBranch: boolean;
}

async function promptMergeOptions(): Promise<MergePromptOptions | undefined> {
  const pr = await vscode.window.showInputBox({
    title: 'Peridot: Merge GitHub PR',
    prompt: 'PR number or URL. Leave empty for the PR linked to the current branch.',
    ignoreFocusOut: true,
  });
  if (pr === undefined) return undefined;
  const method = await vscode.window.showQuickPick(
    [
      { label: 'Merge commit', description: 'Preserve branch commits', value: 'merge' },
      { label: 'Squash', description: 'Collapse into one commit', value: 'squash' },
      { label: 'Rebase', description: 'Replay commits onto base', value: 'rebase' },
    ],
    {
      title: 'Peridot: Merge GitHub PR',
      placeHolder: 'Choose merge strategy.',
      ignoreFocusOut: true,
    },
  );
  if (!method) return undefined;
  const branch = await vscode.window.showQuickPick(
    [
      { label: 'Delete branch', description: 'Recommended after merge', value: 'delete' },
      { label: 'Keep branch', description: 'Leave the remote branch in place', value: 'keep' },
    ],
    {
      title: 'Peridot: Merge GitHub PR',
      placeHolder: 'Choose branch cleanup behavior.',
      ignoreFocusOut: true,
    },
  );
  if (!branch) return undefined;
  return {
    pr: nonEmpty(pr),
    method: method.value as MergePromptOptions['method'],
    keepBranch: branch.value === 'keep',
  };
}

function buildMergeArgs(options: MergePromptOptions): string[] {
  const args = ['pr', 'merge'];
  if (options.pr) args.push(options.pr);
  args.push(`--${options.method}`);
  if (!options.keepBranch) args.push('--delete-branch');
  return args;
}

function mergePreviewText(options: MergePromptOptions): string {
  return [
    'Peridot will merge a GitHub pull request.',
    '',
    `PR: ${options.pr ?? 'current branch PR'}`,
    `Method: ${options.method}`,
    `Branch cleanup: ${options.keepBranch ? 'keep branch' : 'delete branch'}`,
    '',
    'This changes remote repository state.',
  ].join('\n');
}

function nonEmpty(value: string): string | undefined {
  const trimmed = value.trim();
  return trimmed.length > 0 ? trimmed : undefined;
}

function sanitizePathSegment(value: string): string {
  const sanitized = value.replace(/[^A-Za-z0-9._-]+/g, '-').replace(/^-+|-+$/g, '');
  return sanitized.length > 0 ? sanitized : 'session';
}

async function pathExists(filePath: string): Promise<boolean> {
  try {
    await vscode.workspace.fs.stat(vscode.Uri.file(filePath));
    return true;
  } catch {
    return false;
  }
}

async function execPeridotCli(
  args: string[],
  cwd: string,
): Promise<{ stdout: string; stderr: string }> {
  const binary = await resolvePeridotBinary();
  return execFile(binary, args, cwd);
}

function execFile(
  command: string,
  args: string[],
  cwd: string,
): Promise<{ stdout: string; stderr: string }> {
  return new Promise((resolve, reject) => {
    childProcess.execFile(
      command,
      args,
      {
        cwd,
        env: peridotChildEnv(),
        maxBuffer: 10 * 1024 * 1024,
      },
      (error, stdout, stderr) => {
        if (error) {
          const detail = stderr.trim() || stdout.trim() || error.message;
          reject(new Error(detail));
          return;
        }
        resolve({ stdout, stderr });
      },
    );
  });
}

function parseJson(raw: string): unknown {
  try {
    return JSON.parse(raw);
  } catch {
    return { output: raw };
  }
}

interface ExportedArtifactView {
  class: string;
  path: string;
  count: number;
}

function exportedArtifactsFromPayload(payload: unknown): ExportedArtifactView[] {
  if (!isRecord(payload) || !Array.isArray(payload.artifacts)) return [];
  return payload.artifacts
    .filter(isRecord)
    .map((artifact) => ({
      class: typeof artifact.class === 'string' ? artifact.class : 'artifact',
      path: typeof artifact.path === 'string' ? artifact.path : 'artifact',
      count: typeof artifact.count === 'number' ? artifact.count : 0,
    }));
}

function shipPreviewText(value: unknown): string {
  const steps = shipSteps(value);
  const lines = steps.slice(0, 6).map((step) => `- ${step.status} ${step.step}${step.detail ? `: ${step.detail}` : ''}`);
  return [
    'Peridot will run the following publish steps:',
    '',
    ...lines,
    '',
    'This will commit, push, and may open a GitHub PR.',
  ].join('\n');
}

function shipResultView(value: unknown, title: string, command: string): CommandResultView {
  const steps = shipSteps(value);
  const prUrl = steps.find((step) => step.step === 'pr' && step.url)?.url;
  return {
    kind: 'ship',
    title,
    message: prUrl ? `Ship complete: ${prUrl}` : title,
    severity: 'info',
    command,
    items: steps.map((step) => ({
      source: step.step,
      label: `${step.status} ${step.step}`,
      detail: step.detail,
    })),
  };
}

interface ShipStepView {
  step: string;
  status: string;
  detail?: string;
  url?: string;
}

function shipSteps(value: unknown): ShipStepView[] {
  const rawSteps = isRecord(value) && Array.isArray(value.steps) ? value.steps : [];
  return rawSteps.filter(isRecord).map((step) => {
    const name = typeof step.step === 'string' ? step.step : 'step';
    const status = typeof step.status === 'string' ? step.status : 'unknown';
    const detail = [
      typeof step.message === 'string' ? step.message : undefined,
      typeof step.branch === 'string' ? step.branch : undefined,
      typeof step.to === 'string' ? step.to : undefined,
      typeof step.title === 'string' ? step.title : undefined,
      typeof step.reason === 'string' ? step.reason : undefined,
      typeof step.url === 'string' ? step.url : undefined,
    ]
      .filter((entry): entry is string => Boolean(entry))
      .join(' · ');
    return {
      step: name,
      status,
      ...(detail ? { detail } : {}),
      ...(typeof step.url === 'string' ? { url: step.url } : {}),
    };
  });
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
      running: activeRunCount() > 0,
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
    void refreshSessionList(output, sidebar).catch((err: unknown) => {
      const message = err instanceof Error ? err.message : String(err);
      output.appendLine(`[peridot] session list refresh failed: ${message}`);
    });
    const extensionVersion =
      vscode.extensions.getExtension('dlsxj101.peridot-vscode')?.packageJSON?.version ??
      'unknown';
    const cleanupProblem = worktreeCleanupProblem(result.worktree_cleanup);
    const cleanupSummary = worktreeCleanupSummary(result.worktree_cleanup);
    if (cleanupSummary) output.appendLine(`[peridot] worktree cleanup: ${cleanupSummary}`);
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
      status: cleanupProblem ? 'Needs attention' : activeRunCount() > 0 ? 'Running' : 'Idle',
      problem: cleanupProblem,
      running: activeRunCount() > 0,
    });
  } catch (err) {
    const message = err instanceof Error ? err.message : String(err);
    output.appendLine(`[peridot] status failed: ${message}`);
    sidebar.setContext({
      workspace: folder,
      status: activeRunCount() > 0 ? 'Running' : 'Needs attention',
      problem: message,
      running: activeRunCount() > 0,
    });
  }
}

function worktreeCleanupSummary(cleanup?: WorktreeCleanupResult): string | undefined {
  if (!cleanup) return undefined;
  const parts: string[] = [];
  const suspended = cleanup.suspended_sessions?.length ?? 0;
  const removed = cleanup.removed_worktrees?.length ?? 0;
  const preserved = cleanup.preserved_worktrees?.length ?? 0;
  const missing = cleanup.missing_worktrees?.length ?? 0;
  const errors = cleanup.errors?.length ?? 0;
  if (suspended > 0) parts.push(`${suspended} stale session(s) suspended`);
  if (removed > 0) parts.push(`${removed} clean worktree(s) removed`);
  if (preserved > 0) parts.push(`${preserved} dirty worktree(s) preserved`);
  if (missing > 0) parts.push(`${missing} missing worktree record(s) reconciled`);
  if (errors > 0) parts.push(`${errors} cleanup error(s)`);
  return parts.length > 0 ? parts.join('; ') : undefined;
}

function worktreeCleanupProblem(cleanup?: WorktreeCleanupResult): string | undefined {
  if (!cleanup) return undefined;
  const preserved = cleanup.preserved_worktrees ?? [];
  const errors = cleanup.errors ?? [];
  if (preserved.length === 0 && errors.length === 0) return undefined;
  const parts: string[] = [];
  if (preserved.length > 0) {
    const first = preserved[0];
    const suffix = preserved.length > 1 ? ` and ${preserved.length - 1} more` : '';
    parts.push(
      `Dirty Peridot worktree preserved: ${first.path ?? first.session_id ?? 'unknown'}${suffix}`,
    );
  }
  if (errors.length > 0) {
    const first = errors[0];
    const suffix = errors.length > 1 ? ` and ${errors.length - 1} more` : '';
    parts.push(`Worktree cleanup error: ${first.message ?? first.path ?? 'unknown'}${suffix}`);
  }
  return parts.join(' · ');
}

async function fetchSlashCatalog(
  folder: string,
  output: vscode.OutputChannel,
): Promise<SlashCommandSpec[]> {
  if (workspaceRun?.daemon) {
    const catalog = (await workspaceRun.daemon.send(
      'session.command_catalog',
      { surface: 'vscode' },
    )) as SlashCommandCatalogResult;
    const skills = await fetchSkillsList(workspaceRun.daemon, output);
    return mergeSlashCatalogAndSkills(catalog, skills);
  }
  output.appendLine(`[peridot] slash catalog fetch (spawn) for ${folder}`);
  const daemon = await PeridotDaemon.spawn(folder);
  try {
    const catalog = (await daemon.send('session.command_catalog', {
      surface: 'vscode',
    })) as SlashCommandCatalogResult;
    const skills = await fetchSkillsList(daemon, output);
    return mergeSlashCatalogAndSkills(catalog, skills);
  } finally {
    await daemon.shutdown();
  }
}

async function fetchSkillsList(
  daemon: PeridotDaemon,
  output: vscode.OutputChannel,
): Promise<SkillsListResult> {
  try {
    return (await daemon.send('skills.list')) as SkillsListResult;
  } catch (err) {
    const message = err instanceof Error ? err.message : String(err);
    output.appendLine(`[peridot] skills.list failed: ${message}`);
    return {};
  }
}

function mergeSlashCatalogAndSkills(
  catalog: SlashCommandCatalogResult,
  skillsResult: SkillsListResult,
): SlashCommandSpec[] {
  const commands = normalizeSlashCatalog(catalog);
  const existing = new Set(commands.map((command) => command.name));
  for (const skill of normalizeSkillSlashEntries(skillsResult)) {
    if (existing.has(skill.name)) continue;
    existing.add(skill.name);
    commands.push(skill);
  }
  return commands;
}

function normalizeSlashCatalog(result: SlashCommandCatalogResult): SlashCommandSpec[] {
  const commands = Array.isArray(result.commands) ? result.commands : [];
  return commands
    .filter((entry) => typeof entry.name === 'string' && typeof entry.description === 'string')
    .filter((entry) => slashCommandSupportsSurface(entry, 'vscode'))
    .map((entry) => ({
      name: entry.name as string,
      description: entry.description as string,
      ...(typeof entry.arg_hint === 'string' ? { argHint: entry.arg_hint } : {}),
      ...slashCommandArgOptionsField(entry),
      ...(typeof entry.category === 'string' ? { category: entry.category } : {}),
      ...slashCommandSurfacesField(entry),
    }));
}

function slashCommandArgOptionsField(entry: { arg_options?: unknown }): Pick<SlashCommandSpec, 'argOptions'> {
  const argOptions = Array.isArray(entry.arg_options)
    ? entry.arg_options.filter((option): option is string => typeof option === 'string')
    : [];
  return argOptions.length > 0 ? { argOptions } : {};
}

function slashCommandSupportsSurface(
  entry: { surfaces?: unknown },
  surface: string,
): boolean {
  const surfaces = normalizeSlashCommandSurfaces(entry);
  return surfaces.length === 0 || surfaces.includes(surface);
}

function slashCommandSurfacesField(entry: { surfaces?: unknown }): Pick<SlashCommandSpec, 'surfaces'> {
  const surfaces = normalizeSlashCommandSurfaces(entry);
  return surfaces.length > 0 ? { surfaces } : {};
}

function normalizeSlashCommandSurfaces(entry: { surfaces?: unknown }): string[] {
  return Array.isArray(entry.surfaces)
    ? entry.surfaces.filter((surface): surface is string => typeof surface === 'string')
    : [];
}

function normalizeSkillSlashEntries(result: SkillsListResult): SlashCommandSpec[] {
  const skills = Array.isArray(result.skills) ? result.skills : [];
  return skills
    .filter((entry) => typeof entry.name === 'string' && entry.name.trim().length > 0)
    .map((entry) => {
      const name = String(entry.name).replace(/^\/+/, '');
      return {
        name: `/${name}`,
        description:
          typeof entry.description === 'string' && entry.description.trim().length > 0
            ? entry.description.trim()
            : 'stored auto-skill',
        category: 'skill',
      };
    });
}

async function fetchStatus(
  folder: string,
  output: vscode.OutputChannel,
): Promise<DaemonStatusResult> {
  // Reuse the long-lived daemon when a session is active so we don't
  // double-spawn just to read context.
  if (workspaceRun?.daemon) {
    return (await workspaceRun.daemon.send('peridot.status')) as DaemonStatusResult;
  }
  output.appendLine(`[peridot] status fetch (spawn) for ${folder}`);
  const daemon = await PeridotDaemon.spawn(folder);
  try {
    return (await daemon.send('peridot.status')) as DaemonStatusResult;
  } finally {
    await daemon.shutdown();
  }
}

async function refreshSessionList(
  output: vscode.OutputChannel,
  sidebar: PeridotSidebarProvider,
): Promise<void> {
  const folder = vscode.workspace.workspaceFolders?.[0]?.uri.fsPath;
  if (!folder) return;
  const result = await fetchSessionList(folder, output);
  sidebar.reconcileDaemonSessions(normalizeDaemonSessions(result));
}

async function fetchSessionList(
  folder: string,
  output: vscode.OutputChannel,
): Promise<SessionListResult> {
  if (workspaceRun?.daemon) {
    return (await workspaceRun.daemon.send('session.list')) as SessionListResult;
  }
  output.appendLine(`[peridot] session list fetch (spawn) for ${folder}`);
  const daemon = await PeridotDaemon.spawn(folder);
  try {
    return (await daemon.send('session.list')) as SessionListResult;
  } finally {
    await daemon.shutdown();
  }
}

function normalizeDaemonSessions(value: unknown): DaemonSessionSummary[] {
  const sessions = isRecord(value) && Array.isArray(value.sessions) ? value.sessions : [];
  return sessions
    .filter(isRecord)
    .map((entry): DaemonSessionSummary | undefined => {
      const id = typeof entry.id === 'string' ? entry.id.trim() : '';
      if (!id) return undefined;
      const summary: DaemonSessionSummary = { id };
      if (typeof entry.title === 'string') summary.title = entry.title;
      if (typeof entry.summary === 'string') summary.summary = entry.summary;
      if (typeof entry.status === 'string') summary.status = entry.status;
      if (typeof entry.running === 'boolean') summary.running = entry.running;
      if (typeof entry.updated_at_unix === 'number') summary.updated_at_unix = entry.updated_at_unix;
      if (typeof entry.last_task === 'string') summary.last_task = entry.last_task;
      if (typeof entry.total_tokens === 'number') summary.total_tokens = entry.total_tokens;
      if (typeof entry.total_cost_usd === 'number') summary.total_cost_usd = entry.total_cost_usd;
      if (typeof entry.turns_used === 'number') summary.turns_used = entry.turns_used;
      return summary;
    })
    .filter((entry): entry is DaemonSessionSummary => Boolean(entry));
}

function invalidateStatusCache(): void {
  statusCache?.invalidate();
}

async function loginOpenAi(
  output: vscode.OutputChannel,
  sidebar: PeridotSidebarProvider,
): Promise<void> {
  if (activeRunCount() > 0) {
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
  const run = sidebar ? currentActiveRun(sidebar) : singleActiveRun();
  if (!run) {
    await vscode.window.showInformationMessage('Peridot is not running a task.');
    return;
  }
  const sessionId = run.sessionId;
  if (!sessionId) {
    output.appendLine('[peridot] cancelling daemon before session id was assigned');
    await finishRunByClient(run.clientSessionId, output);
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
      await finishRunByClient(run.clientSessionId, output);
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
  const run = runForAskUserRequest(requestId);
  if (!run) {
    const message = 'No active Peridot run can receive this response.';
    sidebar.appendError(message);
    await vscode.window.showWarningMessage(message);
    return;
  }
  try {
    const result = (await run.daemon.send('interaction.respond', {
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
  const run = runForApproval(decision, sidebar);
  const sessionId = decision.sessionId ?? run?.sessionId;
  if (!sessionId || !run?.daemon) {
    const message = 'No active Peridot run can receive this approval decision.';
    sidebar.appendError(message);
    await vscode.window.showWarningMessage(message);
    return;
  }
  try {
    const result = (await run.daemon.send('approval.respond', {
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
      await finishRunBySession(sessionId, output);
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
  if (activeRunCount() > 0) {
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
          ['config', 'set', 'api.base_url', CLAUDE_API_BASE_URL],
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
          ['config', 'set', 'api.base_url', OPENAI_API_BASE_URL],
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
          ['config', 'set', 'api.base_url', OPENROUTER_API_BASE_URL],
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
  projectRoot?: string,
): Promise<void> {
  const folder = vscode.workspace.workspaceFolders?.[0];
  if (!folder && !projectRoot) {
    output.appendLine(`[peridot] openFile ignored — no workspace open: ${relativePath}`);
    return;
  }

  const isAbsolute = relativePath.startsWith('/') || /^[A-Za-z]:[\\/]/.test(relativePath);

  // Build candidate URIs for relative paths.  Prefer the daemon's project
  // root (which may differ from the VS Code workspace folder) so that paths
  // emitted by the agent resolve correctly.
  //
  // URI construction uses the workspace folder's scheme + authority so it
  // works correctly in remote environments (WSL, SSH, containers).
  const candidateUris: vscode.Uri[] = [];
  if (isAbsolute) {
    // Absolute path: use the folder's scheme/authority for remote compatibility
    const uri = folder
      ? folder.uri.with({ path: relativePath })
      : vscode.Uri.file(relativePath);
    candidateUris.push(uri);
  } else {
    // Relative path: try project root first, then workspace folder, then
    // a findFiles fallback for cases where both roots are the same or wrong.
    if (projectRoot && folder) {
      candidateUris.push(vscode.Uri.joinPath(folder.uri.with({ path: projectRoot }), relativePath));
    }
    if (folder) {
      const folderCandidate = vscode.Uri.joinPath(folder.uri, relativePath);
      // Avoid duplicate when projectRoot === folder path
      if (!candidateUris.some((u) => u.toString() === folderCandidate.toString())) {
        candidateUris.push(folderCandidate);
      }
    }
  }

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

  // Try each candidate until one succeeds.
  const errors: string[] = [];
  for (const uri of candidateUris) {
    try {
      const document = await vscode.workspace.openTextDocument(uri);
      await vscode.window.showTextDocument(document, {
        ...selectionOptions,
        preview: openOptions?.preview ?? false,
        viewColumn:
          openOptions?.beside && vscode.window.activeTextEditor
            ? vscode.ViewColumn.Beside
            : vscode.ViewColumn.Active,
      });
      return; // success — stop trying
    } catch (err) {
      errors.push(err instanceof Error ? err.message : String(err));
    }
  }

  // All direct candidates failed — try a workspace-wide file search as a
  // last resort.  This handles the common case where the VS Code workspace
  // folder is a parent of the actual project root (e.g. workspace is
  // /home/user/workspace but project is /home/user/workspace/my-project).
  if (!isAbsolute) {
    try {
      const pattern = `**/${relativePath}`;
      const found = await vscode.workspace.findFiles(pattern, undefined, 1);
      if (found.length > 0) {
        const document = await vscode.workspace.openTextDocument(found[0]);
        await vscode.window.showTextDocument(document, {
          ...selectionOptions,
          preview: openOptions?.preview ?? false,
          viewColumn:
            openOptions?.beside && vscode.window.activeTextEditor
              ? vscode.ViewColumn.Beside
              : vscode.ViewColumn.Active,
        });
        return;
      }
    } catch (err) {
      errors.push(`findFiles: ${err instanceof Error ? err.message : String(err)}`);
    }
  }

  // Everything failed.
  output.appendLine(`[peridot] openFile failed for "${relativePath}": ${errors.join(' | ')}`);
  void vscode.window.showWarningMessage(`파일을 열 수 없습니다: ${relativePath}`);
}

async function clearExtensionSession(
  output: vscode.OutputChannel,
  sidebar: PeridotSidebarProvider,
  skipDaemonCancel = false,
): Promise<void> {
  const current = currentActiveRun(sidebar) ?? singleActiveRun();
  if (current?.sessionId && !skipDaemonCancel) {
    try {
      await current.daemon.send('session.cancel', { session_id: current.sessionId });
    } catch (err) {
      const message = err instanceof Error ? err.message : String(err);
      output.appendLine(`[peridot] clear cancel failed: ${message}`);
    }
    await finishRunByClient(current.clientSessionId, output);
  }
}

async function deleteExtensionSession(
  clientSessionId: string,
  daemonSessionId: string | undefined,
  output: vscode.OutputChannel,
): Promise<void> {
  const run =
    workspaceRun?.activeRuns.get(clientSessionId) ??
    (daemonSessionId ? runForDaemonSession(daemonSessionId) : undefined);
  const sessionId = run?.sessionId ?? daemonSessionId;
  const daemon = run?.daemon ?? workspaceRun?.daemon;
  if (sessionId && daemon) {
    try {
      await daemon.send('session.cancel', { session_id: sessionId });
    } catch (err) {
      const message = err instanceof Error ? err.message : String(err);
      output.appendLine(`[peridot] delete session cancel failed: ${message}`);
    }
  }
  await finishRunByClient(clientSessionId, output);
}

async function handleDaemonNotification(
  notification: RpcNotification,
  output: vscode.OutputChannel,
  sidebar: PeridotSidebarProvider,
): Promise<void> {
  if (notification.method === 'session.list_changed') {
    sidebar.reconcileDaemonSessions(normalizeDaemonSessions(notification.params));
    return;
  }
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
  const clientSessionId = runForDaemonSession(sessionId)?.clientSessionId;
  sidebar.appendNotificationFor(clientSessionId, params);
  const planDocumentPath = planDocumentPathFromEvent(event);
  if (planDocumentPath) {
    await openWorkspaceFile(planDocumentPath, output, undefined, undefined, {
      beside: true,
      preview: false,
    });
  }

  if (isTerminalEvent(event)) {
    await finishRunBySession(sessionId, output);
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

async function finishWorkspaceRun(output?: vscode.OutputChannel): Promise<void> {
  const run = workspaceRun;
  if (!run) return;
  workspaceRun = undefined;
  run.activeRuns.clear();
  disposeWorkspaceRun(run);
  try {
    await run.daemon.shutdown();
  } catch (err) {
    const message = err instanceof Error ? err.message : String(err);
    output?.appendLine(`[peridot] daemon shutdown failed: ${message}`);
  }
}

async function finishRunByClient(
  clientSessionId: string,
  output?: vscode.OutputChannel,
): Promise<void> {
  if (!workspaceRun) return;
  workspaceRun.activeRuns.delete(clientSessionId);
  await shutdownWorkspaceDaemonIfIdle(output);
}

async function finishRunBySession(
  sessionId: string,
  output?: vscode.OutputChannel,
): Promise<void> {
  const run = runForDaemonSession(sessionId);
  if (!run) return;
  await finishRunByClient(run.clientSessionId, output);
}

async function shutdownWorkspaceDaemonIfIdle(output?: vscode.OutputChannel): Promise<void> {
  if (!workspaceRun || workspaceRun.activeRuns.size > 0) return;
  if (workspaceRun.keepAlive) return;
  await finishWorkspaceRun(output);
}

function disposeWorkspaceRun(run: WorkspaceRun): void {
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
