import * as vscode from 'vscode';

export interface SidebarHandlers {
  runTask: (task: string, options: RunOptions) => Promise<void>;
  cancelTask: () => Promise<void>;
  loginOpenAi: () => Promise<void>;
  refreshStatus: () => Promise<void>;
  respondAskUser: (requestId: string, answer: AskUserAnswer) => Promise<void>;
  respondApproval: (decision: ApprovalResponse) => Promise<void>;
}

export interface RunOptions {
  mode: 'execute' | 'plan' | 'goal';
  permission: 'auto' | 'safe' | 'yolo';
  model?: string;
}

export type AskUserAnswer =
  | { kind: 'selected'; index: number; text: string }
  | { kind: 'multi_selected'; indices: number[] }
  | { kind: 'text'; text: string }
  | { kind: 'cancelled' };

export interface ApprovalResponse {
  approved: boolean;
  scope: 'once' | 'session' | 'command' | 'path';
  toolName?: string;
  reason?: string;
  parameters?: unknown;
}

type TranscriptRole = 'user' | 'assistant' | 'tool' | 'status' | 'error' | 'interaction' | 'diff' | 'approval';

interface TranscriptItem {
  role: TranscriptRole;
  text: string;
  detail?: string;
  requestId?: string;
  request?: unknown;
  path?: string;
  before?: string | null;
  after?: string;
  toolName?: string;
  reason?: string;
  parameters?: unknown;
}

export interface SidebarContext {
  workspace?: string;
  provider?: string;
  model?: string;
  mode?: string;
  permission?: string;
  daemonVersion?: string;
  extensionVersion?: string;
  authConfigured?: boolean;
  authMethod?: string;
  authSource?: string;
  status?: string;
  problem?: string;
  running?: boolean;
}

interface SidebarState {
  running: boolean;
  sessionId?: string;
  status: string;
  context: SidebarContext;
  transcript: TranscriptItem[];
  runOptions: RunOptions;
}

interface DaemonEventParams {
  session_id?: string;
  event?: unknown;
}

interface WebviewMessage {
  type?: string;
  task?: unknown;
  options?: unknown;
  requestId?: unknown;
  answer?: unknown;
  approved?: unknown;
  scope?: unknown;
  toolName?: unknown;
  reason?: unknown;
  parameters?: unknown;
}

export class PeridotSidebarProvider implements vscode.WebviewViewProvider {
  public static readonly viewType = 'peridot.chatView';

  private view: vscode.WebviewView | undefined;
  private state: SidebarState = {
    running: false,
    status: 'Idle',
    context: {},
    transcript: [],
    runOptions: {
      mode: 'execute',
      permission: 'auto',
    },
  };

  public constructor(
    private readonly extensionUri: vscode.Uri,
    private readonly handlers: SidebarHandlers,
  ) {}

  public resolveWebviewView(webviewView: vscode.WebviewView): void {
    this.view = webviewView;
    webviewView.webview.options = {
      enableScripts: true,
      localResourceRoots: [this.extensionUri],
    };
    webviewView.webview.html = this.html(webviewView.webview);
    webviewView.webview.onDidReceiveMessage((message: WebviewMessage) => {
      void this.receive(message);
    });
    this.publish();
  }

  public resetForTask(task: string, workspace: string): void {
    this.state = {
      running: true,
      status: 'Starting daemon',
      context: {
        ...this.state.context,
        workspace,
        status: 'Starting daemon',
        problem: undefined,
        running: true,
      },
      transcript: [
        { role: 'user', text: task },
        { role: 'status', text: 'Starting daemon', detail: workspace },
      ],
      runOptions: this.state.runOptions,
    };
    this.publish();
  }

  public setSession(sessionId: string): void {
    this.state.sessionId = sessionId;
    this.state.status = `Running ${sessionId}`;
    this.state.context = { ...this.state.context, status: 'Running', running: true };
    this.append({ role: 'status', text: 'Session started', detail: sessionId });
  }

  public appendNotification(params: DaemonEventParams): void {
    const sessionId = params.session_id ?? 'unknown-session';
    const event = params.event;
    const item = transcriptItemForEvent(sessionId, event);
    if (item) {
      this.append(item);
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

  private async receive(message: WebviewMessage): Promise<void> {
    if (message.type === 'run') {
      const task = typeof message.task === 'string' ? message.task.trim() : '';
      if (task.length > 0) {
        const options = normalizeRunOptions(message.options);
        this.state.runOptions = options;
        await this.handlers.runTask(task, options);
      }
      return;
    }
    if (message.type === 'askUserRespond') {
      const requestId = typeof message.requestId === 'string' ? message.requestId : '';
      const answer = normalizeAskUserAnswer(message.answer);
      if (requestId && answer) {
        await this.handlers.respondAskUser(requestId, answer);
        this.resolveInteraction(requestId, answerLabel(answer));
      }
      return;
    }
    if (message.type === 'approvalRespond') {
      await this.handlers.respondApproval(normalizeApprovalResponse(message));
      this.resolveApproval(Boolean(message.approved));
      return;
    }
    if (message.type === 'cancel') {
      await this.handlers.cancelTask();
      return;
    }
    if (message.type === 'loginOpenAi') {
      await this.handlers.loginOpenAi();
      return;
    }
    if (message.type === 'refreshStatus') {
      await this.handlers.refreshStatus();
    }
  }

  private append(item: TranscriptItem): void {
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
    this.view?.webview.postMessage({
      type: 'state',
      state: this.state,
    });
  }

  private html(webview: vscode.Webview): string {
    const nonce = nonceValue();
    return /* html */ `<!DOCTYPE html>
<html lang="en">
<head>
  <meta charset="UTF-8">
  <meta
    http-equiv="Content-Security-Policy"
    content="default-src 'none'; style-src 'unsafe-inline'; script-src 'nonce-${nonce}';"
  >
  <meta name="viewport" content="width=device-width, initial-scale=1.0">
  <title>Peridot</title>
  <style>
    :root {
      color-scheme: light dark;
      --peri-accent: #7fbf6a;
      --peri-border: var(--vscode-panel-border);
      --peri-muted: var(--vscode-descriptionForeground);
      --peri-bg-subtle: var(--vscode-sideBarSectionHeader-background);
      --peri-pill-bg: var(--vscode-badge-background);
      --peri-pill-fg: var(--vscode-badge-foreground);
    }
    * { box-sizing: border-box; }
    body {
      margin: 0;
      padding: 0;
      color: var(--vscode-foreground);
      background: var(--vscode-sideBar-background);
      font: var(--vscode-font-size) var(--vscode-font-family);
    }
    .app {
      min-height: 100vh;
      display: grid;
      grid-template-rows: auto auto 1fr auto;
    }
    .toolbar {
      display: flex;
      align-items: center;
      justify-content: space-between;
      gap: 8px;
      padding: 8px 10px;
      border-bottom: 1px solid var(--peri-border);
      background: var(--peri-bg-subtle);
    }
    .toolbar-main {
      min-width: 0;
      flex: 1 1 auto;
    }
    .title {
      min-width: 0;
      font-weight: 600;
      overflow: hidden;
      text-overflow: ellipsis;
      white-space: nowrap;
    }
    .status {
      min-width: 0;
      color: var(--peri-muted);
      font-size: 11px;
      overflow: hidden;
      text-overflow: ellipsis;
      white-space: nowrap;
    }
    .actions {
      display: flex;
      gap: 4px;
      flex: 0 0 auto;
    }
    .icon {
      width: 28px;
      height: 28px;
      flex: 0 0 auto;
      border: 1px solid var(--vscode-button-border, transparent);
      color: var(--vscode-button-foreground);
      background: var(--vscode-button-background);
      border-radius: 4px;
      cursor: pointer;
    }
    .icon.secondary {
      color: var(--vscode-button-secondaryForeground);
      background: var(--vscode-button-secondaryBackground);
    }
    .icon:disabled {
      opacity: 0.45;
      cursor: default;
    }
    .context {
      display: grid;
      align-content: start;
      gap: 7px;
      padding: 8px 10px;
      border-bottom: 1px solid var(--peri-border);
      background: var(--vscode-sideBar-background);
    }
    .context-row {
      display: flex;
      gap: 6px;
      align-items: center;
      min-width: 0;
    }
    .workspace {
      color: var(--peri-muted);
      overflow: hidden;
      text-overflow: ellipsis;
      white-space: nowrap;
    }
    .pill {
      max-width: 100%;
      border-radius: 4px;
      color: var(--peri-pill-fg);
      background: var(--peri-pill-bg);
      padding: 2px 5px;
      font-size: 11px;
      overflow: hidden;
      text-overflow: ellipsis;
      white-space: nowrap;
    }
    .pill.muted {
      color: var(--peri-muted);
      background: var(--vscode-editorWidget-background);
    }
    .pill.problem {
      color: var(--vscode-errorForeground);
      background: var(--vscode-inputValidation-errorBackground);
    }
    .transcript {
      min-height: 0;
      overflow-y: auto;
      padding: 8px 10px 12px;
    }
    .empty {
      color: var(--peri-muted);
      line-height: 1.45;
      padding: 10px 0;
    }
    .message {
      border-left: 2px solid var(--peri-border);
      margin: 0 0 10px;
      padding: 0 0 0 8px;
      line-height: 1.45;
      overflow-wrap: anywhere;
      white-space: pre-wrap;
    }
    .message.user { border-left-color: var(--vscode-textLink-foreground); }
    .message.assistant { border-left-color: var(--peri-accent); }
    .message.tool { border-left-color: var(--vscode-charts-blue); }
    .message.error { border-left-color: var(--vscode-errorForeground); }
    .message.status {
      color: var(--peri-muted);
    }
    .role {
      color: var(--peri-muted);
      font-size: 11px;
      margin-bottom: 2px;
      text-transform: uppercase;
    }
    .detail {
      color: var(--peri-muted);
      font-size: 11px;
      margin-top: 3px;
    }
    .message.interaction {
      border-left-color: var(--vscode-notificationsInfoIcon-foreground);
    }
    .message.diff {
      border-left-color: var(--vscode-gitDecoration-modifiedResourceForeground);
    }
    .message.approval {
      border-left-color: var(--vscode-notificationsWarningIcon-foreground);
    }
    .choice-row {
      display: grid;
      grid-template-columns: auto 1fr;
      gap: 6px;
      align-items: start;
      margin: 4px 0;
    }
    .inline-input {
      width: 100%;
      min-height: 28px;
      color: var(--vscode-input-foreground);
      background: var(--vscode-input-background);
      border: 1px solid var(--vscode-input-border, var(--peri-border));
      border-radius: 4px;
      padding: 5px 6px;
      font: inherit;
    }
    .inline-actions {
      display: flex;
      gap: 6px;
      margin-top: 7px;
    }
    .small-button {
      min-height: 26px;
      color: var(--vscode-button-foreground);
      background: var(--vscode-button-background);
      border: 1px solid var(--vscode-button-border, transparent);
      border-radius: 4px;
      cursor: pointer;
      font: inherit;
      padding: 3px 8px;
    }
    .small-button.secondary {
      color: var(--vscode-button-secondaryForeground);
      background: var(--vscode-button-secondaryBackground);
    }
    .diff-box {
      display: grid;
      gap: 3px;
      color: var(--peri-muted);
      font-size: 11px;
      margin-top: 4px;
    }
    .options {
      display: grid;
      grid-template-columns: 1fr 1fr;
      gap: 6px;
      grid-column: 1 / -1;
    }
    select,
    .model-input {
      min-width: 0;
      height: 28px;
      color: var(--vscode-dropdown-foreground);
      background: var(--vscode-dropdown-background);
      border: 1px solid var(--vscode-dropdown-border, var(--peri-border));
      border-radius: 4px;
      font: inherit;
      padding: 3px 5px;
    }
    .model-input {
      grid-column: 1 / -1;
      color: var(--vscode-input-foreground);
      background: var(--vscode-input-background);
      border-color: var(--vscode-input-border, var(--peri-border));
    }
    .composer {
      display: grid;
      grid-template-columns: 1fr auto;
      gap: 6px;
      padding: 8px 10px 10px;
      border-top: 1px solid var(--peri-border);
      background: var(--vscode-sideBar-background);
    }
    textarea {
      min-height: 34px;
      max-height: 120px;
      resize: vertical;
      color: var(--vscode-input-foreground);
      background: var(--vscode-input-background);
      border: 1px solid var(--vscode-input-border, var(--peri-border));
      border-radius: 4px;
      padding: 7px 8px;
      font: inherit;
    }
    textarea:focus {
      outline: 1px solid var(--vscode-focusBorder);
      outline-offset: -1px;
    }
    .run {
      min-width: 38px;
      height: 34px;
      align-self: end;
      color: var(--vscode-button-foreground);
      background: var(--vscode-button-background);
      border: 1px solid var(--vscode-button-border, transparent);
      border-radius: 4px;
      cursor: pointer;
    }
    .run:hover,
    .icon:hover {
      background: var(--vscode-button-hoverBackground);
    }
    .run:disabled {
      opacity: 0.45;
      cursor: default;
    }
  </style>
</head>
<body>
  <div class="app">
    <header class="toolbar">
      <div class="toolbar-main">
        <div class="title">Peridot</div>
        <div class="status" id="status">Idle</div>
      </div>
      <div class="actions">
        <button class="icon secondary" id="refresh" title="Refresh status">↻</button>
        <button class="icon secondary" id="login" title="Login with ChatGPT">↗</button>
        <button class="icon" id="cancel" title="Cancel current task" disabled>■</button>
      </div>
    </header>
    <section class="context" id="context"></section>
    <main class="transcript" id="transcript">
      <div class="empty">Ready.</div>
    </main>
    <form class="composer" id="composer">
      <div class="options">
        <select id="mode" title="Execution mode">
          <option value="execute">Execute</option>
          <option value="plan">Plan</option>
          <option value="goal">Goal</option>
        </select>
        <select id="permission" title="Permission mode">
          <option value="auto">Auto</option>
          <option value="safe">Safe</option>
          <option value="yolo">Yolo</option>
        </select>
        <input class="model-input" id="model" placeholder="model override (optional)" />
      </div>
      <textarea id="task" rows="2" placeholder="Ask Peridot to work in this repo"></textarea>
      <button class="run" id="run" title="Run task">▶</button>
    </form>
  </div>
  <script nonce="${nonce}">
    const vscode = acquireVsCodeApi();
    const statusEl = document.getElementById('status');
    const contextEl = document.getElementById('context');
    const transcriptEl = document.getElementById('transcript');
    const composer = document.getElementById('composer');
    const taskEl = document.getElementById('task');
    const modeEl = document.getElementById('mode');
    const permissionEl = document.getElementById('permission');
    const modelEl = document.getElementById('model');
    const runEl = document.getElementById('run');
    const cancelEl = document.getElementById('cancel');
    const loginEl = document.getElementById('login');
    const refreshEl = document.getElementById('refresh');

    composer.addEventListener('submit', (event) => {
      event.preventDefault();
      const task = taskEl.value.trim();
      if (!task) return;
      vscode.postMessage({ type: 'run', task, options: currentRunOptions() });
      taskEl.value = '';
    });

    taskEl.addEventListener('keydown', (event) => {
      if (event.key === 'Enter' && (event.metaKey || event.ctrlKey)) {
        composer.requestSubmit();
      }
    });

    cancelEl.addEventListener('click', () => {
      vscode.postMessage({ type: 'cancel' });
    });
    loginEl.addEventListener('click', () => {
      vscode.postMessage({ type: 'loginOpenAi' });
    });
    refreshEl.addEventListener('click', () => {
      vscode.postMessage({ type: 'refreshStatus' });
    });

    window.addEventListener('message', (event) => {
      if (event.data?.type === 'state') {
        render(event.data.state);
      }
    });

    function render(state) {
      statusEl.textContent = state.status || 'Idle';
      runEl.disabled = Boolean(state.running);
      cancelEl.disabled = !state.running;
      loginEl.disabled = Boolean(state.running);
      const options = state.runOptions || {};
      modeEl.value = options.mode || 'execute';
      permissionEl.value = options.permission || 'auto';
      modelEl.value = options.model || '';
      modeEl.disabled = Boolean(state.running);
      permissionEl.disabled = Boolean(state.running);
      modelEl.disabled = Boolean(state.running);
      renderContext(state.context || {});
      if (!state.transcript || state.transcript.length === 0) {
        transcriptEl.innerHTML = '<div class="empty">Ready.</div>';
        return;
      }
      transcriptEl.replaceChildren(...state.transcript.map(renderItem));
      transcriptEl.scrollTop = transcriptEl.scrollHeight;
    }

    function renderContext(context) {
      const rows = [];
      const workspace = context.workspace || 'No workspace';
      rows.push(row([span('workspace', workspace, workspace)]));

      const provider = context.provider || 'provider unknown';
      const model = context.model || 'model unknown';
      const authLabel = context.authConfigured
        ? 'auth ' + (context.authSource || 'configured')
        : 'auth missing';
      rows.push(row([
        span('pill', provider, provider),
        span('pill muted', model, model),
        span(context.authConfigured ? 'pill' : 'pill problem', authLabel, authLabel),
      ]));

      if (context.mode || context.permission || context.daemonVersion) {
        rows.push(row([
          span('pill muted', context.mode || 'mode', context.mode || ''),
          span('pill muted', context.permission || 'permission', context.permission || ''),
          span('pill muted', versionLabel(context), versionLabel(context)),
        ]));
      }

      if (context.problem) {
        rows.push(row([span('pill problem', context.problem, context.problem)]));
      }
      contextEl.replaceChildren(...rows);
    }

    function versionLabel(context) {
      if (!context.daemonVersion && !context.extensionVersion) return 'version';
      return 'daemon ' + (context.daemonVersion || '?') + ' · ext ' + (context.extensionVersion || '?');
    }

    function row(children) {
      const el = document.createElement('div');
      el.className = 'context-row';
      el.append(...children);
      return el;
    }

    function span(className, text, title) {
      const el = document.createElement('span');
      el.className = className;
      el.textContent = text || '';
      if (title) el.title = title;
      return el;
    }

    function renderItem(item) {
      const root = document.createElement('section');
      root.className = 'message ' + item.role;
      const role = document.createElement('div');
      role.className = 'role';
      role.textContent = item.role;
      const text = document.createElement('div');
      text.textContent = item.text || '';
      root.append(role, text);
      if (item.role === 'interaction') {
        root.append(renderAskUser(item));
        return root;
      }
      if (item.role === 'approval') {
        root.append(renderApproval(item));
        return root;
      }
      if (item.role === 'diff') {
        root.append(renderDiff(item));
      }
      if (item.detail) {
        const detail = document.createElement('div');
        detail.className = 'detail';
        detail.textContent = item.detail;
        root.append(detail);
      }
      return root;
    }

    function renderApproval(item) {
      const wrap = document.createElement('div');
      if (item.detail) {
        const detail = document.createElement('div');
        detail.className = 'detail';
        detail.textContent = item.detail;
        wrap.append(detail);
      }
      const scope = document.createElement('select');
      scope.title = 'Approval scope';
      [
        ['once', 'Once'],
        ['command', 'Command'],
        ['path', 'Path'],
        ['session', 'Session'],
      ].forEach(([value, label]) => {
        const option = document.createElement('option');
        option.value = value;
        option.textContent = label;
        scope.append(option);
      });
      const actions = document.createElement('div');
      actions.className = 'inline-actions';
      const approve = document.createElement('button');
      approve.type = 'button';
      approve.className = 'small-button';
      approve.textContent = 'Approve';
      approve.addEventListener('click', () => {
        vscode.postMessage({
          type: 'approvalRespond',
          approved: true,
          scope: scope.value,
          toolName: item.toolName,
          reason: item.reason,
          parameters: item.parameters,
        });
      });
      const deny = document.createElement('button');
      deny.type = 'button';
      deny.className = 'small-button secondary';
      deny.textContent = 'Deny';
      deny.addEventListener('click', () => {
        vscode.postMessage({
          type: 'approvalRespond',
          approved: false,
          scope: scope.value,
          toolName: item.toolName,
          reason: item.reason,
          parameters: item.parameters,
        });
      });
      actions.append(approve, deny);
      wrap.append(scope, actions);
      return wrap;
    }

    function renderAskUser(item) {
      const wrap = document.createElement('div');
      const request = item.request || {};
      const kind = request.kind || '';
      if (kind === 'single_select') {
        (request.options || []).forEach((option, index) => {
          const label = document.createElement('label');
          label.className = 'choice-row';
          const input = document.createElement('input');
          input.type = 'radio';
          input.name = item.requestId;
          input.value = String(index);
          input.checked = index === request.default_index;
          label.append(input, document.createTextNode(option));
          wrap.append(label);
        });
      } else if (kind === 'multi_select') {
        (request.options || []).forEach((option, index) => {
          const label = document.createElement('label');
          label.className = 'choice-row';
          const input = document.createElement('input');
          input.type = 'checkbox';
          input.value = String(index);
          label.append(input, document.createTextNode(option));
          wrap.append(label);
        });
      } else {
        const input = document.createElement('input');
        input.className = 'inline-input';
        input.value = request.default || '';
        input.placeholder = request.hint || '';
        input.dataset.freeform = 'true';
        wrap.append(input);
      }

      const actions = document.createElement('div');
      actions.className = 'inline-actions';
      const send = document.createElement('button');
      send.type = 'button';
      send.className = 'small-button';
      send.textContent = 'Send';
      send.addEventListener('click', () => {
        vscode.postMessage({
          type: 'askUserRespond',
          requestId: item.requestId,
          answer: answerForRequest(item, wrap),
        });
      });
      const cancel = document.createElement('button');
      cancel.type = 'button';
      cancel.className = 'small-button secondary';
      cancel.textContent = 'Cancel';
      cancel.addEventListener('click', () => {
        vscode.postMessage({
          type: 'askUserRespond',
          requestId: item.requestId,
          answer: { kind: 'cancelled' },
        });
      });
      actions.append(send, cancel);
      wrap.append(actions);
      return wrap;
    }

    function answerForRequest(item, wrap) {
      const request = item.request || {};
      if (request.kind === 'single_select') {
        const selected = wrap.querySelector('input[type="radio"]:checked');
        const index = selected ? Number(selected.value) : Number(request.default_index || 0);
        const options = request.options || [];
        return { kind: 'selected', index, text: String(options[index] || '') };
      }
      if (request.kind === 'multi_select') {
        const indices = Array.from(wrap.querySelectorAll('input[type="checkbox"]:checked'))
          .map((input) => Number(input.value))
          .filter((value) => Number.isFinite(value));
        return { kind: 'multi_selected', indices };
      }
      const input = wrap.querySelector('[data-freeform="true"]');
      return { kind: 'text', text: input ? input.value : '' };
    }

    function renderDiff(item) {
      const box = document.createElement('div');
      box.className = 'diff-box';
      const before = typeof item.before === 'string' ? item.before.split('\\n').length : 0;
      const after = typeof item.after === 'string' ? item.after.split('\\n').length : 0;
      box.textContent = 'before ' + before + ' lines · after ' + after + ' lines';
      return box;
    }

    function currentRunOptions() {
      const model = modelEl.value.trim();
      return {
        mode: modeEl.value,
        permission: permissionEl.value,
        model: model || undefined,
      };
    }
  </script>
</body>
</html>`;
  }
}

function transcriptItemForEvent(sessionId: string, event: unknown): TranscriptItem | undefined {
  if (!isRecord(event)) {
    return { role: 'status', text: `Event from ${sessionId}` };
  }
  const kind = typeof event.kind === 'string' ? event.kind : 'unknown';
  switch (kind) {
    case 'started':
      return { role: 'status', text: 'Daemon started', detail: sessionId };
    case 'run_started':
      return { role: 'status', text: 'Run started', detail: sessionId };
    case 'agents_md_loaded':
      return {
        role: 'status',
        text: 'AGENTS.md loaded',
        detail: compactAgentsDetail(event),
      };
    case 'turn_started':
      return {
        role: 'status',
        text: `Turn ${numberField(event, 'turn_index') + 1} started`,
      };
    case 'assistant_started':
      return { role: 'status', text: 'Assistant started' };
    case 'assistant_delta':
      return { role: 'assistant', text: stringField(event, 'delta') };
    case 'thinking':
      return { role: 'assistant', text: stringField(event, 'text') };
    case 'assistant_finished':
    case 'context_utilization_changed':
      return undefined;
    case 'tool_started':
      if (stringField(event, 'name') === 'agent_ask_user') {
        return {
          role: 'tool',
          text: 'Started agent_ask_user',
          detail: compactAskUserToolDetail(event.parameters),
        };
      }
      return {
        role: 'tool',
        text: `Started ${stringField(event, 'name')}`,
        detail: json(event.parameters),
      };
    case 'tool_finished':
      return {
        role: 'tool',
        text: `Finished ${stringField(event, 'name')}`,
        detail: summarizeToolResult(event),
      };
    case 'ask_user_requested':
      return {
        role: 'interaction',
        text: questionForAskUser(event.request),
        detail: stringField(event, 'request_id'),
        requestId: stringField(event, 'request_id'),
        request: event.request,
      };
    case 'approval_requested':
      return {
        role: 'approval',
        text: `Approval requested: ${stringField(event, 'tool_name')}`,
        detail: [stringField(event, 'reason'), json(event.parameters)].filter(Boolean).join('\n'),
        toolName: stringField(event, 'tool_name'),
        reason: stringField(event, 'reason'),
        parameters: event.parameters,
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
    case 'file_diff': {
      const payload = isRecord(event) ? event : {};
      const path = stringField(payload, 'path');
      return {
        role: 'diff',
        text: `Changed ${path || 'file'}`,
        detail: stringField(payload, 'tool_name'),
        path,
        before: typeof payload.before === 'string' ? payload.before : null,
        after: typeof payload.after === 'string' ? payload.after : '',
      };
    }
    case 'usage_updated':
      return {
        role: 'status',
        text: 'Usage updated',
        detail: compactUsageDetail(event.usage),
      };
    case 'turn_ended':
      return {
        role: 'status',
        text: booleanField(event, 'success') ? 'Turn completed' : 'Turn failed',
      };
    case 'finished':
      return { role: 'status', text: 'Finished', detail: compactFinishedDetail(event) };
    case 'error':
      return { role: 'error', text: stringField(event, 'message') };
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
  return json(event);
}

function compactAgentsDetail(event: Record<string, unknown>): string {
  const ruleCount = event.rule_count;
  const paths = Array.isArray(event.paths)
    ? event.paths.filter((value): value is string => typeof value === 'string')
    : [];
  const pathLabel = paths.length > 0 ? paths.join(', ') : 'no paths';
  return typeof ruleCount === 'number' ? `${ruleCount} rules · ${pathLabel}` : pathLabel;
}

function compactUsageDetail(value: unknown): string {
  if (!isRecord(value)) {
    return '';
  }
  const input = numberField(value, 'input_tokens');
  const output = numberField(value, 'output_tokens');
  const cost = value.estimated_cost_usd;
  const costLabel = typeof cost === 'number' ? ` · $${cost.toFixed(4)}` : '';
  return `${input} in · ${output} out${costLabel}`;
}

function compactFinishedDetail(event: Record<string, unknown>): string {
  const duration = numberField(event, 'duration_ms');
  const turns = numberField(event, 'turns');
  const reason = stringField(event, 'stopped_reason');
  const seconds = duration > 0 ? `${(duration / 1000).toFixed(1)}s` : '';
  return [reason, turns > 0 ? `${turns} turns` : '', seconds].filter(Boolean).join(' · ');
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
  return isRecord(event) && (event.kind === 'approval_requested' || event.kind === 'approval_waiting');
}

function stringField(record: Record<string, unknown>, key: string): string {
  const value = record[key];
  return typeof value === 'string' ? value : json(value);
}

function numberField(record: Record<string, unknown>, key: string): number {
  const value = record[key];
  return typeof value === 'number' ? value : 0;
}

function booleanField(record: Record<string, unknown>, key: string): boolean {
  return record[key] === true;
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

function normalizeRunOptions(value: unknown): RunOptions {
  const record = isRecord(value) ? value : {};
  const mode = record.mode === 'plan' || record.mode === 'goal' ? record.mode : 'execute';
  const permission =
    record.permission === 'safe' || record.permission === 'yolo' ? record.permission : 'auto';
  const model = typeof record.model === 'string' ? record.model.trim() : '';
  return {
    mode,
    permission,
    ...(model ? { model } : {}),
  };
}

function normalizeAskUserAnswer(value: unknown): AskUserAnswer | undefined {
  if (!isRecord(value) || typeof value.kind !== 'string') {
    return undefined;
  }
  if (value.kind === 'cancelled') {
    return { kind: 'cancelled' };
  }
  if (value.kind === 'selected') {
    const index = typeof value.index === 'number' ? value.index : 0;
    const text = typeof value.text === 'string' ? value.text : '';
    return { kind: 'selected', index, text };
  }
  if (value.kind === 'multi_selected') {
    const indices = Array.isArray(value.indices)
      ? value.indices.filter((index): index is number => typeof index === 'number')
      : [];
    return { kind: 'multi_selected', indices };
  }
  if (value.kind === 'text') {
    const text = typeof value.text === 'string' ? value.text : '';
    return { kind: 'text', text };
  }
  return undefined;
}

function normalizeApprovalResponse(value: WebviewMessage): ApprovalResponse {
  const scope =
    value.scope === 'session' || value.scope === 'command' || value.scope === 'path'
      ? value.scope
      : 'once';
  return {
    approved: value.approved === true,
    scope,
    toolName: typeof value.toolName === 'string' ? value.toolName : undefined,
    reason: typeof value.reason === 'string' ? value.reason : undefined,
    parameters: value.parameters,
  };
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
