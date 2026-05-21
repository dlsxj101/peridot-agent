import * as vscode from 'vscode';
import {
  ApprovalResponse,
  AskUserAnswer,
  BudgetSlice,
  ChatSessionSummary,
  CommitteeRoleSlice,
  CommandResultView,
  ContextSlice,
  HudState,
  InboundMessage,
  OutboundMessage,
  PlanSlice,
  PlanStepView,
  ProviderChoice,
  QueuedMessage,
  RunOptions,
  SidebarContext,
  SidebarState,
  TranscriptItem,
  UsageSlice,
} from './types';

export type {
  ApprovalResponse,
  ApprovalScope,
  AskUserAnswer,
  ProviderChoice,
  RunOptions,
} from './types';

export interface SidebarHandlers {
  runTask: (task: string, options: RunOptions) => Promise<void>;
  runSlashCommand: (command: string, options: RunOptions) => Promise<CommandResultView>;
  cancelTask: () => Promise<void>;
  clearSession: () => Promise<void>;
  loginOpenAi: () => Promise<void>;
  refreshStatus: () => Promise<void>;
  respondAskUser: (requestId: string, answer: AskUserAnswer) => Promise<void>;
  respondApproval: (decision: ApprovalResponse) => Promise<void>;
  openFile: (relativePath: string, line?: number, column?: number) => Promise<void>;
  registerProvider: (provider: ProviderChoice, params: Record<string, string>) => Promise<void>;
}

interface DaemonEventParams {
  session_id?: string;
  event?: unknown;
}

interface StoredChatSession {
  id: string;
  title: string;
  daemonSessionId?: string;
  status: string;
  running: boolean;
  transcript: TranscriptItem[];
  hud: HudState;
}

interface PersistedSidebarSnapshot {
  version: 1;
  activeChatId?: string;
  nextSessionOrdinal: number;
  runOptions: RunOptions;
  context: SidebarContext;
  view: SidebarState['view'];
  landing: SidebarState['landing'];
  queue: QueuedMessage[];
  sessions: StoredChatSession[];
}

const PERSISTENCE_KEY = 'peridot.sidebarState.v1';

const EXTENSION_SLASH_COMMANDS: Array<[string, string]> = [
  ['/plan', 'switch to plan mode'],
  ['/execute', 'switch to execute mode'],
  ['/goal <objective>', 'start a goal-mode run, or use pause/resume/clear/status'],
  ['/safe', 'switch to safe permission mode'],
  ['/auto', 'switch to auto permission mode'],
  ['/yolo', 'switch to yolo permission mode'],
  ['/model <name>', 'switch the active model'],
  ['/provider <name>', 'switch the displayed provider for this session'],
  ['/reasoning <off|low|medium|high>', 'set reasoning intensity for future runs'],
  ['/think [off|low|medium|high]', 'shortcut for reasoning high, or disable it'],
  ['/fast [on|off|toggle]', 'toggle fast / priority service tier'],
  ['/note <text>', 'attach a note to the transcript'],
  ['/info', 'show current session status'],
  ['/committee <off|planner|full>', 'record committee mode preference'],
  ['/cost', 'show token and cost totals'],
  ['/compact', 'request context compaction when available'],
  ['/context top', 'show current context usage'],
  ['/sidepanel', 'show status summary'],
  ['/status', 'show status summary'],
  ['/collapse', 'toggle transcript collapse preference'],
  ['/session save', 'mark the current session as saved'],
  ['/plan show', 'show current plan steps'],
  ['/diff', 'request a working-tree diff'],
  ['/undo', 'request undo guidance'],
  ['/lang en|ko', 'record display locale preference'],
  ['/clear', 'clear transcript and daemon context'],
  ['/fork <task>', 'spawn a fork-style subagent task through Peridot'],
  ['/teammate <task>', 'spawn a teammate-style subagent task through Peridot'],
  ['/worktree <branch> <task>', 'request worktree-isolated subagent work'],
  ['/subagent model <name|reset>', 'set subagent model preference'],
  ['/mcp list', 'list configured MCP servers'],
  ['/mcp add <name> <stdio|http> <command|url>', 'register an MCP server'],
  ['/mcp remove <name>', 'remove an MCP server'],
  ['/mcp test <name>', 'test an MCP server'],
  ['/todos', 'request TODO/FIXME/HACK/XXX/BUG listing'],
  ['/rewind', 'rewind the last local exchange'],
  ['/branch save <name>', 'snapshot the current session branch'],
  ['/branch restore <name>', 'restore a saved branch snapshot'],
  ['/branch list', 'list saved branch snapshots'],
  ['/branch turn <turn-id>', 'fork the session at a previous turn'],
  ['/branch tree', 'show saved branch tree'],
  ['/branch switch <index>', 'switch to a saved branch limb'],
  ['/session new [task]', 'open a new chat session'],
  ['/session list', 'list open chat sessions'],
  ['/session switch <id|title>', 'switch to another session'],
  ['/session close <id|title>', 'close a chat session'],
  ['/autofix [on|off|N]', 'record auto-fix preference'],
  ['/help', 'show this help'],
];

export interface PreparedTask {
  clientSessionId: string;
  continueSessionId?: string;
}

export class PeridotSidebarProvider implements vscode.WebviewViewProvider {
  public static readonly viewType = 'peridot.chatView';

  private view: vscode.WebviewView | undefined;
  private state: SidebarState = freshState();
  private sessions = new Map<string, StoredChatSession>();
  private nextSessionOrdinal = 1;

  public constructor(
    private readonly extensionUri: vscode.Uri,
    private readonly storage: vscode.Memento,
    private readonly handlers: SidebarHandlers,
  ) {
    this.restorePersistedState();
  }

  public resolveWebviewView(webviewView: vscode.WebviewView): void {
    this.view = webviewView;
    webviewView.webview.options = {
      enableScripts: true,
      localResourceRoots: [
        vscode.Uri.joinPath(this.extensionUri, 'dist'),
        vscode.Uri.joinPath(this.extensionUri, 'resources'),
      ],
    };
    webviewView.webview.html = this.html(webviewView.webview);
    webviewView.onDidDispose(() => {
      if (this.view === webviewView) this.view = undefined;
    });
    webviewView.webview.onDidReceiveMessage((message: OutboundMessage) => {
      void this.receive(message);
    });
    this.publish();
  }

  public prepareForTask(task: string, workspace: string): PreparedTask {
    const session = this.ensureActiveSession();
    const continueSessionId = session.daemonSessionId;
    if (session.title.startsWith('New session')) {
      session.title = taskTitle(task);
    }
    this.state.view = 'session';
    this.state.context = {
      ...this.state.context,
      workspace,
      status: 'Starting daemon',
      problem: undefined,
      running: true,
    };
    this.state.running = true;
    this.state.status = 'Starting daemon';
    this.state.transcript.push({ role: 'user', text: task });
    this.publish();
    return {
      clientSessionId: session.id,
      continueSessionId,
    };
  }

  public setSession(sessionId: string): void {
    this.state.sessionId = sessionId;
    this.state.status = `Running ${sessionId}`;
    this.state.context = { ...this.state.context, status: 'Running', running: true };
    const session = this.activeStoredSession();
    if (session) session.daemonSessionId = sessionId;
    this.publish();
  }

  public appendNotificationFor(
    clientSessionId: string | undefined,
    params: DaemonEventParams,
  ): void {
    const previousActive = this.state.activeChatId;
    const temporarilyLoaded = Boolean(
      clientSessionId && clientSessionId !== previousActive && this.sessions.has(clientSessionId),
    );
    if (temporarilyLoaded) {
      this.saveActiveSession();
      this.loadSessionIntoState(clientSessionId, false);
    }
    const event = params.event;
    if (!isRecord(event)) {
      this.append({ role: 'status', text: 'Event' });
      if (temporarilyLoaded) {
        this.saveActiveSession();
        this.loadSessionIntoState(previousActive, false);
        this.publish();
      }
      return;
    }
    const kind = typeof event.kind === 'string' ? event.kind : '';

    this.applyHudSideEffects(kind, event);

    if (kind === 'tool_started') {
      this.appendToolStarted(event);
    } else if (kind === 'tool_finished') {
      this.appendToolFinished(event);
    } else if (kind === 'approval_requested') {
      this.appendApproval(event);
    } else {
      const item = transcriptItemForEvent(kind, event);
      if (item) this.append(item);
    }

    if (isApprovalWaitingEvent(event)) {
      this.state.running = true;
      this.state.status = 'Waiting for approval';
      this.state.context = {
        ...this.state.context,
        status: this.state.status,
        running: true,
      };
      this.publish();
    }
    if (isTerminalEvent(event)) {
      this.state.running = false;
      this.state.status = isErrorEvent(event) ? 'Failed' : 'Finished';
      this.state.context = {
        ...this.state.context,
        status: this.state.status,
        running: false,
      };
      this.publish();
    }
    if (temporarilyLoaded) {
      this.saveActiveSession();
      this.loadSessionIntoState(previousActive, false);
      this.publish();
    }
  }

  public appendSystem(text: string, detail?: string): void {
    this.append({ role: 'status', text, detail });
  }

  public appendError(text: string): void {
    this.state.running = false;
    this.state.status = 'Failed';
    this.state.context = {
      ...this.state.context,
      status: 'Failed',
      problem: text,
      running: false,
    };
    this.append({ role: 'error', text });
  }

  public markIdle(status = 'Idle'): void {
    this.state.running = false;
    this.state.status = status;
    this.state.context = { ...this.state.context, status, running: false };
    this.publish();
  }

  public setContext(context: SidebarContext): void {
    this.state.context = {
      ...this.state.context,
      ...context,
    };
    this.state.running = Boolean(context.running ?? this.state.running);
    if (context.status) {
      this.state.status = context.status;
    }
    // The landing screen flips to session as soon as we know auth is
    // configured; if it's not configured (or we don't know yet), keep the
    // user on landing so they can pick an auth method.
    if (context.authConfigured === true && this.state.view === 'landing') {
      this.state.view = 'session';
      this.state.landing = 'home';
    } else if (
      context.authConfigured === false &&
      !this.state.running &&
      this.state.transcript.length === 0
    ) {
      this.state.view = 'landing';
    }
    this.publish();
  }

  public setWorkspaceProblem(problem: string): void {
    this.state.context = {
      ...this.state.context,
      status: 'Needs attention',
      problem,
      running: false,
    };
    this.state.status = 'Needs attention';
    this.state.running = false;
    this.publish();
  }

  public setAuthBusy(busy: boolean, error?: string): void {
    this.state.authBusy = busy;
    if (error !== undefined) this.state.authError = error;
    this.publish();
  }

  /** Pulls the head of the queue and signals the host to run it. */
  public takeNextQueued(): QueuedMessage | undefined {
    const next = this.state.queue.shift();
    if (next) this.publish();
    return next;
  }

  public hasQueue(): boolean {
    return this.state.queue.length > 0;
  }

  public currentRunOptions(): RunOptions {
    return { ...this.state.runOptions };
  }

  public currentDaemonSessionId(): string | undefined {
    return this.state.sessionId;
  }

  public appendCommandResult(result: CommandResultView): void {
    this.append({
      role: result.severity === 'error' ? 'error' : 'command',
      text: result.message ?? result.title ?? result.kind ?? 'Command',
      commandResult: result,
    });
  }

  public appendAuthLink(url: string): void {
    this.append({
      role: 'assistant',
      text: [
        'ChatGPT 로그인 브라우저가 자동으로 열리지 않으면 아래 링크를 열어주세요.',
        '',
        `[Sign in with ChatGPT](${url})`,
        '',
        url,
      ].join('\n'),
    });
  }

  public createNewSession(workspace?: string): void {
    this.saveActiveSession();
    const session = this.createSession();
    this.state.activeChatId = session.id;
    this.state.view = 'session';
    this.state.sessionId = undefined;
    this.state.status = session.status;
    this.state.running = false;
    this.state.transcript = session.transcript;
    this.state.hud = session.hud;
    this.state.context = {
      ...this.state.context,
      ...(workspace ? { workspace } : {}),
      status: session.status,
      running: false,
      problem: undefined,
    };
    this.publish();
  }

  public selectSession(id: string): void {
    this.saveActiveSession();
    this.loadSessionIntoState(id, true);
  }

  private appendToolStarted(event: Record<string, unknown>): void {
    const name = stringField(event, 'name');
    if (name === 'agent_ask_user') {
      this.append({
        role: 'tool',
        text: name,
        detail: compactAskUserToolDetail(event.parameters),
        toolName: name,
        pending: true,
      });
      return;
    }
    this.append({
      role: 'tool',
      text: name,
      detail: undefined,
      toolName: name,
      path: pickString(event.parameters, 'path'),
      line: pickNumber(event.parameters, 'line') ?? pickNumber(event.parameters, 'start_line'),
      column: pickNumber(event.parameters, 'column'),
      toolParameters: event.parameters,
      pending: true,
    });
  }

  private appendToolFinished(event: Record<string, unknown>): void {
    const name = stringField(event, 'name');
    const summary = summarizeToolResult(event);
    for (let i = this.state.transcript.length - 1; i >= 0; i -= 1) {
      const item = this.state.transcript[i];
      if (item.role === 'tool' && item.pending && (item.toolName === name || !item.toolName)) {
        item.pending = false;
        item.text = name;
        item.toolResultSummary = summary;
        item.detail = undefined;
        this.publish();
        return;
      }
    }
    this.append({
      role: 'tool',
      text: name,
      detail: summary,
      toolName: name,
      pending: false,
      toolResultSummary: summary,
    });
  }

  private appendApproval(event: Record<string, unknown>): void {
    const toolName = stringField(event, 'tool_name');
    const reason = stringField(event, 'reason');
    const parameters = event.parameters;
    const item: TranscriptItem = {
      role: 'approval',
      text: toolName,
      detail: reason,
      toolName,
      reason,
      parameters,
    };
    const path = pickString(parameters, 'path');
    if (path) {
      item.path = path;
    }
    this.append(item);

    if ((toolName === 'file_write' || toolName === 'file_patch') && path) {
      void this.enrichApprovalDiff(item, toolName, parameters);
    }
  }

  private async enrichApprovalDiff(
    item: TranscriptItem,
    toolName: string,
    parameters: unknown,
  ): Promise<void> {
    const path = item.path;
    if (!path) return;
    const before = await readWorkspaceFile(path);
    if (toolName === 'file_write') {
      const after = pickString(parameters, 'content') ?? '';
      item.before = before;
      item.after = after;
    } else if (toolName === 'file_patch') {
      const oldText = pickString(parameters, 'old_text') ?? '';
      const newText = pickString(parameters, 'new_text') ?? '';
      item.before = before;
      item.after =
        typeof before === 'string'
          ? before.includes(oldText)
            ? before.replace(oldText, newText)
            : `${before}\n${newText}`
          : newText;
    }
    this.publish();
  }

  private applyHudSideEffects(kind: string, event: Record<string, unknown>): void {
    switch (kind) {
      case 'usage_updated': {
        const usage = isRecord(event.usage) ? event.usage : undefined;
        if (usage) {
          const next: UsageSlice = {
            inputTokens: numberField(usage, 'input_tokens'),
            outputTokens: numberField(usage, 'output_tokens'),
            cacheReadTokens: optionalNumber(usage, 'cache_read_input_tokens'),
            cacheCreationTokens: optionalNumber(usage, 'cache_creation_input_tokens'),
            costUsd: optionalNumber(usage, 'estimated_cost_usd'),
          };
          this.state.hud.usage = next;
          this.publish();
        }
        return;
      }
      case 'budget_updated': {
        const next: BudgetSlice = {
          costUsed: numberField(event, 'cost_used'),
          turnsUsed: numberField(event, 'turns_used'),
        };
        const costLimit = optionalNumber(event, 'cost_limit');
        const turnsLimit = optionalNumber(event, 'turns_limit');
        if (typeof costLimit === 'number') next.costLimit = costLimit;
        if (typeof turnsLimit === 'number') next.turnsLimit = turnsLimit;
        this.state.hud.budget = next;
        this.publish();
        return;
      }
      case 'context_utilization_changed': {
        const next: ContextSlice = {
          tokensUsed: numberField(event, 'tokens_used'),
          threshold: numberField(event, 'threshold'),
        };
        this.state.hud.context = next;
        this.publish();
        return;
      }
      case 'plan_updated': {
        const stepsRaw = Array.isArray(event.steps) ? event.steps : [];
        const steps: PlanStepView[] = stepsRaw.map((entry) => {
          if (!isRecord(entry)) return { text: stringField({ value: entry }, 'value') };
          return {
            text: stringField(entry, 'text'),
            status: typeof entry.status === 'string' ? entry.status : undefined,
          };
        });
        const next: PlanSlice = { steps };
        const current = optionalNumber(event, 'current');
        if (typeof current === 'number') next.current = current;
        this.state.hud.plan = next;
        this.publish();
        return;
      }
      case 'committee_role_usage': {
        const role = stringField(event, 'role');
        if (!role) return;
        const slice: CommitteeRoleSlice = {
          tokens: numberField(event, 'tokens'),
          costUsd: numberField(event, 'cost_usd'),
        };
        const prev = this.state.hud.committee?.[role];
        this.state.hud.committee = {
          ...(this.state.hud.committee ?? {}),
          [role]: {
            tokens: (prev?.tokens ?? 0) + slice.tokens,
            costUsd: (prev?.costUsd ?? 0) + slice.costUsd,
          },
        };
        this.publish();
        return;
      }
      default:
        return;
    }
  }

  private async receive(message: OutboundMessage): Promise<void> {
    switch (message.type) {
      case 'ready':
        this.publish();
        return;
      case 'run': {
        const task = message.task.trim();
        if (task.length === 0) return;
        this.state.runOptions = message.options;
        if (task.startsWith('/')) {
          await this.handleSlashCommand(task, message.options);
          return;
        }
        await this.handlers.runTask(task, message.options);
        return;
      }
      case 'cancel':
        await this.handlers.cancelTask();
        return;
      case 'loginOpenAi':
        await this.handlers.loginOpenAi();
        return;
      case 'refreshStatus':
        await this.handlers.refreshStatus();
        return;
      case 'askUserRespond':
        if (message.requestId) {
          await this.handlers.respondAskUser(message.requestId, message.answer);
          this.resolveInteraction(message.requestId, answerLabel(message.answer));
        }
        return;
      case 'approvalRespond':
        await this.handlers.respondApproval({
          approved: message.approved,
          scope: message.scope,
          toolName: message.toolName,
          reason: message.reason,
          parameters: message.parameters,
        });
        this.resolveApproval(message.approved);
        return;
      case 'openFile':
        await this.handlers.openFile(message.path, message.line, message.column);
        return;
      case 'registerProvider':
        await this.handlers.registerProvider(message.provider, message.params);
        return;
      case 'showLanding':
        this.state.view = 'landing';
        this.state.landing = message.screen ?? 'home';
        this.publish();
        return;
      case 'showSession':
        this.state.view = 'session';
        this.publish();
        return;
      case 'newSession':
        this.createNewSession(this.state.context.workspace);
        return;
      case 'selectSession':
        this.selectSession(message.id);
        return;
      case 'queueAdd':
        if (message.task.trim().length > 0) {
          this.state.queue = [...this.state.queue, { id: queueId(), text: message.task.trim() }];
          this.publish();
        }
        return;
      case 'queueRemove':
        this.state.queue = this.state.queue.filter((item) => item.id !== message.id);
        this.publish();
        return;
      case 'queueEdit':
        this.state.queue = this.state.queue.map((item) =>
          item.id === message.id ? { ...item, text: message.text } : item,
        );
        this.publish();
        return;
      case 'queueClear':
        this.state.queue = [];
        this.publish();
        return;
    }
  }

  private append(item: TranscriptItem): void {
    if (shouldSuppress(item)) {
      return;
    }
    const last = this.state.transcript[this.state.transcript.length - 1];
    if (item.role === 'assistant' && last?.role === 'assistant') {
      last.text += item.text;
    } else {
      this.state.transcript.push(item);
    }
    this.publish();
  }

  private resolveInteraction(requestId: string, detail: string): void {
    const item = this.state.transcript.find((entry) => entry.requestId === requestId);
    if (item) {
      item.role = 'status';
      item.text = 'User response sent';
      item.detail = detail;
      item.request = undefined;
    }
    this.publish();
  }

  private resolveApproval(approved: boolean): void {
    const item = [...this.state.transcript].reverse().find((entry) => entry.role === 'approval');
    if (item) {
      item.role = 'status';
      item.text = approved ? 'Approval sent' : 'Approval denied';
      item.detail = item.toolName;
    }
    this.publish();
  }

  private publish(): void {
    this.saveActiveSession();
    this.refreshSessionSummaries();
    this.persistState();
    const message: InboundMessage = { type: 'state', state: this.state };
    this.view?.webview.postMessage(message);
  }

  private async handleSlashCommand(input: string, options: RunOptions): Promise<void> {
    const [command, ...restParts] = input.slice(1).trim().split(/\s+/);
    const rest = restParts.join(' ').trim();
    switch (command) {
      case 'plan':
        if (rest.length === 0) {
          this.updateRunOptions({ ...options, mode: 'plan' }, 'mode: plan');
          return;
        }
        if (rest === 'show') {
          this.showPlan();
          return;
        }
        break;
      case 'execute':
        if (rest.length === 0) {
          this.updateRunOptions({ ...options, mode: 'execute' }, 'mode: execute');
          return;
        }
        break;
      case 'goal':
        await this.handleGoalSlash(rest, options);
        return;
      case 'safe':
        if (rest.length === 0) {
          this.updateRunOptions({ ...options, permission: 'safe' }, 'permission: safe');
          return;
        }
        break;
      case 'auto':
        if (rest.length === 0) {
          this.updateRunOptions({ ...options, permission: 'auto' }, 'permission: auto');
          return;
        }
        break;
      case 'yolo':
        if (rest.length === 0) {
          this.updateRunOptions({ ...options, permission: 'yolo' }, 'permission: yolo');
          return;
        }
        break;
      case 'model':
        if (rest.length > 0) {
          this.updateRunOptions({ ...options, model: rest }, `model: ${rest}`);
          return;
        }
        this.appendError('Usage: /model <name>');
        return;
      case 'provider':
        if (rest.length > 0) {
          this.state.context = { ...this.state.context, provider: rest };
          this.append({ role: 'status', text: `provider: ${rest}` });
          this.publish();
          return;
        }
        this.appendError('Usage: /provider <name>');
        return;
      case 'reasoning': {
        const effort = parseReasoningEffort(rest);
        if (!effort) {
          this.appendError('Usage: /reasoning <off|low|medium|high>');
          return;
        }
        this.updateRunOptions({ ...options, reasoningEffort: effort }, `reasoning: ${effort}`);
        return;
      }
      case 'think': {
        const effort = parseThinkEffort(rest);
        if (!effort) {
          this.appendError('Usage: /think [off|low|medium|high]');
          return;
        }
        this.updateRunOptions({ ...options, reasoningEffort: effort }, `reasoning: ${effort}`);
        return;
      }
      case 'fast': {
        const tier = parseFastTier(rest, options.serviceTier);
        if (!tier) {
          this.appendError('Usage: /fast [on|off|toggle]');
          return;
        }
        this.updateRunOptions({ ...options, serviceTier: tier }, `service tier: ${tier}`);
        return;
      }
      case 'note':
        if (rest.length > 0) {
          this.append({ role: 'status', text: `note: ${rest}` });
          return;
        }
        this.appendError('Usage: /note <text>');
        return;
      case 'info':
        if (rest.length === 0) {
          this.showInfo();
          return;
        }
        break;
      case 'committee':
        if (['off', 'planner', 'full'].includes(rest)) {
          this.append({ role: 'status', text: `committee: ${rest}` });
          return;
        }
        this.appendError('Usage: /committee <off|planner|full>');
        return;
      case 'cost':
        if (rest.length === 0) {
          this.showCost();
          return;
        }
        break;
      case 'compact':
        if (rest.length === 0) {
          await this.executeDaemonSlash(input, options);
          return;
        }
        break;
      case 'context':
        if (rest === 'top' || rest.length === 0) {
          await this.executeDaemonSlash(input, options);
          return;
        }
        break;
      case 'sidepanel':
      case 'status':
        if (rest.length === 0) {
          this.showInfo();
          return;
        }
        break;
      case 'collapse':
        if (rest.length === 0) {
          this.append({ role: 'status', text: 'collapse: compact transcript layout is active' });
          return;
        }
        break;
      case 'diff':
        if (rest.length === 0) {
          await this.executeDaemonSlash(input, options);
          return;
        }
        break;
      case 'undo':
        if (rest.length === 0) {
          await this.executeDaemonSlash(input, options);
          return;
        }
        break;
      case 'lang':
        if (rest === 'en' || rest === 'ko') {
          this.append({ role: 'status', text: `language: ${rest}` });
          return;
        }
        this.appendError('Usage: /lang en|ko');
        return;
      case 'clear':
        if (rest.length > 0) {
          this.appendError('Usage: /clear');
          return;
        }
        await this.handlers.clearSession();
        this.clearActiveSession();
        this.append({ role: 'status', text: 'clear: transcript + context wiped, new session' });
        return;
      case 'fork':
      case 'teammate':
        if (rest.length > 0) {
          await this.executeDaemonSlash(input, options);
          return;
        }
        this.appendError(`Usage: /${command} <task>`);
        return;
      case 'worktree':
        if (rest.length > 0) {
          await this.executeDaemonSlash(input, options);
          return;
        }
        this.appendError('Usage: /worktree <branch> <task>');
        return;
      case 'subagent':
        if (rest.startsWith('model ') && rest.slice('model '.length).trim().length > 0) {
          this.append({ role: 'status', text: `subagent model: ${rest.slice('model '.length).trim()}` });
          return;
        }
        this.appendError('Usage: /subagent model <name|reset>');
        return;
      case 'mcp':
        await this.executeDaemonSlash(input, options);
        return;
      case 'todos':
        if (rest.length === 0) {
          await this.executeDaemonSlash(input, options);
          return;
        }
        break;
      case 'rewind':
        if (rest.length === 0) {
          this.rewindLastExchange();
          return;
        }
        break;
      case 'branch':
        await this.executeDaemonSlash(input, options);
        return;
      case 'autofix':
        if (rest.length === 0 || rest === 'on' || rest === 'off' || /^\d+$/.test(rest)) {
          this.append({ role: 'status', text: `autofix: ${rest || 'toggle'}` });
          return;
        }
        this.appendError('Usage: /autofix [on|off|N]');
        return;
      case 'session':
        await this.handleSessionSlash(rest, options);
        return;
      case 'help':
        if (rest.length === 0) {
          this.append({ role: 'assistant', text: slashHelpText() });
          return;
        }
        break;
      default:
        break;
    }
    this.appendError(`Unknown command: /${command}`);
    this.append({ role: 'status', text: 'Type /help for available commands' });
  }

  private async handleGoalSlash(rest: string, options: RunOptions): Promise<void> {
    const next = { ...options, mode: 'goal' as const };
    switch (rest) {
      case '':
        this.updateRunOptions(next, 'mode: goal');
        return;
      case 'pause':
      case 'resume':
      case 'clear':
      case 'status':
        this.updateRunOptions(next, `goal ${rest}`);
        return;
      default:
        this.state.runOptions = next;
        this.state.context = {
          ...this.state.context,
          mode: 'goal',
          permission: next.permission,
          model: next.model ?? this.state.context.model,
          reasoningEffort: next.reasoningEffort,
          serviceTier: next.serviceTier,
        };
        await this.handlers.runTask(rest, next);
        return;
    }
  }

  private async handleSessionSlash(rest: string, options: RunOptions): Promise<void> {
    const [subcommand, ...tailParts] = rest.split(/\s+/).filter(Boolean);
    const tail = tailParts.join(' ').trim();
    switch (subcommand) {
      case 'new':
        this.createNewSession(this.state.context.workspace);
        if (tail.length > 0) {
          await this.handlers.runTask(tail, options);
        }
        return;
      case 'list': {
        this.refreshSessionSummaries();
        const lines = this.state.sessions.map((session) => {
          const marker = session.active ? '*' : '-';
          const running = session.running ? ' running' : '';
          return `${marker} ${session.title} [${session.id}]${running}`;
        });
        this.append({ role: 'assistant', text: lines.length > 0 ? lines.join('\n') : 'No sessions' });
        return;
      }
      case 'save':
        this.append({ role: 'status', text: 'session saved' });
        return;
      case 'switch': {
        const session = this.findSession(tail);
        if (!session) {
          this.appendError('Usage: /session switch <id|title>');
          return;
        }
        this.selectSession(session.id);
        return;
      }
      case 'close': {
        if (!this.closeSession(tail)) {
          this.appendError('Usage: /session close <id|title>');
        }
        return;
      }
      default:
        this.appendError(
          'Usage: /session new [task] | /session list | /session switch <id|title> | /session close <id|title> | /session save',
        );
        return;
    }
  }

  private updateRunOptions(next: RunOptions, notice: string): void {
    this.state.runOptions = next;
    this.state.context = {
      ...this.state.context,
      mode: next.mode,
      permission: next.permission,
      model: next.model ?? this.state.context.model,
      reasoningEffort: next.reasoningEffort,
      serviceTier: next.serviceTier,
    };
    this.append({ role: 'status', text: notice });
  }

  private async executeDaemonSlash(input: string, options: RunOptions): Promise<void> {
    try {
      const result = await this.handlers.runSlashCommand(input, options);
      if (result.action === 'clear') {
        await this.handlers.clearSession();
        this.clearActiveSession();
        this.append({ role: 'status', text: 'clear: transcript + context wiped, new session' });
        return;
      }
      this.appendCommandResult(result);
      if (result.kind === 'start_task' && result.task) {
        await this.handlers.runTask(result.task, options);
      }
    } catch (err) {
      this.appendError(err instanceof Error ? err.message : String(err));
    }
  }

  private showInfo(): void {
    const c = this.state.context;
    const o = this.state.runOptions;
    this.append({
      role: 'assistant',
      text: [
        `Status: ${this.state.status}`,
        `Workspace: ${c.workspace ?? 'unknown'}`,
        `Provider: ${c.provider ?? 'unknown'}`,
        `Model: ${o.model ?? c.model ?? 'default'}`,
        `Mode: ${o.mode}`,
        `Permission: ${o.permission}`,
        `Reasoning: ${o.reasoningEffort ?? c.reasoningEffort ?? 'default'}`,
        `Service tier: ${o.serviceTier ?? c.serviceTier ?? 'standard'}`,
        `Sessions: ${this.sessions.size}`,
      ].join('\n'),
    });
  }

  private showCost(): void {
    const usage = this.state.hud.usage;
    if (!usage) {
      this.append({ role: 'assistant', text: 'No usage data yet.' });
      return;
    }
    this.append({
      role: 'assistant',
      text: [
        `Input tokens: ${usage.inputTokens}`,
        `Output tokens: ${usage.outputTokens}`,
        `Cache read tokens: ${usage.cacheReadTokens ?? 0}`,
        `Cache creation tokens: ${usage.cacheCreationTokens ?? 0}`,
        `Estimated cost: $${(usage.costUsd ?? 0).toFixed(4)}`,
      ].join('\n'),
    });
  }

  private showPlan(): void {
    const steps = this.state.hud.plan?.steps ?? [];
    if (steps.length === 0) {
      this.append({ role: 'assistant', text: 'No plan steps yet.' });
      return;
    }
    this.append({
      role: 'assistant',
      text: steps.map((step, index) => `${index + 1}. ${step.status ?? 'pending'} ${step.text}`).join('\n'),
    });
  }

  private rewindLastExchange(): void {
    const lastUser = this.state.transcript.map((item) => item.role).lastIndexOf('user');
    if (lastUser < 0) {
      this.append({ role: 'status', text: 'rewind: nothing to rewind' });
      return;
    }
    this.state.transcript = this.state.transcript.slice(0, lastUser);
    this.append({ role: 'status', text: 'rewind: last exchange removed' });
  }

  private findSession(query: string): StoredChatSession | undefined {
    const needle = query.trim().toLowerCase();
    if (!needle) return undefined;
    return Array.from(this.sessions.values()).find(
      (session) =>
        session.id.toLowerCase() === needle ||
        session.title.toLowerCase() === needle ||
        session.title.toLowerCase().includes(needle),
    );
  }

  private closeSession(query: string): boolean {
    const session =
      query.trim().length > 0
        ? this.findSession(query)
        : this.activeStoredSession();
    if (!session) return false;
    if (session.running) {
      this.appendError('Cannot close a running session.');
      return true;
    }

    const wasActive = session.id === this.state.activeChatId;
    this.sessions.delete(session.id);
    if (wasActive) {
      const next = this.sessions.values().next().value as StoredChatSession | undefined;
      if (next) {
        this.loadSessionIntoState(next.id, false);
      } else {
        const replacement = this.createSession();
        this.state.activeChatId = replacement.id;
        this.state.sessionId = undefined;
        this.state.status = replacement.status;
        this.state.running = false;
        this.state.transcript = replacement.transcript;
        this.state.hud = replacement.hud;
      }
    }
    this.append({ role: 'status', text: `session closed: ${session.title}` });
    return true;
  }

  private ensureActiveSession(): StoredChatSession {
    if (this.state.activeChatId && this.sessions.has(this.state.activeChatId)) {
      return this.sessions.get(this.state.activeChatId)!;
    }
    const session = this.createSession();
    this.state.activeChatId = session.id;
    this.state.sessionId = session.daemonSessionId;
    this.state.transcript = session.transcript;
    this.state.hud = session.hud;
    this.state.status = session.status;
    this.state.running = session.running;
    return session;
  }

  private activeStoredSession(): StoredChatSession | undefined {
    return this.state.activeChatId ? this.sessions.get(this.state.activeChatId) : undefined;
  }

  private createSession(title?: string): StoredChatSession {
    const id = `chat-${Date.now()}-${this.nextSessionOrdinal}`;
    const session: StoredChatSession = {
      id,
      title: title ?? `New session ${this.nextSessionOrdinal}`,
      status: 'Idle',
      running: false,
      transcript: [],
      hud: {},
    };
    this.nextSessionOrdinal += 1;
    this.sessions.set(id, session);
    return session;
  }

  private saveActiveSession(): void {
    const id = this.state.activeChatId;
    if (!id) return;
    const session = this.sessions.get(id);
    if (!session) return;
    session.daemonSessionId = this.state.sessionId;
    session.status = this.state.status;
    session.running = this.state.running;
    session.transcript = this.state.transcript;
    session.hud = this.state.hud;
  }

  private restorePersistedState(): void {
    const snapshot = this.storage.get<PersistedSidebarSnapshot>(PERSISTENCE_KEY);
    if (!snapshot || snapshot.version !== 1 || !Array.isArray(snapshot.sessions)) {
      return;
    }
    this.sessions.clear();
    for (const raw of snapshot.sessions) {
      const session: StoredChatSession = {
        ...raw,
        status: raw.running ? 'Idle' : raw.status,
        running: false,
        transcript: Array.isArray(raw.transcript) ? raw.transcript : [],
        hud: raw.hud ?? {},
      };
      this.sessions.set(session.id, session);
    }
    this.nextSessionOrdinal = Math.max(1, snapshot.nextSessionOrdinal);
    this.state = {
      ...freshState(),
      view: snapshot.view ?? 'landing',
      landing: snapshot.landing ?? 'home',
      activeChatId: snapshot.activeChatId,
      context: {
        ...(snapshot.context ?? {}),
        status: 'Idle',
        running: false,
        problem: undefined,
      },
      queue: Array.isArray(snapshot.queue) ? snapshot.queue : [],
      runOptions: snapshot.runOptions ?? freshState().runOptions,
    };
    if (this.state.activeChatId && this.sessions.has(this.state.activeChatId)) {
      this.loadSessionIntoState(this.state.activeChatId, false);
    }
    this.refreshSessionSummaries();
  }

  private persistState(): void {
    const sessions = Array.from(this.sessions.values()).map(
      (session): StoredChatSession => ({
        ...session,
        status: session.running ? 'Idle' : session.status,
        running: false,
      }),
    );
    const snapshot: PersistedSidebarSnapshot = {
      version: 1,
      activeChatId: this.state.activeChatId,
      nextSessionOrdinal: this.nextSessionOrdinal,
      runOptions: this.state.runOptions,
      context: {
        ...this.state.context,
        status: this.state.running ? 'Idle' : this.state.status,
        running: false,
      },
      view: this.state.view,
      landing: this.state.landing,
      queue: this.state.queue,
      sessions,
    };
    void this.storage.update(PERSISTENCE_KEY, snapshot);
  }

  private loadSessionIntoState(id: string | undefined, publish: boolean): void {
    if (!id) return;
    const session = this.sessions.get(id);
    if (!session) return;
    this.state.activeChatId = id;
    this.state.sessionId = session.daemonSessionId;
    this.state.status = session.status;
    this.state.running = session.running;
    this.state.transcript = session.transcript;
    this.state.hud = session.hud;
    this.state.context = {
      ...this.state.context,
      status: session.status,
      running: session.running,
      problem: undefined,
    };
    this.state.view = 'session';
    if (publish) this.publish();
  }

  private refreshSessionSummaries(): void {
    const active = this.state.activeChatId;
    this.state.sessions = Array.from(this.sessions.values()).map(
      (session): ChatSessionSummary => ({
        id: session.id,
        title: session.title,
        status: session.status,
        running: session.running,
        active: session.id === active,
      }),
    );
  }

  private clearActiveSession(): void {
    this.ensureActiveSession();
    this.state.sessionId = undefined;
    this.state.status = 'Idle';
    this.state.running = false;
    this.state.transcript = [];
    this.state.hud = {};
    this.state.context = { ...this.state.context, status: 'Idle', running: false };
    const session = this.activeStoredSession();
    if (session) {
      session.daemonSessionId = undefined;
      session.status = 'Idle';
      session.running = false;
      session.transcript = [];
      session.hud = {};
    }
    this.publish();
  }

  private html(webview: vscode.Webview): string {
    const nonce = nonceValue();
    const scriptUri = webview.asWebviewUri(
      vscode.Uri.joinPath(this.extensionUri, 'dist', 'webview.js'),
    );
    const styleUri = webview.asWebviewUri(
      vscode.Uri.joinPath(this.extensionUri, 'dist', 'webview.css'),
    );
    const iconUri = webview.asWebviewUri(
      vscode.Uri.joinPath(this.extensionUri, 'resources', 'peridot-icon.png'),
    );
    return /* html */ `<!DOCTYPE html>
<html lang="en">
<head>
  <meta charset="UTF-8">
  <meta
    http-equiv="Content-Security-Policy"
    content="default-src 'none'; img-src ${webview.cspSource} data: https: http:; style-src ${webview.cspSource} 'unsafe-inline'; script-src 'nonce-${nonce}';"
  >
  <meta name="viewport" content="width=device-width, initial-scale=1.0">
  <title>Peridot</title>
  <link href="${styleUri}" rel="stylesheet" />
</head>
<body>
  <div class="app" id="app" data-mascot="${iconUri}">
    <!-- Webview bundle owns layout; index.ts populates #app based on
         the SidebarState received over postMessage. -->
  </div>
  <script nonce="${nonce}" src="${scriptUri}"></script>
</body>
</html>`;
  }
}

function slashHelpText(): string {
  return [
    'Slash commands:',
    ...EXTENSION_SLASH_COMMANDS.map(([command, description]) => `- \`${command}\` - ${description}`),
  ].join('\n');
}

function parseReasoningEffort(value: string): RunOptions['reasoningEffort'] | undefined {
  switch (value.trim().toLowerCase()) {
    case 'off':
    case 'none':
    case 'false':
    case '0':
      return 'off';
    case 'low':
    case 'min':
    case 'minimal':
      return 'low';
    case 'medium':
    case 'med':
    case 'default':
    case 'true':
      return 'medium';
    case 'high':
    case 'max':
    case 'maximum':
      return 'high';
    default:
      return undefined;
  }
}

function parseThinkEffort(value: string): RunOptions['reasoningEffort'] | undefined {
  const trimmed = value.trim().toLowerCase();
  if (!trimmed || trimmed === 'hard' || trimmed === 'harder' || trimmed === 'more') {
    return 'high';
  }
  if (trimmed === 'stop' || trimmed === 'less') {
    return 'off';
  }
  return parseReasoningEffort(trimmed);
}

function parseFastTier(
  value: string,
  current: RunOptions['serviceTier'],
): RunOptions['serviceTier'] | undefined {
  switch (value.trim().toLowerCase()) {
    case '':
    case 'on':
    case 'fast':
    case 'priority':
      return 'fast';
    case 'off':
    case 'standard':
    case 'default':
    case 'none':
      return 'standard';
    case 'toggle':
      return current === 'fast' ? 'standard' : 'fast';
    default:
      return undefined;
  }
}

function freshState(): SidebarState {
  return {
    view: 'landing',
    landing: 'home',
    running: false,
    status: 'Idle',
    context: {},
    transcript: [],
    sessions: [],
    queue: [],
    runOptions: {
      mode: 'execute',
      permission: 'auto',
    },
    hud: {} as HudState,
    authBusy: false,
  };
}

function shouldSuppress(item: TranscriptItem): boolean {
  if (item.role !== 'status') return false;
  const noisy = ['agents md loaded', 'turn started', 'turn ended', 'assistant started'];
  const lowered = item.text.toLowerCase();
  return noisy.some((needle) => lowered.includes(needle));
}

function transcriptItemForEvent(
  kind: string,
  event: Record<string, unknown>,
): TranscriptItem | undefined {
  switch (kind) {
    case 'started':
      return { role: 'status', text: 'Daemon started' };
    case 'run_started':
      return undefined;
    case 'agents_md_loaded':
    case 'turn_started':
    case 'turn_ended':
    case 'assistant_started':
    case 'assistant_finished':
    case 'context_utilization_changed':
    case 'usage_updated':
    case 'budget_updated':
    case 'committee_role_usage':
      return undefined;
    case 'assistant_delta':
      return { role: 'assistant', text: stringField(event, 'delta') };
    case 'thinking':
      return { role: 'assistant', text: stringField(event, 'text') };
    case 'ask_user_requested':
      return {
        role: 'interaction',
        text: questionForAskUser(event.request),
        detail: stringField(event, 'request_id'),
        requestId: stringField(event, 'request_id'),
        request: event.request,
      };
    case 'approval_waiting':
      return { role: 'status', text: 'Waiting for approval' };
    case 'approval_resumed':
      return {
        role: 'status',
        text: 'Approval accepted',
        detail: `scope ${stringField(event, 'scope')}`,
      };
    case 'approval_denied':
      return { role: 'error', text: 'Approval denied' };
    case 'plan_updated':
      return { role: 'status', text: 'Plan updated' };
    case 'command_result':
      return undefined;
    case 'file_diff': {
      const path = stringField(event, 'path');
      return {
        role: 'diff',
        text: path || 'file',
        detail: stringField(event, 'tool_name'),
        path,
        before: typeof event.before === 'string' ? event.before : null,
        after: typeof event.after === 'string' ? event.after : '',
      };
    }
    case 'finished':
      return undefined;
    case 'error':
      return { role: 'error', text: stringField(event, 'message') };
    case 'recovery':
      return { role: 'error', text: stringField(event, 'message') || 'Recovery failed' };
    case 'interrupted':
      return { role: 'status', text: 'Interrupted', detail: stringField(event, 'stage') };
    default:
      return { role: 'status', text: kind };
  }
}

function questionForAskUser(request: unknown): string {
  if (isRecord(request) && typeof request.question === 'string') {
    return request.question;
  }
  return 'Peridot needs your input';
}

function compactAskUserToolDetail(value: unknown): string {
  if (!isRecord(value)) {
    return '';
  }
  const request = value.request;
  if (!isRecord(request)) {
    return '';
  }
  const question = typeof request.question === 'string' ? request.question : '';
  const kind = typeof request.kind === 'string' ? request.kind : 'ask_user';
  const options = Array.isArray(request.options)
    ? request.options.filter((item): item is string => typeof item === 'string')
    : [];
  const optionLabel = options.length > 0 ? ` · ${options.join(' / ')}` : '';
  return [kind, question].filter(Boolean).join(': ') + optionLabel;
}

function summarizeToolResult(event: Record<string, unknown>): string {
  const result = event.result;
  if (isRecord(result)) {
    const summary = result.summary;
    if (typeof summary === 'string') {
      return summary;
    }
  }
  return json(event.result ?? event);
}

function isTerminalEvent(event: unknown): boolean {
  return (
    isRecord(event) &&
    (event.kind === 'finished' || event.kind === 'error' || event.kind === 'approval_denied')
  );
}

function isErrorEvent(event: unknown): boolean {
  return isRecord(event) && (event.kind === 'error' || event.kind === 'approval_denied');
}

function isApprovalWaitingEvent(event: unknown): boolean {
  return (
    isRecord(event) &&
    (event.kind === 'approval_requested' || event.kind === 'approval_waiting')
  );
}

function stringField(record: Record<string, unknown>, key: string): string {
  const value = record[key];
  return typeof value === 'string' ? value : json(value);
}

function numberField(record: Record<string, unknown>, key: string): number {
  const value = record[key];
  return typeof value === 'number' ? value : 0;
}

function optionalNumber(record: Record<string, unknown>, key: string): number | undefined {
  const value = record[key];
  return typeof value === 'number' ? value : undefined;
}

function pickString(value: unknown, key: string): string | undefined {
  if (!isRecord(value)) return undefined;
  const inner = value[key];
  return typeof inner === 'string' ? inner : undefined;
}

function pickNumber(value: unknown, key: string): number | undefined {
  if (!isRecord(value)) return undefined;
  const inner = value[key];
  return typeof inner === 'number' && Number.isFinite(inner) ? inner : undefined;
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

function answerLabel(answer: AskUserAnswer): string {
  switch (answer.kind) {
    case 'selected':
      return answer.text;
    case 'multi_selected':
      return answer.indices.join(', ');
    case 'text':
      return answer.text;
    case 'cancelled':
      return 'cancelled';
  }
}

function nonceValue(): string {
  const alphabet = 'ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789';
  let value = '';
  for (let i = 0; i < 32; i++) {
    value += alphabet[Math.floor(Math.random() * alphabet.length)];
  }
  return value;
}

function queueId(): string {
  return `q-${Date.now()}-${Math.floor(Math.random() * 1e6)}`;
}

function taskTitle(task: string): string {
  const title = task.replace(/\s+/g, ' ').trim();
  return title.length > 42 ? `${title.slice(0, 39)}...` : title || 'New session';
}

async function readWorkspaceFile(relativePath: string): Promise<string | null> {
  const folder = vscode.workspace.workspaceFolders?.[0];
  if (!folder) return null;
  try {
    const uri = vscode.Uri.joinPath(folder.uri, relativePath);
    const bytes = await vscode.workspace.fs.readFile(uri);
    return new TextDecoder('utf-8').decode(bytes);
  } catch {
    return null;
  }
}
