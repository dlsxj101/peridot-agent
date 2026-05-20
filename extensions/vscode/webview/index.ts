import './style.css';
import type {
  InboundMessage,
  OutboundMessage,
  PlanSlice,
  QueuedMessage,
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
const root = document.getElementById('app') as HTMLElement;
const mascotUri = root.dataset.mascot ?? '';

// Last state snapshot — used so non-state-driven inputs (like the composer
// textarea while typing) survive a re-render.
let state: SidebarState | undefined;
let composerDraft = '';

window.addEventListener('message', (event: MessageEvent<InboundMessage>) => {
  if (event.data?.type === 'state') {
    state = event.data.state;
    render(state);
  }
});

function render(s: SidebarState): void {
  // Preserve composer draft across renders so streaming events don't
  // clobber what the user is typing.
  const textarea = document.getElementById('composer-input') as HTMLTextAreaElement | null;
  if (textarea) composerDraft = textarea.value;

  root.replaceChildren(s.view === 'landing' ? renderLanding(s) : renderSession(s));

  // Restore focus + draft.
  const newTextarea = document.getElementById('composer-input') as HTMLTextAreaElement | null;
  if (newTextarea) {
    newTextarea.value = composerDraft;
    if (document.activeElement === root) newTextarea.focus();
  }
}

// ──────────────────────────────────────────────────────────────────────
// Landing view: three-button entry pickers + nested forms.
// ──────────────────────────────────────────────────────────────────────

function renderLanding(s: SidebarState): HTMLElement {
  const screen = s.landing;
  const wrap = el('div', 'landing');

  const hero = el('div', 'hero');
  if (mascotUri) {
    const img = document.createElement('img');
    img.className = 'hero-mascot';
    img.src = mascotUri;
    img.alt = 'Peridot';
    img.width = 96;
    img.height = 96;
    hero.append(img);
  }
  hero.append(el('h1', 'hero-title', 'Peridot Agent'));
  hero.append(
    el(
      'p',
      'hero-tagline',
      'A Rust coding agent with native tools and a deer named Peridot.',
    ),
  );
  wrap.append(hero);

  if (screen === 'home') {
    wrap.append(renderLandingHome(s));
  } else if (screen === 'openrouter') {
    wrap.append(renderOpenRouterForm(s));
  } else {
    wrap.append(renderLocalLlmForm(s));
  }

  if (s.authError) {
    wrap.append(el('div', 'auth-error', s.authError));
  }
  return wrap;
}

function renderLandingHome(s: SidebarState): HTMLElement {
  const list = el('div', 'option-list');

  list.append(
    optionCard({
      title: 'Sign in with ChatGPT',
      body: 'OAuth via your ChatGPT account. Uses your existing subscription if eligible.',
      cta: 'Sign in',
      disabled: s.authBusy,
      onClick: () =>
        vscode.postMessage({ type: 'registerProvider', provider: 'chatgpt', params: {} }),
    }),
  );

  list.append(
    optionCard({
      title: 'OpenRouter API key',
      body: 'Bring a single key, route across 75+ models. Best for trying providers without committing.',
      cta: 'Set up key',
      disabled: s.authBusy,
      onClick: () => vscode.postMessage({ type: 'showLanding', screen: 'openrouter' }),
    }),
  );

  list.append(
    optionCard({
      title: 'Local LLM endpoint',
      body: 'Ollama, LM Studio, vLLM — anything that speaks the OpenAI HTTP API.',
      cta: 'Set up endpoint',
      disabled: s.authBusy,
      onClick: () => vscode.postMessage({ type: 'showLanding', screen: 'localLlm' }),
    }),
  );

  if (s.context.authConfigured) {
    list.append(
      el(
        'button',
        'link-button',
        'Skip — keep current provider',
      ).addEventListener('click' as never, (() =>
        vscode.postMessage({ type: 'showSession' })) as never) as unknown as HTMLElement,
    );
    // The above coercion is ugly — addEventListener returns void so we
    // can't chain. Rebuild it cleanly:
    list.lastChild?.remove();
    const skip = el('button', 'link-button', 'Skip — keep current provider');
    skip.addEventListener('click', () => vscode.postMessage({ type: 'showSession' }));
    list.append(skip);
  }
  return list;
}

interface OptionCardArgs {
  title: string;
  body: string;
  cta: string;
  disabled?: boolean;
  onClick: () => void;
}

function optionCard(opts: OptionCardArgs): HTMLElement {
  const card = el('button', 'option-card');
  card.type = 'button';
  card.disabled = !!opts.disabled;
  const title = el('div', 'option-title', opts.title);
  const body = el('div', 'option-body', opts.body);
  const cta = el('div', 'option-cta', opts.cta);
  card.append(title, body, cta);
  card.addEventListener('click', opts.onClick);
  return card;
}

function renderOpenRouterForm(s: SidebarState): HTMLElement {
  const form = el('form', 'landing-form');
  form.append(formBack());
  form.append(el('h2', 'form-title', 'OpenRouter API key'));
  form.append(
    el(
      'p',
      'form-help',
      "Get a key at openrouter.ai/keys. We store it in Peridot's local env store, never in your shell rc files.",
    ),
  );

  const keyField = labelledInput({
    id: 'or-key',
    label: 'API key',
    type: 'password',
    placeholder: 'sk-or-…',
    required: true,
  });
  form.append(keyField.wrap);

  const modelField = labelledInput({
    id: 'or-model',
    label: 'Default model (optional)',
    type: 'text',
    placeholder: 'anthropic/claude-sonnet-4',
  });
  form.append(modelField.wrap);

  form.append(submitButton('Save and continue', s.authBusy));

  form.addEventListener('submit', (event) => {
    event.preventDefault();
    if (s.authBusy) return;
    vscode.postMessage({
      type: 'registerProvider',
      provider: 'openrouter',
      params: {
        apiKey: keyField.input.value,
        model: modelField.input.value,
      },
    });
  });
  return form;
}

function renderLocalLlmForm(s: SidebarState): HTMLElement {
  const form = el('form', 'landing-form');
  form.append(formBack());
  form.append(el('h2', 'form-title', 'Local LLM endpoint'));
  form.append(
    el(
      'p',
      'form-help',
      "Any OpenAI-compatible HTTP API. Common bases: http://localhost:11434/v1 (Ollama), http://localhost:1234/v1 (LM Studio).",
    ),
  );

  const urlField = labelledInput({
    id: 'll-url',
    label: 'Base URL',
    type: 'url',
    placeholder: 'http://localhost:11434/v1',
    required: true,
  });
  form.append(urlField.wrap);

  const keyField = labelledInput({
    id: 'll-key',
    label: 'API key (use "local" if your server does not require one)',
    type: 'password',
    placeholder: 'local',
  });
  form.append(keyField.wrap);

  const modelField = labelledInput({
    id: 'll-model',
    label: 'Default model',
    type: 'text',
    placeholder: 'llama3.2:3b',
  });
  form.append(modelField.wrap);

  form.append(submitButton('Save and continue', s.authBusy));

  form.addEventListener('submit', (event) => {
    event.preventDefault();
    if (s.authBusy) return;
    vscode.postMessage({
      type: 'registerProvider',
      provider: 'localLlm',
      params: {
        baseUrl: urlField.input.value,
        apiKey: keyField.input.value || 'local',
        model: modelField.input.value,
      },
    });
  });
  return form;
}

function formBack(): HTMLElement {
  const back = el('button', 'link-button back-link', '← Back');
  back.type = 'button' as never;
  back.addEventListener('click', () =>
    vscode.postMessage({ type: 'showLanding', screen: 'home' }),
  );
  return back;
}

interface LabelledInput {
  wrap: HTMLElement;
  input: HTMLInputElement;
}

interface LabelledInputArgs {
  id: string;
  label: string;
  type: string;
  placeholder?: string;
  required?: boolean;
}

function labelledInput(args: LabelledInputArgs): LabelledInput {
  const wrap = el('label', 'form-field');
  wrap.htmlFor = args.id;
  wrap.append(el('span', 'form-label', args.label));
  const input = document.createElement('input');
  input.id = args.id;
  input.type = args.type;
  if (args.placeholder) input.placeholder = args.placeholder;
  if (args.required) input.required = true;
  input.className = 'form-input';
  input.autocomplete = 'off';
  input.spellcheck = false;
  wrap.append(input);
  return { wrap, input };
}

function submitButton(label: string, busy: boolean): HTMLElement {
  const button = el('button', 'primary-button', busy ? 'Working…' : label);
  button.type = 'submit' as never;
  if (busy) (button as HTMLButtonElement).disabled = true;
  return button;
}

// ──────────────────────────────────────────────────────────────────────
// Session view: header + HUD + transcript + queue + composer.
// ──────────────────────────────────────────────────────────────────────

function renderSession(s: SidebarState): HTMLElement {
  const wrap = el('div', 'session');
  wrap.append(renderHeader(s));
  wrap.append(renderContextStrip(s.context));
  if (hasHudData(s)) wrap.append(renderHud(s));
  wrap.append(renderTranscript(s));
  wrap.append(renderQueue(s));
  wrap.append(renderComposer(s));
  return wrap;
}

function renderHeader(s: SidebarState): HTMLElement {
  const header = el('header', 'session-header');
  const left = el('div', 'header-left');
  if (mascotUri) {
    const img = document.createElement('img');
    img.src = mascotUri;
    img.alt = '';
    img.className = 'header-mascot';
    img.width = 22;
    img.height = 22;
    left.append(img);
  }
  const titleWrap = el('div', 'header-title-wrap');
  titleWrap.append(el('div', 'header-title', 'Peridot'));
  titleWrap.append(el('div', 'header-status', s.status));
  left.append(titleWrap);

  const right = el('div', 'header-actions');
  right.append(iconButton('refresh', 'Refresh', () => vscode.postMessage({ type: 'refreshStatus' })));
  right.append(
    iconButton('switch', 'Switch provider', () =>
      vscode.postMessage({ type: 'showLanding', screen: 'home' }),
    ),
  );
  header.append(left, right);
  return header;
}

function iconButton(kind: string, label: string, onClick: () => void): HTMLElement {
  const btn = el('button', `icon-button icon-${kind}`);
  btn.type = 'button' as never;
  btn.title = label;
  btn.setAttribute('aria-label', label);
  btn.innerHTML = iconSvg(kind);
  btn.addEventListener('click', onClick);
  return btn;
}

function iconSvg(kind: string): string {
  // Inline minimal monochrome glyphs — `currentColor` lets the icons pick
  // up the VS Code button foreground color.
  switch (kind) {
    case 'refresh':
      return `<svg viewBox="0 0 16 16" width="14" height="14" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"><path d="M12.5 8a4.5 4.5 0 1 1-1.32-3.18"/><path d="M12.5 2.5v3h-3"/></svg>`;
    case 'switch':
      return `<svg viewBox="0 0 16 16" width="14" height="14" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"><path d="M3 6h9"/><path d="M9 3l3 3-3 3"/><path d="M13 10H4"/><path d="M7 13l-3-3 3-3"/></svg>`;
    case 'stop':
      return `<svg viewBox="0 0 16 16" width="14" height="14" fill="currentColor"><rect x="3.5" y="3.5" width="9" height="9" rx="1.5"/></svg>`;
    case 'send':
      return `<svg viewBox="0 0 16 16" width="14" height="14" fill="currentColor"><path d="M2 13l12-5L2 3l1.5 5L9 8l-5.5 0z"/></svg>`;
    case 'remove':
      return `<svg viewBox="0 0 16 16" width="12" height="12" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round"><path d="M4 4l8 8M12 4l-8 8"/></svg>`;
    case 'edit':
      return `<svg viewBox="0 0 16 16" width="12" height="12" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"><path d="M3 13l1-3 7-7 2 2-7 7-3 1z"/></svg>`;
    default:
      return '';
  }
}

function renderContextStrip(context: SidebarContext): HTMLElement {
  const strip = el('div', 'context-strip');
  const workspace = context.workspace || 'No workspace';
  strip.append(el('span', 'workspace-text', workspace));

  const pills = el('div', 'pill-row');
  if (context.provider) pills.append(pill(context.provider, 'provider'));
  if (context.model) pills.append(pill(context.model, 'model'));
  if (context.authConfigured) {
    pills.append(pill('auth ok', 'good'));
  } else {
    pills.append(pill('auth missing', 'warn'));
  }
  if (context.mode) pills.append(pill(context.mode, 'mode'));
  if (context.permission) pills.append(pill(context.permission, 'mode'));
  if (context.daemonVersion || context.extensionVersion) {
    pills.append(
      pill(
        `daemon ${context.daemonVersion ?? '?'} · ext ${context.extensionVersion ?? '?'}`,
        'mute',
      ),
    );
  }
  strip.append(pills);

  if (context.problem) {
    strip.append(el('div', 'problem-banner', context.problem));
  }
  return strip;
}

function pill(text: string, variant: string): HTMLElement {
  const span = el('span', `pill pill-${variant}`, text);
  span.title = text;
  return span;
}

function hasHudData(s: SidebarState): boolean {
  return Boolean(s.hud.usage || s.hud.budget || s.hud.context || s.hud.plan || s.hud.committee);
}

function renderHud(s: SidebarState): HTMLElement {
  const hud = el('div', 'hud');
  const hudState = s.hud;

  const meters = el('div', 'hud-meters');

  if (hudState.usage) {
    const u = hudState.usage;
    meters.append(
      meter(
        'Tokens',
        `${formatTokens(u.inputTokens)} in · ${formatTokens(u.outputTokens)} out`,
        formatUsd(u.costUsd),
      ),
    );
  }
  if (hudState.context && hudState.context.threshold > 0) {
    const pct = Math.min(1, hudState.context.tokensUsed / hudState.context.threshold);
    meters.append(
      barMeter('Context', `${Math.round(pct * 100)}%`, pct),
    );
  }
  if (hudState.budget) {
    const b = hudState.budget;
    const cost =
      typeof b.costLimit === 'number'
        ? `${formatUsd(b.costUsed)} / ${formatUsd(b.costLimit)}`
        : formatUsd(b.costUsed);
    const turns =
      typeof b.turnsLimit === 'number'
        ? `${b.turnsUsed}/${b.turnsLimit} turns`
        : `${b.turnsUsed} turns`;
    meters.append(meter('Budget', cost, turns));
  }
  if (hudState.committee) {
    for (const [role, slice] of Object.entries(hudState.committee)) {
      meters.append(
        meter(
          role.charAt(0).toUpperCase() + role.slice(1),
          formatUsd(slice.costUsd),
          `${formatTokens(slice.tokens)} tok`,
        ),
      );
    }
  }
  hud.append(meters);

  if (hudState.plan && hudState.plan.steps.length > 0) {
    hud.append(renderPlan(hudState.plan));
  }
  return hud;
}

function meter(label: string, primary: string, secondary: string): HTMLElement {
  const wrap = el('div', 'meter');
  wrap.append(el('span', 'meter-label', label));
  wrap.append(el('span', 'meter-primary', primary));
  wrap.append(el('span', 'meter-secondary', secondary));
  return wrap;
}

function barMeter(label: string, value: string, pct: number): HTMLElement {
  const wrap = el('div', 'meter meter-bar');
  wrap.append(el('span', 'meter-label', label));
  const barWrap = el('div', 'bar-wrap');
  const bar = el('div', 'bar');
  bar.style.width = `${Math.round(pct * 100)}%`;
  if (pct >= 0.9) bar.classList.add('bar-critical');
  else if (pct >= 0.75) bar.classList.add('bar-warn');
  barWrap.append(bar);
  wrap.append(barWrap, el('span', 'meter-primary', value));
  return wrap;
}

function renderPlan(plan: PlanSlice): HTMLElement {
  const details = el('details', 'plan-panel');
  details.open = true;
  const summary = el(
    'summary',
    'plan-summary',
    `Plan · ${plan.steps.length} step${plan.steps.length === 1 ? '' : 's'}` +
      (typeof plan.current === 'number' ? ` · on step ${plan.current + 1}` : ''),
  );
  details.append(summary);
  const ol = el('ol', 'plan-steps');
  plan.steps.forEach((step, index) => {
    const li = el('li', 'plan-step', step.text);
    if (step.status === 'done') li.classList.add('plan-done');
    if (plan.current === index) li.classList.add('plan-current');
    ol.append(li);
  });
  details.append(ol);
  return details;
}

// ──────────────────────────────────────────────────────────────────────
// Transcript: chat-style with tool cards and inline diffs.
// ──────────────────────────────────────────────────────────────────────

function renderTranscript(s: SidebarState): HTMLElement {
  const wrap = el('main', 'transcript');
  if (!s.transcript || s.transcript.length === 0) {
    wrap.append(renderEmptyState(s.context));
    return wrap;
  }
  s.transcript.forEach((item) => wrap.append(renderItem(item)));
  // Auto-scroll to bottom on new content.
  requestAnimationFrame(() => {
    wrap.scrollTop = wrap.scrollHeight;
  });
  return wrap;
}

function renderEmptyState(context: SidebarContext): HTMLElement {
  const wrap = el('div', 'empty-state');
  if (!context.workspace || context.workspace === 'No workspace') {
    wrap.append(el('div', 'empty-heading', 'No workspace open'));
    wrap.append(
      el(
        'div',
        'empty-body',
        'Open a folder to let Peridot run tasks against your project.',
      ),
    );
    return wrap;
  }
  wrap.append(el('div', 'empty-heading', 'Ready'));
  wrap.append(
    el(
      'div',
      'empty-body',
      'Type a task below. Enter sends, Shift+Enter inserts a newline.',
    ),
  );
  return wrap;
}

function renderItem(item: TranscriptItem): HTMLElement {
  switch (item.role) {
    case 'user':
      return renderUserBubble(item);
    case 'assistant':
      return renderAssistantBubble(item);
    case 'tool':
      return renderToolBlock(item);
    case 'status':
      return renderStatusLine(item);
    case 'error':
      return renderErrorLine(item);
    case 'interaction':
      return renderAskUserBubble(item);
    case 'approval':
      return renderApprovalBubble(item);
    case 'diff':
      return renderDiffBlock(item);
    default:
      return el('div', 'transcript-fallback', item.text);
  }
}

function renderUserBubble(item: TranscriptItem): HTMLElement {
  const wrap = el('section', 'msg msg-user');
  wrap.append(el('div', 'msg-label', 'You'));
  wrap.append(el('div', 'msg-body', item.text));
  return wrap;
}

function renderAssistantBubble(item: TranscriptItem): HTMLElement {
  const wrap = el('section', 'msg msg-assistant');
  wrap.append(el('div', 'msg-label', 'Peridot'));
  wrap.append(el('div', 'msg-body', item.text));
  return wrap;
}

function renderToolBlock(item: TranscriptItem): HTMLElement {
  const wrap = el('section', 'msg msg-tool');
  const header = el('div', 'tool-header');
  const dot = el('span', `tool-dot${item.pending ? ' tool-dot-pending' : ''}`);
  header.append(dot);
  header.append(el('span', 'tool-name', item.toolName || item.text));
  if (item.pending) {
    header.append(el('span', 'tool-status', 'running'));
  } else if (item.toolResultSummary) {
    header.append(el('span', 'tool-status tool-status-done', 'done'));
  }
  wrap.append(header);

  if (item.detail) {
    wrap.append(el('div', 'tool-detail', item.detail));
  }
  if (item.toolParameters !== undefined && !item.toolResultSummary) {
    const pre = el('pre', 'tool-code');
    pre.textContent = json(item.toolParameters);
    wrap.append(pre);
  } else if (item.toolResultSummary) {
    const pre = el('pre', 'tool-code');
    pre.textContent = item.toolResultSummary;
    wrap.append(pre);
  }
  return wrap;
}

function renderStatusLine(item: TranscriptItem): HTMLElement {
  const wrap = el('div', 'status-line');
  wrap.append(el('span', 'status-dot'));
  wrap.append(el('span', 'status-text', item.text));
  if (item.detail) wrap.append(el('span', 'status-detail', `· ${item.detail}`));
  return wrap;
}

function renderErrorLine(item: TranscriptItem): HTMLElement {
  const wrap = el('div', 'error-line');
  wrap.append(el('span', 'error-icon', '!'));
  wrap.append(el('span', 'error-text', item.text));
  return wrap;
}

function renderAskUserBubble(item: TranscriptItem): HTMLElement {
  const wrap = el('section', 'msg msg-prompt');
  wrap.append(el('div', 'msg-label', 'Peridot asks'));
  wrap.append(el('div', 'msg-body', item.text));
  wrap.append(renderAskUserForm(item));
  return wrap;
}

function renderApprovalBubble(item: TranscriptItem): HTMLElement {
  const wrap = el('section', 'msg msg-approval');
  wrap.append(el('div', 'msg-label', 'Approval requested'));
  wrap.append(el('div', 'msg-body', item.toolName || item.text));
  if (item.reason) wrap.append(el('div', 'approval-reason', item.reason));

  if (typeof item.before === 'string' || typeof item.after === 'string') {
    wrap.append(renderUnifiedDiff(item.before ?? '', item.after ?? '', item.path));
    if (item.path) {
      const openLink = el('button', 'link-button file-link', `Open ${item.path}`);
      openLink.type = 'button' as never;
      openLink.addEventListener('click', () =>
        vscode.postMessage({ type: 'openFile', path: item.path as string }),
      );
      wrap.append(openLink);
    }
  } else if (item.parameters !== undefined) {
    const pre = el('pre', 'tool-code');
    pre.textContent = json(item.parameters);
    wrap.append(pre);
  }

  const scope = document.createElement('select');
  scope.className = 'scope-select';
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

  const actions = el('div', 'msg-actions');
  const approve = el('button', 'primary-button compact-button', 'Approve');
  approve.type = 'button' as never;
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
  const deny = el('button', 'secondary-button compact-button', 'Deny');
  deny.type = 'button' as never;
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
  actions.append(scope, approve, deny);
  wrap.append(actions);
  return wrap;
}

function renderAskUserForm(item: TranscriptItem): HTMLElement {
  const wrap = el('div', 'ask-form');
  const request = isRecord(item.request) ? item.request : {};
  const kind = typeof request.kind === 'string' ? request.kind : '';
  const options = Array.isArray(request.options)
    ? request.options.filter((value): value is string => typeof value === 'string')
    : [];

  if (kind === 'single_select') {
    options.forEach((option, index) => {
      const label = el('label', 'choice');
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
      const label = el('label', 'choice');
      const input = document.createElement('input');
      input.type = 'checkbox';
      input.value = String(index);
      label.append(input, document.createTextNode(option));
      wrap.append(label);
    });
  } else {
    const input = el('input', 'inline-text-input') as HTMLInputElement;
    input.type = 'text';
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
      if (event.key === 'Escape' && item.requestId) {
        event.preventDefault();
        vscode.postMessage({
          type: 'askUserRespond',
          requestId: item.requestId,
          answer: { kind: 'cancelled' },
        });
      }
    });
  }

  const actions = el('div', 'msg-actions');
  const send = el('button', 'primary-button compact-button', 'Send');
  send.type = 'button' as never;
  send.addEventListener('click', sendAnswer);
  const cancel = el('button', 'secondary-button compact-button', 'Cancel');
  cancel.type = 'button' as never;
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

function renderDiffBlock(item: TranscriptItem): HTMLElement {
  const wrap = el('section', 'msg msg-diff');
  const header = el('div', 'diff-header');
  if (item.path) {
    const openLink = el('button', 'link-button file-link', item.path);
    openLink.type = 'button' as never;
    openLink.addEventListener('click', () =>
      vscode.postMessage({ type: 'openFile', path: item.path as string }),
    );
    header.append(openLink);
  } else {
    header.append(el('span', 'diff-path', item.text));
  }
  const stats = diffStats(item.before, item.after);
  header.append(el('span', 'diff-add', `+${stats.added}`));
  header.append(el('span', 'diff-del', `−${stats.removed}`));
  wrap.append(header);
  wrap.append(renderUnifiedDiff(item.before, item.after, item.path));
  if (item.detail) wrap.append(el('div', 'diff-meta', item.detail));
  return wrap;
}

// ──────────────────────────────────────────────────────────────────────
// Queue strip: messages typed while the agent is busy.
// ──────────────────────────────────────────────────────────────────────

function renderQueue(s: SidebarState): HTMLElement {
  if (s.queue.length === 0) {
    const empty = el('div', 'queue-empty');
    return empty;
  }
  const wrap = el('div', 'queue');
  const header = el('div', 'queue-header');
  header.append(
    el(
      'span',
      'queue-label',
      `${s.queue.length} queued — will run after the current task`,
    ),
  );
  const clear = el('button', 'link-button', 'Clear');
  clear.type = 'button' as never;
  clear.addEventListener('click', () => vscode.postMessage({ type: 'queueClear' }));
  header.append(clear);
  wrap.append(header);

  s.queue.forEach((item) => wrap.append(renderQueueItem(item)));
  return wrap;
}

function renderQueueItem(item: QueuedMessage): HTMLElement {
  const wrap = el('div', 'queue-item');

  const text = el('div', 'queue-text', item.text);
  text.contentEditable = 'true';
  text.spellcheck = false;
  // Save on blur so users can refine queued prompts without a Save button.
  text.addEventListener('blur', () => {
    const next = text.textContent ?? '';
    if (next.trim() !== item.text.trim()) {
      vscode.postMessage({ type: 'queueEdit', id: item.id, text: next });
    }
  });
  text.addEventListener('keydown', (event) => {
    if (event.key === 'Enter' && !event.shiftKey) {
      event.preventDefault();
      (event.target as HTMLElement).blur();
    }
  });
  wrap.append(text);

  const actions = el('div', 'queue-actions');
  const remove = el('button', 'icon-button mini', '');
  remove.type = 'button' as never;
  remove.title = 'Remove';
  remove.innerHTML = iconSvg('remove');
  remove.addEventListener('click', () =>
    vscode.postMessage({ type: 'queueRemove', id: item.id }),
  );
  actions.append(remove);
  wrap.append(actions);
  return wrap;
}

// ──────────────────────────────────────────────────────────────────────
// Composer.
// ──────────────────────────────────────────────────────────────────────

function renderComposer(s: SidebarState): HTMLElement {
  const wrap = el('form', 'composer');

  const optionsRow = el('div', 'composer-options');
  optionsRow.append(modeSelect(s.runOptions));
  optionsRow.append(permissionSelect(s.runOptions));
  optionsRow.append(modelInput(s.runOptions));
  wrap.append(optionsRow);

  const inputRow = el('div', 'composer-input-row');
  const textarea = document.createElement('textarea');
  textarea.id = 'composer-input';
  textarea.className = 'composer-textarea';
  textarea.placeholder = s.running
    ? 'Type another task — Enter queues it'
    : 'Ask Peridot to work in this repo';
  textarea.rows = 1;
  textarea.value = composerDraft;
  textarea.addEventListener('input', () => {
    composerDraft = textarea.value;
    autoresize(textarea);
  });
  textarea.addEventListener('keydown', (event) => {
    if (event.key === 'Enter' && !event.shiftKey && !event.isComposing) {
      event.preventDefault();
      handleSubmit();
    }
  });
  inputRow.append(textarea);

  const button = el('button', `composer-button ${s.running ? 'stop' : 'send'}`);
  button.type = 'button' as never;
  button.title = s.running ? 'Stop current task' : 'Send';
  button.innerHTML = iconSvg(s.running ? 'stop' : 'send');
  button.addEventListener('click', (event) => {
    event.preventDefault();
    if (s.running) {
      vscode.postMessage({ type: 'cancel' });
    } else {
      handleSubmit();
    }
  });
  inputRow.append(button);
  wrap.append(inputRow);

  // Auto-size on initial render to honor multi-line drafts.
  setTimeout(() => autoresize(textarea), 0);

  function handleSubmit(): void {
    const value = textarea.value.trim();
    if (!value) return;
    if (s.running) {
      vscode.postMessage({ type: 'queueAdd', task: value });
    } else {
      vscode.postMessage({
        type: 'run',
        task: value,
        options: currentOptionsFromDom(),
      });
    }
    textarea.value = '';
    composerDraft = '';
    autoresize(textarea);
  }

  return wrap;
}

function modeSelect(opts: RunOptions): HTMLSelectElement {
  const select = document.createElement('select');
  select.className = 'composer-select';
  select.id = 'composer-mode';
  select.title = 'Execution mode';
  for (const [value, label] of [
    ['execute', 'Execute'],
    ['plan', 'Plan'],
    ['goal', 'Goal'],
  ] as const) {
    const option = document.createElement('option');
    option.value = value;
    option.textContent = label;
    if (opts.mode === value) option.selected = true;
    select.append(option);
  }
  return select;
}

function permissionSelect(opts: RunOptions): HTMLSelectElement {
  const select = document.createElement('select');
  select.className = 'composer-select';
  select.id = 'composer-permission';
  select.title = 'Permission';
  for (const [value, label] of [
    ['auto', 'Auto'],
    ['safe', 'Safe'],
    ['yolo', 'Yolo'],
  ] as const) {
    const option = document.createElement('option');
    option.value = value;
    option.textContent = label;
    if (opts.permission === value) option.selected = true;
    select.append(option);
  }
  return select;
}

function modelInput(opts: RunOptions): HTMLInputElement {
  const input = document.createElement('input');
  input.className = 'composer-model';
  input.id = 'composer-model';
  input.placeholder = 'model override';
  input.value = opts.model ?? '';
  input.spellcheck = false;
  input.autocomplete = 'off';
  return input;
}

function currentOptionsFromDom(): RunOptions {
  const mode = (document.getElementById('composer-mode') as HTMLSelectElement | null)?.value ?? 'execute';
  const permission =
    (document.getElementById('composer-permission') as HTMLSelectElement | null)?.value ?? 'auto';
  const modelValue = (
    document.getElementById('composer-model') as HTMLInputElement | null
  )?.value.trim();
  const options: RunOptions = {
    mode: mode as RunOptions['mode'],
    permission: permission as RunOptions['permission'],
  };
  if (modelValue) options.model = modelValue;
  return options;
}

function autoresize(textarea: HTMLTextAreaElement): void {
  textarea.style.height = 'auto';
  textarea.style.height = `${Math.min(textarea.scrollHeight, 180)}px`;
}

