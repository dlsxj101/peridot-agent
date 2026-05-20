import * as vscode from 'vscode';

export interface SidebarHandlers {
  runTask: (task: string) => Promise<void>;
  cancelTask: () => Promise<void>;
  loginOpenAi: () => Promise<void>;
  refreshStatus: () => Promise<void>;
}

type TranscriptRole = 'user' | 'assistant' | 'tool' | 'status' | 'error';

interface TranscriptItem {
  role: TranscriptRole;
  text: string;
  detail?: string;
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
}

interface DaemonEventParams {
  session_id?: string;
  event?: unknown;
}

interface WebviewMessage {
  type?: string;
  task?: unknown;
}

export class PeridotSidebarProvider implements vscode.WebviewViewProvider {
  public static readonly viewType = 'peridot.chatView';

  private view: vscode.WebviewView | undefined;
  private state: SidebarState = {
    running: false,
    status: 'Idle',
    context: {},
    transcript: [],
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
        await this.handlers.runTask(task);
      }
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
      grid-template-rows: auto 1fr auto;
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
    const runEl = document.getElementById('run');
    const cancelEl = document.getElementById('cancel');
    const loginEl = document.getElementById('login');
    const refreshEl = document.getElementById('refresh');

    composer.addEventListener('submit', (event) => {
      event.preventDefault();
      const task = taskEl.value.trim();
      if (!task) return;
      vscode.postMessage({ type: 'run', task });
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
      if (item.detail) {
        const detail = document.createElement('div');
        detail.className = 'detail';
        detail.textContent = item.detail;
        root.append(detail);
      }
      return root;
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
  return isRecord(event) && (event.kind === 'finished' || event.kind === 'error');
}

function isErrorEvent(event: unknown): boolean {
  return isRecord(event) && event.kind === 'error';
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

function nonceValue(): string {
  const alphabet = 'ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789';
  let value = '';
  for (let i = 0; i < 32; i++) {
    value += alphabet[Math.floor(Math.random() * alphabet.length)];
  }
  return value;
}
