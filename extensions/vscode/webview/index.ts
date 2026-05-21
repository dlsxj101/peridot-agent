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
let toolHistoryOpen = false;
let slashPickerSelected = 0;

const CHATGPT_MODELS = ['gpt-5.5', 'gpt-5.5-fast', 'gpt-5.4', 'gpt-5.4-mini'];
interface SlashCommandSpec {
  name: string;
  description: string;
  argHint?: string;
}

const SLASH_COMMANDS: SlashCommandSpec[] = [
  { name: '/clear', description: 'Clear transcript and start a fresh session' },
  { name: '/plan', description: 'Switch to plan mode' },
  { name: '/execute', description: 'Switch to execute mode' },
  { name: '/safe', description: 'Use safe permission mode' },
  { name: '/auto', description: 'Use auto permission mode' },
  { name: '/yolo', description: 'Use yolo permission mode' },
  { name: '/model', description: 'Switch the active model', argHint: '<name>' },
  { name: '/session new', description: 'Open a new chat session', argHint: '[task]' },
  { name: '/session list', description: 'List open chat sessions' },
  { name: '/session switch', description: 'Switch to another session', argHint: '<id|title>' },
  { name: '/help', description: 'Show slash command help' },
];

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
  // Preserve composer draft / selection across renders so streaming
  // events don't clobber what the user is typing or picking.
  const textarea = document.getElementById('composer-input') as HTMLTextAreaElement | null;
  if (textarea) composerDraft = textarea.value;
  const modeEl = document.getElementById('composer-mode') as HTMLSelectElement | null;
  if (modeEl) composerModeOverride = modeEl.value;
  const permEl = document.getElementById('composer-permission') as HTMLSelectElement | null;
  if (permEl) composerPermissionOverride = permEl.value;
  const modelEl = document.getElementById('composer-model') as HTMLInputElement | null;
  if (modelEl) composerModelOverride = modelEl.value;
  const transcriptEl = document.querySelector<HTMLElement>('.transcript');
  if (transcriptEl) {
    transcriptPinnedToBottom = isTranscriptAtBottom(transcriptEl);
    transcriptScrollTop = transcriptEl.scrollTop;
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
    img.width = 66;
    img.height = 66;
    left.append(img);
  }
  const titleWrap = el('div', 'header-title-wrap');
  titleWrap.append(el('div', 'header-title', 'Peridot'));
  titleWrap.append(el('div', 'header-status', s.status));
  left.append(titleWrap);

  const right = el('div', 'header-actions');
  if (s.sessions.length > 0) {
    const select = document.createElement('select');
    select.className = 'session-select';
    select.title = 'Open sessions';
    select.setAttribute('aria-label', 'Open sessions');
    s.sessions.forEach((session) => {
      const option = document.createElement('option');
      option.value = session.id;
      option.textContent = `${session.running ? '● ' : ''}${session.title}`;
      option.selected = session.active;
      select.append(option);
    });
    select.addEventListener('change', () =>
      vscode.postMessage({ type: 'selectSession', id: select.value }),
    );
    right.append(select);
  }
  right.append(iconButton('new', 'New session', () => vscode.postMessage({ type: 'newSession' })));
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
  return Boolean(s.hud.usage || s.hud.budget || s.hud.plan || s.hud.committee);
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

  if (hudState.plan && hudState.plan.steps.length > 0) {
    hud.append(renderPlan(hudState.plan));
  }
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
      index -= 1;
      wrap.append(renderToolStack(tools));
    } else {
      wrap.append(renderItem(item));
    }
  }
  requestAnimationFrame(() => {
    if (transcriptPinnedToBottom) {
      wrap.scrollTop = wrap.scrollHeight;
    } else {
      const maxScrollTop = Math.max(0, wrap.scrollHeight - wrap.clientHeight);
      wrap.scrollTop = Math.min(transcriptScrollTop, maxScrollTop);
    }
    transcriptScrollTop = wrap.scrollTop;
  });
  return wrap;
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
  wrap.append(renderMarkdownBody(item.text));
  const copy = el('button', 'copy-button', '');
  copy.type = 'button';
  copy.title = 'Copy response';
  copy.setAttribute('aria-label', 'Copy response');
  copy.innerHTML = iconSvg('copy');
  copy.addEventListener('click', () => copyText(item.text));
  wrap.append(copy);
  return wrap;
}

function copyText(text: string): void {
  if (navigator.clipboard?.writeText) {
    void navigator.clipboard.writeText(text);
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

function renderToolBlock(item: TranscriptItem): HTMLElement {
  return renderToolStack([item]);
}

function renderToolStack(items: TranscriptItem[]): HTMLElement {
  const latest = items[items.length - 1];
  const details = el('details', 'tool-stack');
  details.open = toolHistoryOpen;
  details.addEventListener('toggle', () => {
    toolHistoryOpen = details.open;
  });
  const summary = el('summary', 'tool-summary');
  const name = el('span', 'tool-name', latest.toolName || latest.text);
  const result = el('span', 'tool-result', toolSummaryText(latest));
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

  const history = el('div', 'tool-history');
  items.forEach((item) => history.append(renderToolDetail(item)));
  details.append(history);
  return details;
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
  // Animate the icon swap when the running state flips. The class is
  // applied once, then auto-cleared after the keyframes finish.
  const innerSvg = button.querySelector('svg');
  if (innerSvg) {
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
  if (needle.length === 0) return SLASH_COMMANDS;
  return SLASH_COMMANDS.filter((command) => {
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
  return options;
}

function autoresize(textarea: HTMLTextAreaElement): void {
  textarea.style.height = 'auto';
  textarea.style.height = `${Math.min(textarea.scrollHeight, 180)}px`;
}
