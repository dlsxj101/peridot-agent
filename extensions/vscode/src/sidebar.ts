import * as vscode from 'vscode';
import {
  ApprovalResponse,
  AskUserAnswer,
  BudgetSlice,
  ChatSessionSummary,
  CommitteeRoleSlice,
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
  cancelTask: () => Promise<void>;
  loginOpenAi: () => Promise<void>;
  refreshStatus: () => Promise<void>;
  respondAskUser: (requestId: string, answer: AskUserAnswer) => Promise<void>;
  respondApproval: (decision: ApprovalResponse) => Promise<void>;
  openFile: (relativePath: string) => Promise<void>;
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
    private readonly handlers: SidebarHandlers,
  ) {}

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
        if (task === '/clear') {
          this.clearActiveSession();
          return;
        }
        this.state.runOptions = message.options;
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
        await this.handlers.openFile(message.path);
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
    const message: InboundMessage = { type: 'state', state: this.state };
    this.view?.webview.postMessage(message);
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
      return { role: 'status', text: 'Run started' };
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
