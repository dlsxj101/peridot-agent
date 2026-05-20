import './style.css';
import type {
  InboundMessage,
  OutboundMessage,
  PlanSlice,
  RunOptions,
  SidebarContext,
  SidebarState,
  TranscriptItem,
} from '../src/types';
import { diffStats, renderUnifiedDiff } from './diff';
import { el, formatTokens, formatUsd, isRecord, json } from './util';

declare function acquireVsCodeApi(): {
  postMessage(msg: OutboundMessage): void;
  setState(state: unknown): void;
  getState(): unknown;
};

const vscode = acquireVsCodeApi();

const statusEl = document.getElementById('status') as HTMLElement;
const contextEl = document.getElementById('context') as HTMLElement;
const hudEl = document.getElementById('hud') as HTMLElement;
const transcriptEl = document.getElementById('transcript') as HTMLElement;
const composer = document.getElementById('composer') as HTMLFormElement;
const taskEl = document.getElementById('task') as HTMLTextAreaElement;
const modeEl = document.getElementById('mode') as HTMLSelectElement;
const permissionEl = document.getElementById('permission') as HTMLSelectElement;
const modelEl = document.getElementById('model') as HTMLInputElement;
const runEl = document.getElementById('run') as HTMLButtonElement;
const cancelEl = document.getElementById('cancel') as HTMLButtonElement;
const loginEl = document.getElementById('login') as HTMLButtonElement;
const refreshEl = document.getElementById('refresh') as HTMLButtonElement;

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

window.addEventListener('message', (event: MessageEvent<InboundMessage>) => {
  if (event.data?.type === 'state') {
    render(event.data.state);
  }
});

function render(state: SidebarState): void {
  statusEl.textContent = state.status || 'Idle';
  runEl.disabled = Boolean(state.running);
  cancelEl.disabled = !state.running;
  loginEl.disabled = Boolean(state.running);
  const options = state.runOptions || ({ mode: 'execute', permission: 'auto' } satisfies RunOptions);
  modeEl.value = options.mode || 'execute';
  permissionEl.value = options.permission || 'auto';
  modelEl.value = options.model || '';
  modeEl.disabled = Boolean(state.running);
  permissionEl.disabled = Boolean(state.running);
  modelEl.disabled = Boolean(state.running);
  renderContext(state.context || {});
  renderHud(state);
  renderTranscript(state.transcript || [], state.context || {});
}

function renderContext(context: SidebarContext): void {
  const rows: HTMLElement[] = [];
  const workspace = context.workspace || 'No workspace';
  rows.push(row([span('workspace', workspace, workspace)]));

  const provider = context.provider || 'provider unknown';
  const model = context.model || 'model unknown';
  const authLabel = context.authConfigured
    ? 'auth ' + (context.authSource || 'configured')
    : 'auth missing';
  rows.push(
    row([
      span('pill', provider, provider),
      span('pill muted', model, model),
      span(context.authConfigured ? 'pill' : 'pill problem', authLabel, authLabel),
    ]),
  );

  if (context.mode || context.permission || context.daemonVersion) {
    rows.push(
      row([
        span('pill muted', context.mode || 'mode', context.mode || ''),
        span('pill muted', context.permission || 'permission', context.permission || ''),
        span('pill muted', versionLabel(context), versionLabel(context)),
      ]),
    );
  }

  if (context.problem) {
    rows.push(row([span('pill problem', context.problem, context.problem)]));
  }
  contextEl.replaceChildren(...rows);
}

function renderHud(state: SidebarState): void {
  const hud = state.hud || {};
  const children: HTMLElement[] = [];

  if (hud.usage) {
    const tokens = hud.usage.inputTokens + hud.usage.outputTokens;
    const right = el(
      'span',
      'hud-value',
      `${formatTokens(hud.usage.inputTokens)} in · ${formatTokens(hud.usage.outputTokens)} out · ${formatUsd(hud.usage.costUsd)}`,
    );
    children.push(hudRow('Usage', `${formatTokens(tokens)} total`, right));
  }

  if (hud.context && hud.context.threshold > 0) {
    const pct = Math.min(1, hud.context.tokensUsed / hud.context.threshold);
    children.push(hudBarRow('Context', `${Math.round(pct * 100)}%`, pct));
  }

  if (hud.budget) {
    const cost =
      typeof hud.budget.costLimit === 'number'
        ? `${formatUsd(hud.budget.costUsed)} / ${formatUsd(hud.budget.costLimit)}`
        : formatUsd(hud.budget.costUsed);
    const turns =
      typeof hud.budget.turnsLimit === 'number'
        ? `${hud.budget.turnsUsed}/${hud.budget.turnsLimit} turns`
        : `${hud.budget.turnsUsed} turns`;
    children.push(hudRow('Budget', cost, el('span', 'hud-value', turns)));
  }

  if (hud.committee) {
    for (const [role, slice] of Object.entries(hud.committee)) {
      children.push(
        hudRow(
          role.charAt(0).toUpperCase() + role.slice(1),
          formatUsd(slice.costUsd),
          el('span', 'hud-value', `${formatTokens(slice.tokens)} tok`),
        ),
      );
    }
  }

  if (hud.plan && hud.plan.steps.length > 0) {
    children.push(renderPlan(hud.plan));
  }

  hudEl.replaceChildren(...children);
  hudEl.classList.toggle('hud-empty', children.length === 0);
}

function renderPlan(plan: PlanSlice): HTMLElement {
  const details = el('details', 'plan');
  details.open = true;
  const summary = el(
    'summary',
    undefined,
    `Plan · ${plan.steps.length} step${plan.steps.length === 1 ? '' : 's'}` +
      (typeof plan.current === 'number' ? ` · step ${plan.current + 1}` : ''),
  );
  details.append(summary);
  const ol = el('ol');
  plan.steps.forEach((step, index) => {
    const li = el('li', undefined, step.text);
    if (step.status === 'done') li.classList.add('done');
    if (plan.current === index) li.classList.add('current');
    ol.append(li);
  });
  details.append(ol);
  return details;
}

function hudRow(label: string, value: string, right?: HTMLElement): HTMLElement {
  const wrap = el('div', 'hud-row');
  wrap.append(el('span', 'hud-label', label));
  wrap.append(el('span', 'hud-value', value));
  wrap.append(right ?? el('span', 'hud-value', ''));
  return wrap;
}

function hudBarRow(label: string, value: string, pct: number): HTMLElement {
  const wrap = el('div', 'hud-row');
  wrap.append(el('span', 'hud-label', label));
  const barWrap = el('div', 'hud-bar-wrap');
  const bar = el('div', 'hud-bar');
  bar.style.width = `${Math.round(pct * 100)}%`;
  if (pct >= 0.9) bar.classList.add('critical');
  else if (pct >= 0.75) bar.classList.add('warn');
  barWrap.append(bar);
  wrap.append(barWrap, el('span', 'hud-value', value));
  return wrap;
}

function renderTranscript(items: TranscriptItem[], context: SidebarContext): void {
  if (!items || items.length === 0) {
    transcriptEl.replaceChildren(renderEmpty(context));
    return;
  }
  transcriptEl.replaceChildren(...items.map(renderItem));
  transcriptEl.scrollTop = transcriptEl.scrollHeight;
}

function renderEmpty(context: SidebarContext): HTMLElement {
  const wrap = el('div', 'empty empty-state');
  if (!context.workspace || context.workspace === 'No workspace') {
    wrap.append(el('h4', undefined, 'No workspace open'));
    wrap.append(
      el('div', undefined, 'Open a folder to let Peridot run tasks against your project.'),
    );
    return wrap;
  }
  if (!context.authConfigured) {
    wrap.append(el('h4', undefined, 'Sign in to your provider'));
    wrap.append(
      el(
        'div',
        undefined,
        'Use the ↗ button above to sign in with ChatGPT, or set ANTHROPIC_API_KEY / OPENAI_API_KEY before running a task.',
      ),
    );
    return wrap;
  }
  wrap.append(el('h4', undefined, 'Ready'));
  const ul = el('ul');
  ul.append(el('li', undefined, 'Describe a task below — Ctrl/Cmd+Enter submits.'));
  ul.append(el('li', undefined, 'Pick a mode: Execute, Plan, or Goal.'));
  ul.append(
    el(
      'li',
      undefined,
      'Permission: Auto runs verified tools, Safe asks first, Yolo runs anything.',
    ),
  );
  wrap.append(ul);
  return wrap;
}

function renderItem(item: TranscriptItem): HTMLElement {
  const root = el('section', `message ${item.role}`);
  const role = el('div', 'role', roleLabel(item));
  const text = el('div', undefined, item.text || '');
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
    return root;
  }
  if (item.role === 'tool') {
    const card = renderToolCard(item);
    if (card) root.append(card);
  }
  if (item.detail) {
    root.append(el('div', 'detail', item.detail));
  }
  return root;
}

function roleLabel(item: TranscriptItem): string {
  if (item.role === 'tool' && item.pending) return 'tool · running';
  return item.role;
}

function renderToolCard(item: TranscriptItem): HTMLElement | undefined {
  if (item.toolParameters === undefined && !item.toolResultSummary) return undefined;
  const wrap = el('div', 'tool-card');
  if (item.toolName) {
    wrap.append(el('div', 'tool-meta', item.toolName));
  }
  if (item.toolResultSummary) {
    const pre = el('pre');
    pre.textContent = item.toolResultSummary;
    wrap.append(pre);
  } else if (item.toolParameters !== undefined) {
    const pre = el('pre');
    pre.textContent = json(item.toolParameters);
    wrap.append(pre);
  }
  return wrap;
}

function renderApproval(item: TranscriptItem): HTMLElement {
  const wrap = el('div');
  if (item.detail) {
    wrap.append(el('div', 'detail', item.detail));
  }

  // Diff preview when the parameters look like a file mutation. The
  // before/after content is shipped by the host together with the
  // approval transcript item.
  if (typeof item.before === 'string' || typeof item.after === 'string') {
    wrap.append(renderUnifiedDiff(item.before ?? '', item.after ?? '', item.path));
    if (item.path) {
      const open = el('button', 'diff-open', `Open ${item.path}`);
      open.type = 'button';
      open.addEventListener('click', () => {
        if (item.path) vscode.postMessage({ type: 'openFile', path: item.path });
      });
      wrap.append(open);
    }
  }

  const scope = document.createElement('select');
  scope.title = 'Approval scope';
  for (const [value, label] of [
    ['once', 'Once'],
    ['command', 'Command'],
    ['path', 'Path'],
    ['session', 'Session'],
  ] as const) {
    const option = document.createElement('option');
    option.value = value;
    option.textContent = label;
    scope.append(option);
  }

  const actions = el('div', 'inline-actions');
  const approve = el('button', 'small-button', 'Approve');
  approve.type = 'button';
  approve.addEventListener('click', () => {
    vscode.postMessage({
      type: 'approvalRespond',
      approved: true,
      scope: scope.value as 'once' | 'session' | 'command' | 'path',
      toolName: item.toolName,
      reason: item.reason,
      parameters: item.parameters,
    });
  });
  const deny = el('button', 'small-button secondary', 'Deny');
  deny.type = 'button';
  deny.addEventListener('click', () => {
    vscode.postMessage({
      type: 'approvalRespond',
      approved: false,
      scope: scope.value as 'once' | 'session' | 'command' | 'path',
      toolName: item.toolName,
      reason: item.reason,
      parameters: item.parameters,
    });
  });
  actions.append(approve, deny);
  wrap.append(scope, actions);
  return wrap;
}

function renderAskUser(item: TranscriptItem): HTMLElement {
  const wrap = el('div');
  const request = isRecord(item.request) ? item.request : {};
  const kind = typeof request.kind === 'string' ? request.kind : '';
  const options = Array.isArray(request.options)
    ? request.options.filter((value): value is string => typeof value === 'string')
    : [];

  if (kind === 'single_select') {
    options.forEach((option, index) => {
      const label = el('label', 'choice-row');
      const input = document.createElement('input');
      input.type = 'radio';
      input.name = item.requestId ?? 'ask-user';
      input.value = String(index);
      input.checked = index === (request.default_index as number | undefined);
      label.append(input, document.createTextNode(option));
      wrap.append(label);
    });
  } else if (kind === 'multi_select') {
    options.forEach((option, index) => {
      const label = el('label', 'choice-row');
      const input = document.createElement('input');
      input.type = 'checkbox';
      input.value = String(index);
      label.append(input, document.createTextNode(option));
      wrap.append(label);
    });
  } else {
    const input = el('input', 'inline-input') as HTMLInputElement;
    input.value = typeof request.default === 'string' ? request.default : '';
    input.placeholder = typeof request.hint === 'string' ? request.hint : '';
    input.dataset.freeform = 'true';
    wrap.append(input);
    setTimeout(() => input.focus(), 0);
    input.addEventListener('keydown', (event) => {
      if (event.key === 'Enter' && !event.shiftKey) {
        event.preventDefault();
        sendAnswer();
      }
      if (event.key === 'Escape') {
        event.preventDefault();
        if (item.requestId) {
          vscode.postMessage({
            type: 'askUserRespond',
            requestId: item.requestId,
            answer: { kind: 'cancelled' },
          });
        }
      }
    });
  }

  const actions = el('div', 'inline-actions');
  const send = el('button', 'small-button', 'Send');
  send.type = 'button';
  send.addEventListener('click', sendAnswer);
  const cancel = el('button', 'small-button secondary', 'Cancel');
  cancel.type = 'button';
  cancel.addEventListener('click', () => {
    if (!item.requestId) return;
    vscode.postMessage({
      type: 'askUserRespond',
      requestId: item.requestId,
      answer: { kind: 'cancelled' },
    });
  });
  actions.append(send, cancel);
  wrap.append(actions);
  return wrap;

  function sendAnswer(): void {
    if (!item.requestId) return;
    vscode.postMessage({
      type: 'askUserRespond',
      requestId: item.requestId,
      answer: answerForRequest(item, wrap),
    });
  }
}

function answerForRequest(item: TranscriptItem, wrap: HTMLElement):
  | { kind: 'selected'; index: number; text: string }
  | { kind: 'multi_selected'; indices: number[] }
  | { kind: 'text'; text: string } {
  const request = isRecord(item.request) ? item.request : {};
  const options = Array.isArray(request.options)
    ? request.options.filter((value): value is string => typeof value === 'string')
    : [];
  if (request.kind === 'single_select') {
    const selected = wrap.querySelector<HTMLInputElement>('input[type="radio"]:checked');
    const index = selected ? Number(selected.value) : Number(request.default_index ?? 0);
    return { kind: 'selected', index, text: String(options[index] ?? '') };
  }
  if (request.kind === 'multi_select') {
    const indices = Array.from(wrap.querySelectorAll<HTMLInputElement>('input[type="checkbox"]:checked'))
      .map((input) => Number(input.value))
      .filter((value) => Number.isFinite(value));
    return { kind: 'multi_selected', indices };
  }
  const input = wrap.querySelector<HTMLInputElement>('[data-freeform="true"]');
  return { kind: 'text', text: input ? input.value : '' };
}

function renderDiff(item: TranscriptItem): HTMLElement {
  const wrap = el('div');
  const stats = diffStats(item.before, item.after);
  const summary = el('div', 'diff-summary');
  if (item.path) {
    const open = el('button', 'diff-open', item.path);
    open.type = 'button';
    open.addEventListener('click', () => {
      if (item.path) vscode.postMessage({ type: 'openFile', path: item.path });
    });
    summary.append(open);
  }
  summary.append(el('span', 'diff-add', `+${stats.added}`));
  summary.append(el('span', 'diff-del', `-${stats.removed}`));
  wrap.append(summary);
  wrap.append(renderUnifiedDiff(item.before, item.after, item.path));
  if (item.detail) {
    wrap.append(el('div', 'detail', item.detail));
  }
  return wrap;
}

function row(children: HTMLElement[]): HTMLElement {
  const e = el('div', 'context-row');
  e.append(...children);
  return e;
}

function span(className: string, text: string, title?: string): HTMLSpanElement {
  const e = el('span', className, text);
  if (title) e.title = title;
  return e;
}

function versionLabel(context: SidebarContext): string {
  if (!context.daemonVersion && !context.extensionVersion) return 'version';
  return 'daemon ' + (context.daemonVersion || '?') + ' · ext ' + (context.extensionVersion || '?');
}

function currentRunOptions(): RunOptions {
  const model = modelEl.value.trim();
  return {
    mode: (modeEl.value as RunOptions['mode']) || 'execute',
    permission: (permissionEl.value as RunOptions['permission']) || 'auto',
    ...(model ? { model } : {}),
  };
}
