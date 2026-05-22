import './style.css';
import MarkdownIt from 'markdown-it';
import type {
  InboundMessage,
  OutboundMessage,
  PlanSlice,
  QueuedMessage,
  RunOptions,
  SidebarContext,
  SidebarState,
  SlashCommandSpec,
  TranscriptItem,
} from '../src/types';
import { diffStats, renderUnifiedDiff } from './diff';
import { el, formatTokens, formatUsd, highlightLite, isRecord, json } from './util';

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
// Pending composer selections — captured pre-render so a state update
// triggered by a streaming event doesn't reset the user's mid-edit
// mode / permission / model picks back to whatever was last submitted.
let composerModeOverride: string | undefined;
let composerPermissionOverride: string | undefined;
let composerModelOverride: string | undefined;
let transcriptPinnedToBottom = true;
let transcriptScrollTop = 0;
let forceTranscriptBottomOnce = false;
let transcriptScrollRestorePending = false;
let transcriptScrollRestoreToken = 0;
let lastTranscriptAnimationKey = '';
let lastTranscriptCount = 0;
let lastComposerRunning: boolean | undefined;
interface AssistantStreamSnapshot {
  markdown: string;
  visibleText: string;
}
const assistantTextByKey = new Map<string, AssistantStreamSnapshot>();
let lastComposerSessionKey = '';
let toolHistoryOpen = false;
let slashPickerSelected = 0;
let slashCommands: SlashCommandSpec[] = [];
let todoExpanded = false;
let lastTodoCurrentKey = '';

const CHATGPT_MODELS = ['gpt-5.5', 'gpt-5.5-fast', 'gpt-5.4', 'gpt-5.4-mini'];

const markdownRenderer = new MarkdownIt({
  html: false,
  linkify: true,
  typographer: false,
});
markdownRenderer.validateLink = (url: string): boolean => {
  const normalized = url.trim().toLowerCase();
  return (
    normalized.startsWith('https://') ||
    normalized.startsWith('http://') ||
    normalized.startsWith('mailto:')
  );
};

window.addEventListener('message', (event: MessageEvent<InboundMessage>) => {
  if (event.data?.type === 'state') {
    state = event.data.state;
    render(state);
  }
});
vscode.postMessage({ type: 'ready' });

function render(s: SidebarState): void {
  slashCommands = s.slashCommands;
  const composerSessionKey = s.view === 'session' ? s.activeChatId ?? s.sessionId ?? 'draft' : s.view;
  const composerSessionChanged = composerSessionKey !== lastComposerSessionKey;
  if (composerSessionChanged) {
    composerModeOverride = undefined;
    composerPermissionOverride = undefined;
    composerModelOverride = undefined;
    lastComposerSessionKey = composerSessionKey;
  }
  // Preserve composer draft / selection across renders so streaming
  // events don't clobber what the user is typing or picking.
  const textarea = document.getElementById('composer-input') as HTMLTextAreaElement | null;
  if (textarea) composerDraft = textarea.value;
  const modeEl = document.getElementById('composer-mode') as HTMLSelectElement | null;
  if (modeEl && !composerSessionChanged) composerModeOverride = modeEl.value;
  const permEl = document.getElementById('composer-permission') as HTMLSelectElement | null;
  if (permEl && !composerSessionChanged) composerPermissionOverride = permEl.value;
  const modelEl = document.getElementById('composer-model') as HTMLInputElement | null;
  if (modelEl && !composerSessionChanged) composerModelOverride = modelEl.value;
  const transcriptEl = document.querySelector<HTMLElement>('.transcript');
  if (transcriptEl && !forceTranscriptBottomOnce && !transcriptScrollRestorePending) {
    transcriptPinnedToBottom = isTranscriptAtBottom(transcriptEl);
    transcriptScrollTop = transcriptEl.scrollTop;
  } else if (forceTranscriptBottomOnce) {
    transcriptPinnedToBottom = true;
  }
  // Remember which element had focus so we can re-focus after the
  // destructive replaceChildren below.
  const focusId = (document.activeElement && (document.activeElement as HTMLElement).id) || '';

  root.replaceChildren(s.view === 'landing' ? renderLanding(s) : renderSession(s));

  const newTextarea = document.getElementById('composer-input') as HTMLTextAreaElement | null;
  if (newTextarea) newTextarea.value = composerDraft;
  if (focusId) {
    const target = document.getElementById(focusId) as HTMLElement | null;
    target?.focus({ preventScroll: true });
  }
}

function isTranscriptAtBottom(node: HTMLElement): boolean {
  return node.scrollHeight - node.scrollTop - node.clientHeight <= 24;
}

function pinTranscriptToBottomOnNextRender(): void {
  forceTranscriptBottomOnce = true;
  transcriptPinnedToBottom = true;
  const transcriptEl = document.querySelector<HTMLElement>('.transcript');
  if (transcriptEl) {
    transcriptEl.scrollTop = transcriptEl.scrollHeight;
    transcriptScrollTop = transcriptEl.scrollTop;
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
  } else if (screen === 'localLlm') {
    wrap.append(renderLocalLlmForm(s));
  } else if (screen === 'claude') {
    wrap.append(renderClaudeForm(s));
  } else if (screen === 'openai') {
    wrap.append(renderOpenAiForm(s));
  }

  if (s.authError) {
    wrap.append(el('div', 'auth-error', s.authError));
  }
  return wrap;
}

function renderLandingHome(s: SidebarState): HTMLElement {
  const list = el('div', 'option-list');

  // Primary providers — the two the team recommends for most users.
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
      body: 'One key, 75+ models. Easiest way to try providers without committing.',
      cta: 'Set up key',
      disabled: s.authBusy,
      onClick: () => vscode.postMessage({ type: 'showLanding', screen: 'openrouter' }),
    }),
  );

  // Secondary providers — kept for users who already have direct accounts.
  const divider = el('div', 'option-divider');
  divider.append(el('span', 'option-divider-text', 'or use another provider'));
  list.append(divider);

  list.append(
    optionCardCompact({
      title: 'Anthropic API key',
      body: 'Direct Claude API access.',
      disabled: s.authBusy,
      onClick: () => vscode.postMessage({ type: 'showLanding', screen: 'claude' }),
    }),
  );

  list.append(
    optionCardCompact({
      title: 'OpenAI API key',
      body: 'Direct GPT API access.',
      disabled: s.authBusy,
      onClick: () => vscode.postMessage({ type: 'showLanding', screen: 'openai' }),
    }),
  );

  list.append(
    optionCardCompact({
      title: 'Local LLM endpoint',
      body: 'Ollama, LM Studio, vLLM — anything OpenAI-compatible.',
      disabled: s.authBusy,
      onClick: () => vscode.postMessage({ type: 'showLanding', screen: 'localLlm' }),
    }),
  );

  if (s.context.authConfigured) {
    const skip = el('button', 'link-button', 'Skip — keep current provider');
    skip.type = 'button';
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
  const card = el('button', `option-card${opts.disabled ? ' busy' : ''}`);
  card.type = 'button';
  card.disabled = !!opts.disabled;
  const title = el('div', 'option-title', opts.title);
  const body = el('div', 'option-body', opts.body);
  const cta = el('div', 'option-cta', opts.cta);
  card.append(title, body, cta);
  card.addEventListener('click', opts.onClick);
  return card;
}

interface OptionCardCompactArgs {
  title: string;
  body: string;
  disabled?: boolean;
  onClick: () => void;
}

function optionCardCompact(opts: OptionCardCompactArgs): HTMLElement {
  const card = el('button', `option-card option-card-compact${opts.disabled ? ' busy' : ''}`);
  card.type = 'button';
  card.disabled = !!opts.disabled;
  const title = el('div', 'option-title', opts.title);
  const body = el('div', 'option-body', opts.body);
  card.append(title, body);
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

function renderClaudeForm(s: SidebarState): HTMLElement {
  const form = el('form', 'landing-form');
  form.append(formBack());
  form.append(el('h2', 'form-title', 'Anthropic API key'));
  form.append(
    el(
      'p',
      'form-help',
      'Get a key at console.anthropic.com/settings/keys. We store it in Peridot\'s local env store; the daemon picks it up from ANTHROPIC_API_KEY.',
    ),
  );

  const keyField = labelledInput({
    id: 'an-key',
    label: 'API key',
    type: 'password',
    placeholder: 'sk-ant-…',
    required: true,
  });
  form.append(keyField.wrap);

  const modelField = labelledInput({
    id: 'an-model',
    label: 'Default model (optional)',
    type: 'text',
    placeholder: 'claude-sonnet-4-6',
  });
  form.append(modelField.wrap);

  form.append(submitButton('Save and continue', s.authBusy));

  form.addEventListener('submit', (event) => {
    event.preventDefault();
    if (s.authBusy) return;
    vscode.postMessage({
      type: 'registerProvider',
      provider: 'claude',
      params: {
        apiKey: keyField.input.value,
        model: modelField.input.value,
      },
    });
  });
  return form;
}

function renderOpenAiForm(s: SidebarState): HTMLElement {
  const form = el('form', 'landing-form');
  form.append(formBack());
  form.append(el('h2', 'form-title', 'OpenAI API key'));
  form.append(
    el(
      'p',
      'form-help',
      'Get a key at platform.openai.com/api-keys. Stored locally as OPENAI_API_KEY.',
    ),
  );

  const keyField = labelledInput({
    id: 'oa-key',
    label: 'API key',
    type: 'password',
    placeholder: 'sk-…',
    required: true,
  });
  form.append(keyField.wrap);

  const modelField = labelledInput({
    id: 'oa-model',
    label: 'Default model (optional)',
    type: 'text',
    placeholder: 'gpt-5',
  });
  form.append(modelField.wrap);

  form.append(submitButton('Save and continue', s.authBusy));

  form.addEventListener('submit', (event) => {
    event.preventDefault();
    if (s.authBusy) return;
    vscode.postMessage({
      type: 'registerProvider',
      provider: 'openai',
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
  back.type = 'button';
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
  const button = el('button', `primary-button${busy ? ' busy' : ''}`);
  button.type = 'submit';
  if (busy) {
    (button as HTMLButtonElement).disabled = true;
    const spinner = el('span', 'spinner');
    spinner.setAttribute('aria-hidden', 'true');
    button.append(spinner);
    button.append(document.createTextNode('Working…'));
  } else {
    button.textContent = label;
  }
  return button;
}

// ──────────────────────────────────────────────────────────────────────
// Session view: header + HUD + transcript + queue + composer.
// ──────────────────────────────────────────────────────────────────────

function renderSession(s: SidebarState): HTMLElement {
  const wrap = el('div', 'session');
  wrap.append(renderHeader(s));
  wrap.append(renderContextStrip(s.context));
  if (s.hud.plan && s.hud.plan.steps.length > 0) wrap.append(renderTodoProgress(s.hud.plan, s.running));
  if (hasHudData(s)) wrap.append(renderHud(s));
  wrap.append(renderTranscript(s));
  wrap.append(renderQueue(s));
  if (s.branchPicker) wrap.append(renderBranchPicker(s));
  const approval = latestPendingApproval(s);
  if (approval) wrap.append(renderApprovalDock(approval));
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
    img.width = 66;
    img.height = 66;
    left.append(img);
  }
  const titleWrap = el('div', 'header-title-wrap');
  titleWrap.append(el('div', 'header-title', 'Peridot Agent'));
  titleWrap.append(
    el(
      'div',
      `header-status ${isAnimatedStatus(s.status) ? 'text-gradient-active' : ''}`,
      statusLabel(s.status),
    ),
  );
  left.append(titleWrap);

  const right = el('div', 'header-actions');
  right.append(renderSessionMenu(s));
  right.append(iconButton('refresh', 'Refresh', () => vscode.postMessage({ type: 'refreshStatus' })));
  right.append(
    iconButton('switch', 'Switch provider', () =>
      vscode.postMessage({ type: 'showLanding', screen: 'home' }),
    ),
  );
  header.append(left, right);
  return header;
}

function isAnimatedStatus(status: string): boolean {
  return ['Waiting for model', 'Starting daemon', 'Running'].includes(status);
}

function statusLabel(status: string): string {
  return status === 'Waiting for model' ? 'Preparing response' : status;
}

function renderSessionMenu(s: SidebarState): HTMLElement {
  const active = s.sessions.find((session) => session.active);
  const details = el('details', 'session-menu');
  const summary = el('summary', 'session-menu-trigger');
  summary.title = 'Open sessions';
  summary.setAttribute('aria-label', 'Open sessions');
  summary.append(el('span', 'session-menu-current', active?.title ?? 'New session'));
  summary.append(el('span', 'session-menu-chevron', ''));
  details.append(summary);

  const menu = el('div', 'session-menu-list');
  const newButton = el('button', 'session-menu-item session-menu-new');
  newButton.type = 'button';
  const newIcon = el('span', 'session-menu-icon');
  newIcon.innerHTML = iconSvg('new');
  newButton.append(newIcon);
  const newText = el('span', 'session-menu-text');
  newText.append(el('span', 'session-menu-title', 'New session'));
  newText.append(el('span', 'session-menu-subtitle', 'Start when you send the first message'));
  newButton.append(newText);
  newButton.addEventListener('click', () => {
    composerDraft = '';
    vscode.postMessage({ type: 'newSession' });
  });
  menu.append(newButton);

  if (s.sessions.length > 0) {
    menu.append(el('div', 'session-menu-divider'));
  }

  for (const session of s.sessions) {
    const item = el('button', `session-menu-item ${session.active ? 'active' : ''}`);
    item.type = 'button';
    item.disabled = session.active;
    const marker = el('span', `session-menu-marker ${session.running ? 'running' : ''}`);
    marker.textContent = session.active ? '✓' : session.running ? '●' : '';
    const text = el('span', 'session-menu-text');
    text.append(el('span', 'session-menu-title', session.title));
    text.append(el('span', 'session-menu-subtitle', session.running ? 'Running' : session.status));
    item.append(marker, text);
    item.addEventListener('click', () => {
      composerDraft = '';
      vscode.postMessage({ type: 'selectSession', id: session.id });
    });
    menu.append(item);
  }

  details.append(menu);
  return details;
}

function iconButton(kind: string, label: string, onClick: () => void): HTMLElement {
  const btn = el('button', `icon-button icon-${kind}`);
  btn.type = 'button';
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
    case 'new':
      return `<svg viewBox="0 0 16 16" width="14" height="14" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round"><path d="M8 3v10M3 8h10"/></svg>`;
    case 'stop':
      return `<svg viewBox="0 0 16 16" width="14" height="14" fill="currentColor"><rect x="3.5" y="3.5" width="9" height="9" rx="1.5"/></svg>`;
    case 'send':
      return `<svg viewBox="0 0 16 16" width="14" height="14" fill="currentColor"><path d="M2 13l12-5L2 3l1.5 5L9 8l-5.5 0z"/></svg>`;
    case 'remove':
      return `<svg viewBox="0 0 16 16" width="12" height="12" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round"><path d="M4 4l8 8M12 4l-8 8"/></svg>`;
    case 'copy':
      return `<svg viewBox="0 0 16 16" width="13" height="13" fill="none" stroke="currentColor" stroke-width="1.4" stroke-linecap="round" stroke-linejoin="round"><rect x="5" y="5" width="8" height="8" rx="1.5"/><path d="M3 10.5H2.8A1.8 1.8 0 0 1 1 8.7V2.8A1.8 1.8 0 0 1 2.8 1h5.9A1.8 1.8 0 0 1 10.5 2.8V3"/></svg>`;
    case 'check':
      return `<svg viewBox="0 0 16 16" width="13" height="13" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round"><path d="M3 8.5l3 3L13 4"/></svg>`;
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
  if (context.reasoningEffort) pills.append(pill(`reasoning ${context.reasoningEffort}`, 'mode'));
  if (context.serviceTier && context.serviceTier !== 'standard') {
    pills.append(pill(context.serviceTier, 'mode'));
  }
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
  return Boolean(s.hud.usage || s.hud.budget || s.hud.committee);
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

  return hud;
}

function renderContextDock(s: SidebarState): HTMLElement {
  const context = s.hud.context;
  const dock = el('div', 'context-dock');
  if (!context || context.threshold <= 0) return dock;
  const pct = Math.min(1, context.tokensUsed / context.threshold);
  const pctText = `${Math.round(pct * 100)}%`;
  const exact = `${context.tokensUsed.toLocaleString()} / ${context.threshold.toLocaleString()} tokens`;
  const donut = el('div', 'context-donut');
  donut.style.setProperty('--context-pct', `${Math.round(pct * 100)}%`);
  if (pct >= 0.9) donut.classList.add('critical');
  else if (pct >= 0.75) donut.classList.add('warn');
  donut.title = `Context ${exact} (${pctText})`;
  dock.append(donut);
  return dock;
}

function meter(label: string, primary: string, secondary: string): HTMLElement {
  const wrap = el('div', 'meter');
  wrap.append(el('span', 'meter-label', label));
  wrap.append(el('span', 'meter-primary', primary));
  wrap.append(el('span', 'meter-secondary', secondary));
  return wrap;
}

function renderTodoProgress(plan: PlanSlice, running: boolean): HTMLElement {
  const currentIndex = activePlanIndex(plan);
  const current = plan.steps[currentIndex] ?? plan.steps[0];
  const doneCount = plan.steps.filter((step) => isDoneStep(step.status)).length;
  const key = `${currentIndex}:${current?.text ?? ''}:${current?.status ?? ''}:${running}`;
  const changed = lastTodoCurrentKey.length > 0 && lastTodoCurrentKey !== key;
  lastTodoCurrentKey = key;

  const details = el('details', 'todo-progress');
  details.open = todoExpanded;
  details.addEventListener('toggle', () => {
    todoExpanded = details.open;
  });

  const summary = el('summary', `todo-summary ${changed && !todoExpanded ? 'is-changing' : ''}`);
  const label = el('span', 'todo-summary-label', 'Todo');
  const text = el('span', 'todo-summary-text');
  text.textContent = current
    ? current.text
    : `${doneCount}/${plan.steps.length} complete`;
  const meta = el('span', 'todo-summary-meta', `${doneCount}/${plan.steps.length}`);
  summary.append(label);
  if (running && current && !isDoneStep(current.status)) summary.append(el('span', 'todo-spinner'));
  summary.append(text, meta);
  details.append(summary);

  const ol = el('ol', 'todo-steps');
  plan.steps.forEach((step, index) => {
    const li = el('li', `todo-step ${statusClass(step.status)} ${index === currentIndex ? 'current' : ''}`);
    li.append(el('span', 'todo-step-marker', stepMarker(step.status, index === currentIndex)));
    li.append(el('span', 'todo-step-text', step.text));
    ol.append(li);
  });
  details.append(ol);
  return details;
}

function activePlanIndex(plan: PlanSlice): number {
  if (
    typeof plan.current === 'number' &&
    plan.current >= 0 &&
    plan.current < plan.steps.length
  ) {
    return plan.current;
  }
  const firstActive = plan.steps.findIndex((step) => !isDoneStep(step.status));
  return firstActive >= 0 ? firstActive : Math.max(0, plan.steps.length - 1);
}

function isDoneStep(status: string | undefined): boolean {
  return status === 'done' || status === 'completed';
}

function statusClass(status: string | undefined): string {
  if (isDoneStep(status)) return 'done';
  if (status === 'in_progress' || status === 'active') return 'active';
  return 'pending';
}

function stepMarker(status: string | undefined, current: boolean): string {
  if (isDoneStep(status)) return '✓';
  if (current) return '●';
  return '';
}

// ──────────────────────────────────────────────────────────────────────
// Transcript: chat-style with tool cards and inline diffs.
// ──────────────────────────────────────────────────────────────────────

function renderTranscript(s: SidebarState): HTMLElement {
  const wrap = el('main', 'transcript');
  const transcriptKey = s.activeChatId ?? s.sessionId ?? 'draft';
  const sameTranscript = transcriptKey === lastTranscriptAnimationKey;
  const animationStartIndex = forceTranscriptBottomOnce
    ? Math.max(0, s.transcript.length - 1)
    : sameTranscript
      ? lastTranscriptCount
      : s.transcript.length;
  wrap.addEventListener(
    'scroll',
    () => {
      transcriptPinnedToBottom = isTranscriptAtBottom(wrap);
      transcriptScrollTop = wrap.scrollTop;
    },
    { passive: true },
  );
  if (!s.transcript || s.transcript.length === 0) {
    wrap.append(renderEmptyState(s.context));
    lastTranscriptAnimationKey = transcriptKey;
    lastTranscriptCount = 0;
    transcriptScrollRestoreToken += 1;
    transcriptScrollRestorePending = false;
    forceTranscriptBottomOnce = false;
    return wrap;
  }
  for (let index = 0; index < s.transcript.length; index += 1) {
    const item = s.transcript[index];
    if (item.role === 'tool') {
      const tools: TranscriptItem[] = [];
      while (index < s.transcript.length && s.transcript[index].role === 'tool') {
        tools.push(s.transcript[index]);
        index += 1;
      }
      const stackEnd = index - 1;
      index -= 1;
      wrap.append(
        decorateTranscriptEntry(
          renderToolStack(tools),
          tools[tools.length - 1],
          stackEnd >= animationStartIndex,
        ),
      );
    } else {
      wrap.append(
        decorateTranscriptEntry(
          renderItem(item, `${transcriptKey}:${index}`),
          item,
          index >= animationStartIndex,
        ),
      );
    }
  }
  lastTranscriptAnimationKey = transcriptKey;
  lastTranscriptCount = s.transcript.length;
  const restoreToken = ++transcriptScrollRestoreToken;
  transcriptScrollRestorePending = true;
  requestAnimationFrame(() => {
    if (!wrap.isConnected || restoreToken !== transcriptScrollRestoreToken) return;
    if (forceTranscriptBottomOnce || transcriptPinnedToBottom) {
      wrap.scrollTop = wrap.scrollHeight;
    } else {
      const maxScrollTop = Math.max(0, wrap.scrollHeight - wrap.clientHeight);
      wrap.scrollTop = Math.min(transcriptScrollTop, maxScrollTop);
    }
    forceTranscriptBottomOnce = false;
    transcriptScrollRestorePending = false;
    transcriptScrollTop = wrap.scrollTop;
  });
  return wrap;
}

function decorateTranscriptEntry(
  node: HTMLElement,
  item: TranscriptItem,
  shouldAnimate: boolean,
): HTMLElement {
  if (!shouldAnimate) return node;
  node.classList.add('bubble-enter', `bubble-enter-${animationKindForItem(item)}`);
  return node;
}

function animationKindForItem(item: TranscriptItem): string {
  switch (item.role) {
    case 'user':
      return 'user';
    case 'assistant':
      return 'assistant';
    case 'tool':
      return 'tool';
    case 'interaction':
    case 'approval':
      return 'prompt';
    default:
      return 'neutral';
  }
}

function renderEmptyState(context: SidebarContext): HTMLElement {
  const wrap = el('div', 'empty-state');
  if (mascotUri) {
    const img = document.createElement('img');
    img.className = 'empty-mascot';
    img.src = mascotUri;
    img.alt = '';
    img.width = 56;
    img.height = 56;
    wrap.append(img);
  }
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
  wrap.append(el('div', 'empty-heading', 'Peridot is ready'));
  wrap.append(
    el(
      'div',
      'empty-body',
      'Describe what you want done. Peridot reads your repo, runs tools, asks before touching anything risky.',
    ),
  );

  const tips = el('div', 'empty-tips');
  const tip1 = el('span', 'empty-tip');
  tip1.append(el('span', 'kbd', 'Enter'));
  tip1.append(document.createTextNode(' to send'));
  tips.append(tip1);

  const tip2 = el('span', 'empty-tip');
  tip2.append(el('span', 'kbd', 'Shift'));
  tip2.append(document.createTextNode(' + '));
  tip2.append(el('span', 'kbd', 'Enter'));
  tip2.append(document.createTextNode(' for a newline'));
  tips.append(tip2);

  wrap.append(tips);
  return wrap;
}

function renderItem(item: TranscriptItem, itemKey?: string): HTMLElement {
  switch (item.role) {
    case 'user':
      return renderUserBubble(item);
    case 'assistant':
      return renderAssistantBubble(item, itemKey);
    case 'tool':
      return renderToolBlock(item);
    case 'status':
      return renderStatusLine(item);
    case 'error':
      return renderErrorLine(item);
    case 'thinking':
      return renderThinkingBlock(item);
    case 'interaction':
      return renderAskUserBubble(item);
    case 'approval':
      return renderApprovalBubble(item);
    case 'diff':
      return renderDiffBlock(item);
    case 'command':
      return renderCommandBlock(item);
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

function renderAssistantBubble(item: TranscriptItem, itemKey?: string): HTMLElement {
  const wrap = el('section', 'msg msg-assistant');
  wrap.append(el('div', 'msg-label', 'Peridot'));
  const streamKey = itemKey ?? `assistant:${item.text.length}`;
  const previous = assistantTextByKey.get(streamKey);
  const body = renderMarkdownBody(item.text);
  const visibleText = body.textContent ?? '';
  const textGrew =
    previous !== undefined &&
    item.text.length > previous.markdown.length &&
    item.text.startsWith(previous.markdown);
  if (textGrew) {
    animateVisibleTextSuffix(body, commonPrefixLength(previous.visibleText, visibleText));
  }
  assistantTextByKey.set(streamKey, { markdown: item.text, visibleText });
  wrap.append(body);
  const copy = el('button', 'copy-button', '');
  copy.type = 'button';
  copy.title = 'Copy response';
  copy.setAttribute('aria-label', 'Copy response');
  copy.innerHTML = iconSvg('copy');
  copy.addEventListener('click', () => {
    void markCopied(copy, item.text);
  });
  wrap.append(copy);
  return wrap;
}

async function markCopied(button: HTMLElement, text: string): Promise<void> {
  await copyText(text);
  button.classList.add('copied');
  button.innerHTML = iconSvg('check');
  button.title = 'Copied';
  button.setAttribute('aria-label', 'Copied');
  setTimeout(() => {
    button.classList.remove('copied');
    button.innerHTML = iconSvg('copy');
    button.title = 'Copy response';
    button.setAttribute('aria-label', 'Copy response');
  }, 3000);
}

async function copyText(text: string): Promise<void> {
  if (navigator.clipboard?.writeText) {
    await navigator.clipboard.writeText(text);
    return;
  }
  const textarea = document.createElement('textarea');
  textarea.value = text;
  textarea.style.position = 'fixed';
  textarea.style.left = '-9999px';
  document.body.append(textarea);
  textarea.select();
  document.execCommand('copy');
  textarea.remove();
}

function renderMarkdownBody(markdown: string): HTMLElement {
  const body = el('div', 'msg-body markdown-body');
  body.innerHTML = markdownRenderer.render(markdown);
  body.querySelectorAll('a[href]').forEach((link) => {
    link.setAttribute('target', '_blank');
    link.setAttribute('rel', 'noreferrer noopener');
  });
  body.querySelectorAll('table').forEach((table) => {
    const wrap = el('div', 'md-table-wrap');
    table.replaceWith(wrap);
    wrap.append(table);
  });
  return body;
}

function commonPrefixLength(left: string, right: string): number {
  const max = Math.min(left.length, right.length);
  let index = 0;
  while (index < max && left.charCodeAt(index) === right.charCodeAt(index)) {
    index += 1;
  }
  return index;
}

function animateVisibleTextSuffix(rootEl: HTMLElement, startOffset: number): void {
  const walker = document.createTreeWalker(rootEl, NodeFilter.SHOW_TEXT);
  const textNodes: Text[] = [];
  while (walker.nextNode()) {
    const node = walker.currentNode as Text;
    if (!node.nodeValue || isAnimationSkippedTextNode(node)) continue;
    textNodes.push(node);
  }

  let offset = 0;
  let delayIndex = 0;
  for (const node of textNodes) {
    const text = node.nodeValue ?? '';
    const nodeStart = offset;
    const nodeEnd = nodeStart + text.length;
    offset = nodeEnd;
    if (nodeEnd <= startOffset) continue;

    const splitIndex = Math.max(0, startOffset - nodeStart);
    const before = text.slice(0, splitIndex);
    const suffix = text.slice(splitIndex);
    const fragment = document.createDocumentFragment();
    if (before) fragment.append(document.createTextNode(before));
    for (const char of Array.from(suffix)) {
      const span = document.createElement('span');
      span.className = 'stream-weight-char';
      span.textContent = char;
      span.style.setProperty('--stream-delay', `${Math.min(delayIndex, 24) * 18}ms`);
      if (/\s/.test(char)) span.classList.add('stream-weight-space');
      fragment.append(span);
      delayIndex += 1;
    }
    node.replaceWith(fragment);
  }
}

function isAnimationSkippedTextNode(node: Text): boolean {
  const parent = node.parentElement;
  return Boolean(parent?.closest('pre, code, .tool-code, .command-code'));
}

function renderToolBlock(item: TranscriptItem): HTMLElement {
  return renderToolStack([item]);
}

function renderToolStack(items: TranscriptItem[]): HTMLElement {
  const latest = items[items.length - 1];
  const details = el('details', `tool-stack ${latest.pending ? 'tool-stack-running' : ''}`);
  details.open = toolHistoryOpen;
  details.addEventListener('toggle', () => {
    toolHistoryOpen = details.open;
    if (details.open) {
      ensureToolHistoryRendered(details, items);
    }
  });
  const summary = el('summary', 'tool-summary');
  const name = el(
    'span',
    `tool-name ${latest.pending ? 'text-gradient-active' : ''}`,
    latest.toolName || latest.text,
  );
  const result = el('span', 'tool-result', compactMiddle(toolSummaryText(latest), 180));
  const status = el(
    'span',
    `tool-status${latest.pending ? ' tool-status-running' : ' tool-status-done'}`,
    latest.pending ? 'running' : 'done',
  );
  const toggle = el('span', 'tool-toggle', items.length > 1 ? `${items.length}` : '');
  summary.append(toggle, name);
  if (latest.path) {
    summary.append(renderFilePathButton(latest.path, 'tool-path', latest.line, latest.column));
  }
  summary.append(result, status);
  details.append(summary);

  if (details.open) {
    ensureToolHistoryRendered(details, items);
  }
  return details;
}

function ensureToolHistoryRendered(details: HTMLElement, items: TranscriptItem[]): void {
  if (details.querySelector('.tool-history')) return;
  const history = el('div', 'tool-history');
  items.forEach((item) => history.append(renderToolDetail(item)));
  details.append(history);
}

function renderThinkingBlock(item: TranscriptItem): HTMLElement {
  const details = el('details', 'thinking-block thinking-active');
  const summary = el('summary', 'thinking-summary');
  const label = el('span', 'thinking-label text-gradient-active', 'Thinking');
  const state = el('span', 'thinking-state', 'reasoning trace');
  summary.append(el('span', 'thinking-pulse'), label, state);
  details.append(summary);
  const body = el('pre', 'thinking-body');
  body.textContent = item.text;
  details.append(body);
  return details;
}

function compactMiddle(text: string, maxLength: number): string {
  if (text.length <= maxLength) return text;
  const keep = Math.max(20, Math.floor((maxLength - 1) / 2));
  return `${text.slice(0, keep)}…${text.slice(-keep)}`;
}

function toolSummaryText(item: TranscriptItem): string {
  if (item.toolResultSummary) return item.toolResultSummary;
  if (item.detail) return item.detail;
  if (item.pending) return 'running';
  return item.text;
}

function renderToolDetail(item: TranscriptItem): HTMLElement {
  const wrap = el('div', 'tool-detail-item');
  const header = el('div', 'tool-detail-header');
  header.append(el('span', 'tool-detail-name', item.toolName || item.text));
  if (item.path) {
    header.append(renderFilePathButton(item.path, 'tool-detail-path', item.line, item.column));
  }
  header.append(el('span', 'tool-detail-status', item.pending ? 'running' : 'done'));
  wrap.append(header);

  if (item.detail) wrap.append(el('div', 'tool-detail', item.detail));
  if (item.toolParameters !== undefined && !item.toolResultSummary) {
    const pre = el('pre', 'tool-code');
    pre.innerHTML = highlightLite(json(item.toolParameters));
    wrap.append(pre);
  } else if (item.toolResultSummary) {
    const pre = el('pre', 'tool-code');
    pre.innerHTML = highlightLite(item.toolResultSummary);
    wrap.append(pre);
  }
  return wrap;
}

function renderFilePathButton(
  path: string,
  className = '',
  line?: number,
  column?: number,
): HTMLElement {
  const button = el('button', `link-button file-link ${className}`, path);
  button.type = 'button';
  button.title = line ? `Open ${path}:${line}` : `Open ${path}`;
  button.addEventListener('click', (event) => {
    event.preventDefault();
    event.stopPropagation();
    vscode.postMessage({ type: 'openFile', path, line, column });
  });
  return button;
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
      openLink.type = 'button';
      openLink.addEventListener('click', () =>
        vscode.postMessage({ type: 'openFile', path: item.path as string }),
      );
      wrap.append(openLink);
    }
  } else if (item.parameters !== undefined) {
    const pre = el('pre', 'tool-code');
    pre.innerHTML = highlightLite(json(item.parameters));
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
  approve.type = 'button';
  approve.addEventListener('click', () => {
    vscode.postMessage({
      type: 'approvalRespond',
      approved: true,
      scope: scope.value as 'once' | 'session' | 'command' | 'path',
      toolName: item.toolName,
      reason: item.reason,
      parameters: item.parameters,
      sessionId: item.approvalSessionId,
    });
  });
  const deny = el('button', 'secondary-button compact-button', 'Deny');
  deny.type = 'button';
  deny.addEventListener('click', () => {
    vscode.postMessage({
      type: 'approvalRespond',
      approved: false,
      scope: scope.value as 'once' | 'session' | 'command' | 'path',
      toolName: item.toolName,
      reason: item.reason,
      parameters: item.parameters,
      sessionId: item.approvalSessionId,
    });
  });
  actions.append(scope, approve, deny);
  wrap.append(actions);
  return wrap;
}

function latestPendingApproval(s: SidebarState): TranscriptItem | undefined {
  return s.pendingApproval ?? [...s.transcript].reverse().find((item) => item.role === 'approval');
}

function renderApprovalDock(item: TranscriptItem): HTMLElement {
  const wrap = el('div', 'approval-dock');
  wrap.append(renderApprovalBubble(item));
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
  send.type = 'button';
  send.addEventListener('click', sendAnswer);
  const cancel = el('button', 'secondary-button compact-button', 'Cancel');
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

function renderDiffBlock(item: TranscriptItem): HTMLElement {
  const wrap = el('section', 'msg msg-diff');
  const header = el('div', 'diff-header');
  if (item.path) {
    const openLink = el('button', 'link-button file-link', item.path);
    openLink.type = 'button';
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

function renderCommandBlock(item: TranscriptItem): HTMLElement {
  const result = item.commandResult;
  const wrap = el('section', `command-block ${result?.severity === 'error' ? 'error' : ''}`);
  const header = el('div', 'command-header');
  header.append(el('span', 'command-title', result?.title ?? result?.kind ?? 'Command'));
  if (result?.command) header.append(el('span', 'command-chip', result.command));
  wrap.append(header);
  if (result?.message) {
    wrap.append(el('div', 'command-message', result.message));
  } else if (item.text) {
    wrap.append(el('div', 'command-message', item.text));
  }
  if (result?.source_totals) {
    const totals = el('div', 'command-totals');
    Object.entries(result.source_totals).forEach(([source, tokens]) => {
      totals.append(el('span', 'command-total', `${source} ${formatTokens(tokens)}`));
    });
    wrap.append(totals);
  }
  if (typeof result?.diff === 'string') {
    const pre = el('pre', 'command-code');
    pre.innerHTML = highlightLite(result.diff.trim() || '(no changes)');
    wrap.append(pre);
  }
  if (Array.isArray(result?.items) && result.items.length > 0) {
    const list = el('div', 'command-list');
    result.items.forEach((row) => {
      const line = el('div', 'command-row');
      const label = row.label ?? row.path ?? row.source ?? '';
      if (row.path) {
        line.append(renderFilePathButton(row.path, 'command-path', row.line, row.column));
      } else if (label) {
        line.append(el('span', 'command-row-label', label));
      }
      const meta = [
        typeof row.line === 'number' ? `:${row.line}` : '',
        typeof row.tokens === 'number' ? `${formatTokens(row.tokens)}` : '',
        row.transport,
        typeof row.turn_id === 'number' ? `turn ${row.turn_id}` : '',
      ].filter(Boolean);
      if (meta.length > 0) line.append(el('span', 'command-row-meta', meta.join(' · ')));
      if (row.detail) line.append(el('span', 'command-row-detail', row.detail));
      list.append(line);
    });
    wrap.append(list);
    if (result.truncated) wrap.append(el('div', 'command-footnote', 'further hits truncated'));
  }
  return wrap;
}

function renderBranchPicker(s: SidebarState): HTMLElement {
  const result = s.branchPicker;
  const wrap = el('section', 'branch-picker-panel');
  const header = el('div', 'branch-picker-header');
  header.append(el('div', 'branch-picker-title', result?.title ?? 'Branch Turns'));
  const close = iconButton('remove', 'Close branch picker', () =>
    vscode.postMessage({ type: 'dismissBranchPicker' }),
  );
  header.append(close);
  wrap.append(header);
  if (result?.message) wrap.append(el('div', 'branch-picker-message', result.message));
  const items = Array.isArray(result?.items) ? result.items : [];
  if (items.length === 0) {
    wrap.append(el('div', 'branch-picker-empty', 'No turns available'));
    return wrap;
  }
  const list = el('div', 'branch-picker-list');
  items.forEach((item) => {
    const turnId = item.turn_id;
    const row = el('button', 'branch-picker-row');
    row.type = 'button';
    row.disabled = typeof turnId !== 'number' || s.running;
    row.addEventListener('click', () => {
      if (typeof turnId !== 'number') return;
      vscode.postMessage({ type: 'dismissBranchPicker' });
      vscode.postMessage({
        type: 'run',
        task: `/branch turn ${turnId}`,
        options: currentOptionsFromDom(),
      });
    });
    row.append(el('span', 'branch-picker-row-title', item.label ?? `turn ${turnId ?? '?'}`));
    const meta = [item.source, typeof turnId === 'number' ? `turn ${turnId}` : '']
      .filter(Boolean)
      .join(' · ');
    if (meta) row.append(el('span', 'branch-picker-row-meta', meta));
    if (item.detail) row.append(el('span', 'branch-picker-row-detail', item.detail));
    list.append(row);
  });
  wrap.append(list);
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
  clear.type = 'button';
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
  text.dataset.placeholder = 'Empty prompt — remove or fill it in';
  // Save on blur so users can refine queued prompts without a Save button.
  text.addEventListener('blur', () => {
    const next = text.textContent ?? '';
    if (next.trim() !== item.text.trim()) {
      vscode.postMessage({ type: 'queueEdit', id: item.id, text: next });
    }
  });
  // Strip pasted formatting — only the text content is meaningful here.
  text.addEventListener('paste', (event) => {
    event.preventDefault();
    const plain = event.clipboardData?.getData('text/plain') ?? '';
    document.execCommand('insertText', false, plain);
  });
  text.addEventListener('keydown', (event) => {
    // Enter commits the edit (blur fires save). IME composition bypasses.
    if (event.key === 'Enter' && !event.shiftKey && !event.isComposing) {
      event.preventDefault();
      (event.target as HTMLElement).blur();
    }
    if (event.key === 'Escape') {
      // Cancel — restore original and blur without saving.
      event.preventDefault();
      text.textContent = item.text;
      (event.target as HTMLElement).blur();
    }
  });
  wrap.append(text);

  const actions = el('div', 'queue-actions');
  const remove = el('button', 'icon-button mini', '');
  remove.type = 'button';
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
  optionsRow.append(modelControl(s));
  optionsRow.append(renderContextDock(s));
  wrap.append(optionsRow);

  const slashPicker = el('div', 'slash-picker hidden');
  wrap.append(slashPicker);

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
    updateSlashPicker(textarea, slashPicker);
  });
  textarea.addEventListener('keydown', (event) => {
    if (isSlashPickerOpen(slashPicker)) {
      if (event.key === 'ArrowDown' || event.key === 'ArrowUp') {
        event.preventDefault();
        const matches = filteredSlashCommands(textarea.value);
        if (matches.length > 0) {
          slashPickerSelected = Math.max(
            0,
            Math.min(
              matches.length - 1,
              slashPickerSelected + (event.key === 'ArrowDown' ? 1 : -1),
            ),
          );
          updateSlashPicker(textarea, slashPicker);
        }
        return;
      }
      if (event.key === 'Tab') {
        event.preventDefault();
        acceptSlashSelection(textarea, slashPicker);
        return;
      }
      if (event.key === 'Escape') {
        event.preventDefault();
        slashPicker.classList.add('hidden');
        return;
      }
    }
    if (event.key === 'Enter' && !event.shiftKey && !event.isComposing) {
      event.preventDefault();
      if (isSlashPickerOpen(slashPicker) && !slashExactSelectionIsRunnable(textarea.value)) {
        acceptSlashSelection(textarea, slashPicker);
        return;
      }
      handleSubmit();
    }
  });
  inputRow.append(textarea);

  const button = el('button', `composer-button ${s.running ? 'stop' : 'send'}`);
  button.type = 'button';
  button.title = s.running ? 'Stop current task' : 'Send';
  button.innerHTML = iconSvg(s.running ? 'stop' : 'send');
  const shouldAnimateIconSwap =
    lastComposerRunning !== undefined && lastComposerRunning !== s.running;
  lastComposerRunning = s.running;
  const innerSvg = button.querySelector('svg');
  if (innerSvg && shouldAnimateIconSwap) {
    innerSvg.classList.add('icon-swap');
    setTimeout(() => innerSvg.classList.remove('icon-swap'), 240);
  }
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
  setTimeout(() => {
    autoresize(textarea);
    updateSlashPicker(textarea, slashPicker);
  }, 0);

  function handleSubmit(): void {
    const value = textarea.value.trim();
    if (!value) return;
    pinTranscriptToBottomOnNextRender();
    if (s.running && !value.startsWith('/')) {
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

function filteredSlashCommands(input: string): SlashCommandSpec[] {
  const query = input.trimEnd();
  if (!query.startsWith('/') || query.includes('\n')) return [];
  const needle = query.slice(1).trim().toLowerCase();
  if (needle.length === 0) return slashCommands;
  return slashCommands.filter((command) => {
    const name = command.name.slice(1).toLowerCase();
    const description = command.description.toLowerCase();
    return name.startsWith(needle) || name.includes(` ${needle}`) || description.includes(needle);
  });
}

function updateSlashPicker(textarea: HTMLTextAreaElement, picker: HTMLElement): void {
  const matches = filteredSlashCommands(textarea.value);
  if (matches.length === 0) {
    slashPickerSelected = 0;
    picker.classList.add('hidden');
    picker.replaceChildren();
    return;
  }

  slashPickerSelected = Math.min(slashPickerSelected, matches.length - 1);
  picker.classList.remove('hidden');
  picker.replaceChildren();
  const start = Math.min(
    Math.max(0, slashPickerSelected - 5),
    Math.max(0, matches.length - 6),
  );
  matches.slice(start, start + 6).forEach((command, offset) => {
    const index = start + offset;
    const row = el('button', `slash-option${index === slashPickerSelected ? ' selected' : ''}`);
    row.type = 'button';
    row.addEventListener('mousedown', (event) => {
      event.preventDefault();
      slashPickerSelected = index;
      acceptSlashSelection(textarea, picker);
    });
    const label = command.argHint ? `${command.name} ${command.argHint}` : command.name;
    row.append(el('span', 'slash-name', label));
    row.append(el('span', 'slash-description', command.description));
    picker.append(row);
  });
}

function isSlashPickerOpen(picker: HTMLElement): boolean {
  return !picker.classList.contains('hidden');
}

function acceptSlashSelection(textarea: HTMLTextAreaElement, picker: HTMLElement): void {
  const matches = filteredSlashCommands(textarea.value);
  const command = matches[slashPickerSelected];
  if (!command) return;
  textarea.value = command.argHint ? `${command.name} ${command.argHint}` : command.name;
  textarea.selectionStart = textarea.value.length;
  textarea.selectionEnd = textarea.value.length;
  composerDraft = textarea.value;
  autoresize(textarea);
  updateSlashPicker(textarea, picker);
  textarea.focus();
}

function slashExactSelectionIsRunnable(input: string): boolean {
  const matches = filteredSlashCommands(input);
  const command = matches[slashPickerSelected];
  if (!command) return false;
  return (
    input.trim() === command.name &&
    (!command.argHint || command.argHint.startsWith('['))
  );
}

function modeSelect(opts: RunOptions): HTMLSelectElement {
  const current = composerModeOverride ?? opts.mode;
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
    if (current === value) option.selected = true;
    select.append(option);
  }
  return select;
}

function permissionSelect(opts: RunOptions): HTMLSelectElement {
  const current = composerPermissionOverride ?? opts.permission;
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
    if (current === value) option.selected = true;
    select.append(option);
  }
  return select;
}

function modelControl(s: SidebarState): HTMLInputElement | HTMLSelectElement {
  if (s.context.provider === 'openai-oauth') {
    return chatGptModelSelect(s);
  }
  return modelInput(s.runOptions, s.context.provider);
}

function chatGptModelSelect(s: SidebarState): HTMLSelectElement {
  const configured = composerModelOverride ?? s.runOptions.model ?? s.context.model ?? 'gpt-5.5';
  const current = CHATGPT_MODELS.includes(configured) ? configured : 'gpt-5.5';
  const select = document.createElement('select');
  select.className = 'composer-select composer-model composer-model-select';
  select.id = 'composer-model';
  select.title = 'ChatGPT model';
  for (const model of CHATGPT_MODELS) {
    const option = document.createElement('option');
    option.value = model;
    option.textContent = model;
    if (current === model) option.selected = true;
    select.append(option);
  }
  return select;
}

function modelInput(opts: RunOptions, provider?: string): HTMLInputElement {
  const input = document.createElement('input');
  input.className = 'composer-model';
  input.id = 'composer-model';
  input.placeholder = provider === 'openrouter-api' ? 'openrouter model' : 'model override';
  input.value = composerModelOverride ?? opts.model ?? '';
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
  if (state?.runOptions.reasoningEffort) {
    options.reasoningEffort = state.runOptions.reasoningEffort;
  }
  if (state?.runOptions.serviceTier) {
    options.serviceTier = state.runOptions.serviceTier;
  }
  return options;
}

function autoresize(textarea: HTMLTextAreaElement): void {
  textarea.style.height = 'auto';
  textarea.style.height = `${Math.min(textarea.scrollHeight, 180)}px`;
}
