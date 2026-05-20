import * as vscode from 'vscode';
import {
  ApprovalResponse,
  AskUserAnswer,
  BudgetSlice,
  CommitteeRoleSlice,
  ContextSlice,
  HudState,
  InboundMessage,
  OutboundMessage,
  PlanSlice,
  PlanStepView,
  RunOptions,
  SidebarContext,
  SidebarState,
  TranscriptItem,
  UsageSlice,
} from './types';

export type { ApprovalResponse, ApprovalScope, AskUserAnswer, RunOptions } from './types';

export interface SidebarHandlers {
  runTask: (task: string, options: RunOptions) => Promise<void>;
  cancelTask: () => Promise<void>;
  loginOpenAi: () => Promise<void>;
  refreshStatus: () => Promise<void>;
  respondAskUser: (requestId: string, answer: AskUserAnswer) => Promise<void>;
  respondApproval: (decision: ApprovalResponse) => Promise<void>;
  openFile: (relativePath: string) => Promise<void>;
}

interface DaemonEventParams {
  session_id?: string;
  event?: unknown;
}

const SUPPRESSED_KINDS = new Set<string>([
  'agents_md_loaded',
  'turn_started',
  'turn_ended',
  'assistant_started',
  'assistant_finished',
  'context_utilization_changed',
  'usage_updated',
  'budget_updated',
  'committee_role_usage',
]);

export class PeridotSidebarProvider implements vscode.WebviewViewProvider {
  public static readonly viewType = 'peridot.chatView';

  private view: vscode.WebviewView | undefined;
  private state: SidebarState = freshState();

  public constructor(
    private readonly extensionUri: vscode.Uri,
    private readonly handlers: SidebarHandlers,
  ) {}

  public resolveWebviewView(webviewView: vscode.WebviewView): void {
    this.view = webviewView;
    webviewView.webview.options = {
      enableScripts: true,
      localResourceRoots: [vscode.Uri.joinPath(this.extensionUri, 'dist')],
    };
    webviewView.webview.html = this.html(webviewView.webview);
    webviewView.webview.onDidReceiveMessage((message: OutboundMessage) => {
      void this.receive(message);
    });
    this.publish();
  }

  public resetForTask(task: string, workspace: string): void {
    this.state = {
      ...freshState(),
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
    const event = params.event;
    if (!isRecord(event)) {
      this.append({ role: 'status', text: 'Event' });
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

  private appendToolStarted(event: Record<string, unknown>): void {
    const name = stringField(event, 'name');
    if (name === 'agent_ask_user') {
      this.append({
        role: 'tool',
        text: 'Started agent_ask_user',
        detail: compactAskUserToolDetail(event.parameters),
        toolName: name,
        pending: true,
      });
      return;
    }
    this.append({
      role: 'tool',
      text: `Started ${name}`,
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
        item.text = `Finished ${name}`;
        item.toolResultSummary = summary;
        item.detail = undefined;
        this.publish();
        return;
      }
    }
    this.append({
      role: 'tool',
      text: `Finished ${name}`,
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
      text: `Approval requested: ${toolName}`,
      detail: [reason, json(parameters)].filter(Boolean).join('\n'),
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
    if (message.type === 'run') {
      const task = message.task.trim();
      if (task.length === 0) return;
      this.state.runOptions = message.options;
      await this.handlers.runTask(task, message.options);
      return;
    }
    if (message.type === 'askUserRespond') {
      if (message.requestId) {
        await this.handlers.respondAskUser(message.requestId, message.answer);
        this.resolveInteraction(message.requestId, answerLabel(message.answer));
      }
      return;
    }
    if (message.type === 'approvalRespond') {
      await this.handlers.respondApproval({
        approved: message.approved,
        scope: message.scope,
        toolName: message.toolName,
        reason: message.reason,
        parameters: message.parameters,
      });
      this.resolveApproval(message.approved);
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
      return;
    }
    if (message.type === 'openFile') {
      await this.handlers.openFile(message.path);
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
    const message: InboundMessage = { type: 'state', state: this.state };
    this.view?.webview.postMessage(message);
  }

  private html(webview: vscode.Webview): string {
    const nonce = nonceValue();
    const scriptUri = webview.asWebviewUri(
      vscode.Uri.joinPath(this.extensionUri, 'dist', 'webview.js'),
    );
    const styleUri = webview.asWebviewUri(
      vscode.Uri.joinPath(this.extensionUri, 'dist', 'webview.css'),
    );
    return /* html */ `<!DOCTYPE html>
<html lang="en">
<head>
  <meta charset="UTF-8">
  <meta
    http-equiv="Content-Security-Policy"
    content="default-src 'none'; style-src ${webview.cspSource} 'unsafe-inline'; script-src 'nonce-${nonce}';"
  >
  <meta name="viewport" content="width=device-width, initial-scale=1.0">
  <title>Peridot</title>
  <link href="${styleUri}" rel="stylesheet" />
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
    <section class="hud" id="hud"></section>
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
  <script nonce="${nonce}" src="${scriptUri}"></script>
</body>
</html>`;
  }
}

function freshState(): SidebarState {
  return {
    running: false,
    status: 'Idle',
    context: {},
    transcript: [],
    runOptions: {
      mode: 'execute',
      permission: 'auto',
    },
    hud: {} as HudState,
  };
}

function shouldSuppress(item: TranscriptItem): boolean {
  if (item.role !== 'status') return false;
  const lowered = item.text.toLowerCase();
  for (const kind of SUPPRESSED_KINDS) {
    if (lowered.includes(kind.replace(/_/g, ' '))) return true;
  }
  return false;
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
        text: `Changed ${path || 'file'}`,
        detail: stringField(event, 'tool_name'),
        path,
        before: typeof event.before === 'string' ? event.before : null,
        after: typeof event.after === 'string' ? event.after : '',
      };
    }
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
  return json(event.result ?? event);
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

