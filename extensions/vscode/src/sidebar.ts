import * as vscode from 'vscode';

export interface SidebarHandlers {
  runTask: (task: string) => Promise<void>;
  cancelTask: () => Promise<void>;
}

type TranscriptRole = 'user' | 'assistant' | 'tool' | 'status' | 'error';

interface TranscriptItem {
  role: TranscriptRole;
  text: string;
  detail?: string;
}

interface SidebarState {
  running: boolean;
  sessionId?: string;
  status: string;
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
    this.append({ role: 'status', text: 'Session started', detail: sessionId });
  }

  public appendNotification(params: DaemonEventParams): void {
    const sessionId = params.session_id ?? 'unknown-session';
    const event = params.event;
    const item = transcriptItemForEvent(sessionId, event);
    this.append(item);
    if (isTerminalEvent(event)) {
      this.state.running = false;
      this.state.status = item.role === 'error' ? 'Failed' : 'Finished';
      this.publish();
    }
  }

  public appendSystem(text: string, detail?: string): void {
    this.append({ role: 'status', text, detail });
  }

  public appendError(text: string): void {
    this.state.running = false;
    this.state.status = 'Failed';
    this.append({ role: 'error', text });
  }

  public markIdle(status = 'Idle'): void {
    this.state.running = false;
    this.state.status = status;
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
    .cancel {
      width: 28px;
      height: 28px;
      flex: 0 0 auto;
      border: 1px solid var(--vscode-button-border, transparent);
      color: var(--vscode-button-foreground);
      background: var(--vscode-button-background);
      border-radius: 4px;
      cursor: pointer;
    }
    .cancel:disabled {
      opacity: 0.45;
      cursor: default;
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
    .cancel:hover {
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
      <div>
        <div class="title">Peridot</div>
        <div class="status" id="status">Idle</div>
      </div>
      <button class="cancel" id="cancel" title="Cancel current task" disabled>■</button>
    </header>
    <main class="transcript" id="transcript">
      <div class="empty">Open a workspace and send a task.</div>
    </main>
    <form class="composer" id="composer">
      <textarea id="task" rows="2" placeholder="Ask Peridot to work in this repo"></textarea>
      <button class="run" id="run" title="Run task">▶</button>
    </form>
  </div>
  <script nonce="${nonce}">
    const vscode = acquireVsCodeApi();
    const statusEl = document.getElementById('status');
    const transcriptEl = document.getElementById('transcript');
    const composer = document.getElementById('composer');
    const taskEl = document.getElementById('task');
    const runEl = document.getElementById('run');
    const cancelEl = document.getElementById('cancel');

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

    window.addEventListener('message', (event) => {
      if (event.data?.type === 'state') {
        render(event.data.state);
      }
    });

    function render(state) {
      statusEl.textContent = state.status || 'Idle';
      runEl.disabled = Boolean(state.running);
      cancelEl.disabled = !state.running;
      if (!state.transcript || state.transcript.length === 0) {
        transcriptEl.innerHTML = '<div class="empty">Open a workspace and send a task.</div>';
        return;
      }
      transcriptEl.replaceChildren(...state.transcript.map(renderItem));
      transcriptEl.scrollTop = transcriptEl.scrollHeight;
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

function transcriptItemForEvent(sessionId: string, event: unknown): TranscriptItem {
  if (!isRecord(event)) {
    return { role: 'status', text: `Event from ${sessionId}`, detail: json(event) };
  }
  const kind = typeof event.kind === 'string' ? event.kind : 'unknown';
  switch (kind) {
    case 'started':
    case 'run_started':
      return { role: 'status', text: stringField(event, 'task'), detail: sessionId };
    case 'assistant_delta':
      return { role: 'assistant', text: stringField(event, 'delta') };
    case 'thinking':
      return { role: 'assistant', text: stringField(event, 'text') };
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
    case 'finished':
      return { role: 'status', text: 'Finished', detail: json(event) };
    case 'error':
      return { role: 'error', text: stringField(event, 'message') };
    case 'interrupted':
      return { role: 'status', text: 'Interrupted', detail: stringField(event, 'stage') };
    default:
      return { role: 'status', text: kind, detail: json(event) };
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

function nonceValue(): string {
  const alphabet = 'ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789';
  let value = '';
  for (let i = 0; i < 32; i++) {
    value += alphabet[Math.floor(Math.random() * alphabet.length)];
  }
  return value;
}
