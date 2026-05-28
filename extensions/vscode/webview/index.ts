import './style.css';
import autoAnimate from '@formkit/auto-animate';
import MarkdownIt from 'markdown-it';
import type {
  AttachmentView,
  CompactionDetailItem,
  CompactionSnapshotView,
  CommandResultItem,
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
import {
  ComposerHistory,
  canNavigateComposerHistory,
  type ComposerHistoryDirection,
  type ComposerHistorySnapshot,
} from './composerHistory';
import {
  filteredSlashCommands as filterSlashCommands,
  acceptedSlashCommandText,
  slashArgumentContext as resolveSlashArgumentContext,
  slashExactSelectionIsRunnable as isSlashExactSelectionRunnable,
  slashPickerItemCount as countSlashPickerItems,
  type SlashArgumentContext,
} from './slashAutocomplete';
import { runMetricChips } from './runMetrics';
import { riskChipView } from './riskChip';
import { el, formatTokens, highlightLite, isRecord, json } from './util';

declare function acquireVsCodeApi(): {
  postMessage(msg: OutboundMessage): void;
  setState(state: unknown): void;
  getState(): unknown;
};

const vscode = acquireVsCodeApi();
const root = document.getElementById('app') as HTMLElement;
const mascotUri = root.dataset.mascot ?? '';
const restoredComposerState = readComposerWebviewState(vscode.getState());

// Last state snapshot — used so non-state-driven inputs (like the composer
// textarea while typing) survive a re-render.
let state: SidebarState | undefined;
let composerDraft = '';
const composerDrafts = new Map<string, string>(
  Object.entries(restoredComposerState.composerDrafts ?? {}),
);
const composerHistory = new ComposerHistory(restoredComposerState.composerHistory);
let appliedComposerDraftVersion = 0;
// Pending composer selections — captured pre-render so a state update
// triggered by a streaming event doesn't reset the user's mid-edit
// mode / permission / model picks back to whatever was last submitted.
let composerModeOverride: string | undefined;
let composerPermissionOverride: string | undefined;
let composerModelOverride: string | undefined;
let transcriptPinnedToBottom = true;
let forceTranscriptBottomOnce = false;
let transcriptScrollRestoreToken = 0;
let lastTranscriptAnimationKey = '';
let lastTranscriptCount = 0;
let lastComposerRunning: boolean | undefined;
let editingSessionId: string | undefined;
let editingSessionDraft: string | undefined;
let editingSessionSelectOnFocus: string | undefined;
let deletingSessionId: string | undefined;
let sessionMenuOpen = false;
interface AssistantStreamSnapshot {
  markdown: string;
  visibleText: string;
}
const assistantTextByKey = new Map<string, AssistantStreamSnapshot>();
let lastComposerSessionKey = '';
let slashPickerSelected = 0;
let slashCommands: SlashCommandSpec[] = [];
let todoExpanded = false;
let lastTodoCurrentKey = '';
let lastRenderedState: SidebarState | undefined;
const toolNameSwapTimers = new WeakMap<HTMLElement, number>();
let runFooterTimer: number | undefined;

const CHATGPT_MODELS = ['gpt-5.5', 'gpt-5.5-fast', 'gpt-5.4', 'gpt-5.4-mini'];
const APPROVAL_SCOPE_OPTIONS = [
  ['once', 'Once'],
  ['command', 'Command'],
  ['path', 'Path'],
  ['session', 'Session'],
] as const;
type SelectOption<Value extends string> = readonly [value: Value, label: string];
type ApprovalScopeValue = (typeof APPROVAL_SCOPE_OPTIONS)[number][0];

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

// Close session menu when clicking outside it
document.addEventListener('click', (event) => {
  if (!sessionMenuOpen) return;
  const menu = document.querySelector('.session-menu');
  if (menu && !menu.contains(event.target as Node)) {
    editingSessionId = undefined;
    editingSessionDraft = undefined;
    editingSessionSelectOnFocus = undefined;
    deletingSessionId = undefined;
    sessionMenuOpen = false;
    if (menu instanceof HTMLDetailsElement) menu.open = false;
    if (state) render(state);
  }
});

function render(s: SidebarState): void {
  slashCommands = s.slashCommands;
  const composerSessionKey = s.view === 'session' ? s.activeChatId ?? s.sessionId ?? 'draft' : s.view;
  const previousComposerSessionKey = lastComposerSessionKey;
  const composerSessionChanged = composerSessionKey !== lastComposerSessionKey;
  const textarea = document.getElementById('composer-input') as HTMLTextAreaElement | null;
  if (composerSessionChanged && textarea && previousComposerSessionKey) {
    composerDrafts.set(previousComposerSessionKey, textarea.value);
    persistComposerWebviewState();
  }
  if (composerSessionChanged) {
    composerModeOverride = undefined;
    composerPermissionOverride = undefined;
    composerModelOverride = undefined;
    lastComposerSessionKey = composerSessionKey;
  }
  // Preserve composer draft / selection across renders so streaming
  // events don't clobber what the user is typing or picking.
  const requestedDraftVersion = s.composerDraftVersion ?? 0;
  if (requestedDraftVersion > appliedComposerDraftVersion) {
    composerDraft = s.composerDraft ?? '';
    composerDrafts.set(composerSessionKey, composerDraft);
    appliedComposerDraftVersion = requestedDraftVersion;
    persistComposerWebviewState();
  } else if (textarea && !composerSessionChanged) {
    composerDraft = textarea.value;
    composerDrafts.set(composerSessionKey, composerDraft);
    persistComposerWebviewState();
  } else {
    composerDraft = composerDrafts.get(composerSessionKey) ?? '';
  }
  const modeEl = document.getElementById('composer-mode') as HTMLSelectElement | null;
  if (modeEl && !composerSessionChanged) composerModeOverride = modeEl.value;
  const permEl = document.getElementById('composer-permission') as HTMLSelectElement | null;
  if (permEl && !composerSessionChanged) composerPermissionOverride = permEl.value;
  const modelEl = document.getElementById('composer-model') as HTMLInputElement | null;
  if (modelEl && !composerSessionChanged) composerModelOverride = modelEl.value;
  const transcriptEl = document.querySelector<HTMLElement>('.transcript');
  if (transcriptEl && !forceTranscriptBottomOnce) {
    if (isTranscriptAtBottom(transcriptEl)) transcriptPinnedToBottom = true;
  } else if (forceTranscriptBottomOnce) {
    transcriptPinnedToBottom = true;
  }
  // Remember which element had focus so we can re-focus after the
  // destructive replaceChildren below.
  const focusId = (document.activeElement && (document.activeElement as HTMLElement).id) || '';
  const activeRenameInput =
    document.activeElement instanceof HTMLInputElement &&
    document.activeElement.classList.contains('session-menu-rename-input')
      ? document.activeElement
      : undefined;
  const renameSelection = activeRenameInput
    ? {
        id: activeRenameInput.dataset.sessionId,
        start: activeRenameInput.selectionStart ?? activeRenameInput.value.length,
        end: activeRenameInput.selectionEnd ?? activeRenameInput.value.length,
      }
    : undefined;

  const currentSession = root.firstElementChild;
  if (s.view === 'session' && currentSession instanceof HTMLElement && currentSession.classList.contains('session')) {
    updateSession(currentSession, s);
  } else {
    root.replaceChildren(s.view === 'landing' ? renderLanding(s) : renderSession(s));
  }
  lastRenderedState = s;
  syncRunFooterTimer(s);

  const newTextarea = document.getElementById('composer-input') as HTMLTextAreaElement | null;
  if (newTextarea) newTextarea.value = composerDraft;
  if (focusId) {
    const target = document.getElementById(focusId) as HTMLElement | null;
    target?.focus({ preventScroll: true });
  }
  if (editingSessionId) {
    const input = document.getElementById(`session-rename-${editingSessionId}`) as HTMLInputElement | null;
    if (input) {
      input.focus({ preventScroll: true });
      if (editingSessionSelectOnFocus === editingSessionId) {
        input.select();
        editingSessionSelectOnFocus = undefined;
      } else if (renameSelection?.id === editingSessionId) {
        input.setSelectionRange(
          Math.min(renameSelection.start, input.value.length),
          Math.min(renameSelection.end, input.value.length),
        );
      }
    }
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
  }
}

function bindTranscriptScrollTracking(wrap: HTMLElement): void {
  if (wrap.dataset.scrollTracking === 'true') return;
  wrap.dataset.scrollTracking = 'true';
  // Cline-style scroll lock. Two rules, that's it:
  //   - User wheels up → unpin.
  //   - Scroll reaches bottom (by any means) → re-pin.
  // The wheel handler fires synchronously on user input, so we don't need to
  // disambiguate user vs. programmatic motion in the scroll event — by the
  // time the scroll event arrives, pinned is already false if the user wanted
  // to scroll up.
  wrap.addEventListener(
    'wheel',
    (event: WheelEvent) => {
      if (event.deltaY < 0) transcriptPinnedToBottom = false;
    },
    { passive: true },
  );
  wrap.addEventListener(
    'scroll',
    () => {
      if (isTranscriptAtBottom(wrap)) transcriptPinnedToBottom = true;
    },
    { passive: true },
  );
}

function bindToolHistoryMotion(wrap: HTMLElement): void {
  if (wrap.dataset.motionBound === 'true') return;
  wrap.dataset.motionBound = 'true';
  autoAnimate(wrap, {
    duration: 160,
    easing: 'cubic-bezier(0.2, 0.75, 0.2, 1)',
  });
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
  updateSession(wrap, s);
  return wrap;
}

function updateSession(wrap: HTMLElement, s: SidebarState): void {
  let transcript = wrap.querySelector<HTMLElement>('.transcript');
  if (!transcript) {
    transcript = el('main', 'transcript');
  }
  const transcriptScroll = renderTranscriptInto(transcript, s);

  const children: HTMLElement[] = [];
  children.push(sessionSlot('header', renderHeader(s)));
  children.push(sessionSlot('context', renderContextStrip(s.context)));
  if (s.hud.plan && s.hud.plan.steps.length > 0) {
    children.push(sessionSlot('todo', renderTodoProgress(s.hud.plan, s.running)));
  }
  children.push(sessionSlot('transcript', transcript));
  children.push(sessionSlot('queue', renderQueue(s)));
  if (s.branchPicker) children.push(sessionSlot('branch-picker', renderBranchPicker(s)));
  const prompt = latestPendingPrompt(s);
  if (prompt) children.push(sessionSlot('prompt', renderPromptDock(prompt)));
  const runFooter = renderRunFooter(s);
  if (runFooter) children.push(sessionSlot('run-footer', runFooter));
  children.push(sessionSlot('composer', renderComposer(s)));
  reconcileSessionChildren(wrap, children);
  scheduleTranscriptScroll(transcriptScroll.wrap, transcriptScroll.mode, transcriptScroll.previousScrollTop);
}

function renderRunFooter(s: SidebarState): HTMLElement | undefined {
  const running = Boolean(s.running && typeof s.runStartedAtMs === 'number');
  if (!running) return undefined;

  const footer = el('div', 'run-footer running');
  footer.setAttribute('role', 'status');
  footer.dataset.runStart = String(s.runStartedAtMs);

  const gem = el('span', 'peridot-loader', '◆');
  gem.setAttribute('aria-hidden', 'true');
  footer.append(gem);

  const label = el('span', 'run-footer-text');
  label.append(document.createTextNode('Peridot is working · '));
  label.append(el('span', 'run-footer-time', formatElapsed(Date.now() - (s.runStartedAtMs ?? Date.now()))));
  footer.append(label);
  return footer;
}

function syncRunFooterTimer(s: SidebarState): void {
  const shouldTick = s.view === 'session' && Boolean(s.running && typeof s.runStartedAtMs === 'number');
  if (!shouldTick) {
    if (runFooterTimer !== undefined) {
      window.clearInterval(runFooterTimer);
      runFooterTimer = undefined;
    }
    return;
  }
  updateRunFooterClock();
  if (runFooterTimer !== undefined) return;
  runFooterTimer = window.setInterval(updateRunFooterClock, 1000);
}

function updateRunFooterClock(): void {
  const footer = document.querySelector<HTMLElement>('.run-footer[data-run-start]');
  const time = footer?.querySelector<HTMLElement>('.run-footer-time');
  const startedAt = Number(footer?.dataset.runStart);
  if (!footer || !time || !Number.isFinite(startedAt)) return;
  time.textContent = formatElapsed(Date.now() - startedAt);
}

function formatElapsed(ms: number): string {
  const totalSeconds = Math.max(0, Math.floor(ms / 1000));
  const seconds = totalSeconds % 60;
  const minutes = Math.floor(totalSeconds / 60) % 60;
  const hours = Math.floor(totalSeconds / 3600);
  const two = (value: number) => String(value).padStart(2, '0');
  if (hours > 0) return `${hours}:${two(minutes)}:${two(seconds)}`;
  return `${minutes}:${two(seconds)}`;
}

function sessionSlot(name: string, node: HTMLElement): HTMLElement {
  node.dataset.sessionSlot = name;
  return node;
}

function reconcileSessionChildren(wrap: HTMLElement, nextChildren: HTMLElement[]): void {
  const existing = new Map<string, HTMLElement>();
  Array.from(wrap.children).forEach((child) => {
    if (child instanceof HTMLElement && child.dataset.sessionSlot) {
      existing.set(child.dataset.sessionSlot, child);
    }
  });
  const reconciled = nextChildren.map((next) => {
    if (next.dataset.sessionSlot === 'transcript') return next;
    const previous = next.dataset.sessionSlot ? existing.get(next.dataset.sessionSlot) : undefined;
    return previous && previous.outerHTML === next.outerHTML ? previous : next;
  });

  let cursor: ChildNode | null = wrap.firstChild;
  for (const node of reconciled) {
    if (node !== cursor) {
      wrap.insertBefore(node, cursor);
    }
    cursor = node.nextSibling;
    if (node.dataset.sessionSlot) existing.delete(node.dataset.sessionSlot);
  }
  for (const stale of existing.values()) {
    stale.remove();
  }
  while (cursor) {
    const next = cursor.nextSibling;
    cursor.remove();
    cursor = next;
  }
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
  if (s.phase) {
    const chip = el('span', `phase-chip phase-${phaseColor(s.phase)}`, s.phase);
    titleWrap.append(chip);
  }
  left.append(titleWrap);

  const right = el('div', 'header-actions');
  right.append(renderSessionMenu(s));
  right.append(iconButton('codemap', 'Workspace Code Map', () => vscode.postMessage({ type: 'showCodeMap' })));
  right.append(iconButton('info', 'Workspace Code Map Status', () => vscode.postMessage({ type: 'showCodeMapStatus' })));
  right.append(iconButton('search', 'Search Workspace Code Map', () => vscode.postMessage({ type: 'searchCodeMap' })));
  right.append(iconButton('list-tree', 'Outline Current File', () => vscode.postMessage({ type: 'outlineCurrentFile' })));
  right.append(iconButton('references', 'Find Symbol References', () => vscode.postMessage({ type: 'findSymbolReferences' })));
  right.append(iconButton('skills', 'Show Skills', () => vscode.postMessage({ type: 'showSkills' })));
  right.append(iconButton('archive', 'Show Archived Skills', () => vscode.postMessage({ type: 'showArchivedSkills' })));
  right.append(iconButton('search', 'Search Skills', () => vscode.postMessage({ type: 'searchSkills' })));
  right.append(iconButton('search-archive', 'Search Archived Skills', () => vscode.postMessage({ type: 'searchArchivedSkills' })));
  right.append(iconButton('attach', 'Attach File', () => vscode.postMessage({ type: 'attachFile' })));
  right.append(iconButton('session-new', 'New Session', () => vscode.postMessage({ type: 'newPersistedSession' })));
  right.append(iconButton('session-switch', 'Switch Session', () => vscode.postMessage({ type: 'switchPersistedSession' })));
  right.append(iconButton('session-close', 'Close Session', () => vscode.postMessage({ type: 'closePersistedSession' })));
  right.append(iconButton('session-count', 'Show Session Count', () => vscode.postMessage({ type: 'showSessionCount' })));
  right.append(iconButton('session-detail', 'Show Session Details', () => vscode.postMessage({ type: 'showPersistedSessionDetails' })));
  right.append(iconButton('session-locate', 'Locate Session Directory', () => vscode.postMessage({ type: 'locatePersistedSessionDirectory' })));
  right.append(iconButton('session-resume', 'Resume Session', () => vscode.postMessage({ type: 'resumePersistedSession' })));
  right.append(iconButton('session-rename', 'Rename Session', () => vscode.postMessage({ type: 'renamePersistedSession' })));
  right.append(iconButton('session-delete', 'Delete Session', () => vscode.postMessage({ type: 'deletePersistedSession' })));
  right.append(iconButton('sessions', 'Show Sessions', () => vscode.postMessage({ type: 'showSessions' })));
  right.append(iconButton('session-search', 'Search Sessions', () => vscode.postMessage({ type: 'searchSessions' })));
  right.append(iconButton('trash', 'Prune Sessions', () => vscode.postMessage({ type: 'pruneSessions' })));
  right.append(iconButton('history', 'Replay Session Timeline', () => vscode.postMessage({ type: 'replaySessionTimeline' })));
  right.append(iconButton('export', 'Export Session Artifacts', () => vscode.postMessage({ type: 'exportSessionArtifacts' })));
  right.append(iconButton('import', 'Import Session Artifacts', () => vscode.postMessage({ type: 'importSessionArtifacts' })));
  right.append(iconButton('pr', 'GitHub PR Status', () => vscode.postMessage({ type: 'showPrStatus' })));
  right.append(iconButton('ship', 'Ship Changes to PR', () => vscode.postMessage({ type: 'shipChanges' })));
  right.append(iconButton('merge', 'Merge GitHub PR', () => vscode.postMessage({ type: 'mergePr' })));
  right.append(iconButton('refresh', 'Refresh', () => vscode.postMessage({ type: 'refreshStatus' })));
  right.append(
    iconButton('switch', 'Switch provider', () =>
      vscode.postMessage({ type: 'showLanding', screen: 'home' }),
    ),
  );
  right.append(iconButton('gear', 'Settings', () => vscode.postMessage({ type: 'openSettings' })));
  header.append(left, right);
  return header;
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
  details.open = sessionMenuOpen;
  details.addEventListener('toggle', () => {
    sessionMenuOpen = details.open;
  });

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
    const row = el('div', `session-menu-row ${session.active ? 'active' : ''}`);
    const isEditing = editingSessionId === session.id;
    const item = isEditing
      ? el('div', `session-menu-item editing ${session.active ? 'active' : ''}`)
      : el('button', `session-menu-item ${session.active ? 'active' : ''}`);
    if (item instanceof HTMLButtonElement) {
      item.type = 'button';
      item.disabled = session.active;
    }
    const marker = el('span', `session-menu-marker ${session.running ? 'running' : ''}`);
    marker.textContent = session.active ? '✓' : session.running ? '●' : '';
    const text = el('span', 'session-menu-text');
    if (isEditing) {
      const input = document.createElement('input');
      input.id = `session-rename-${session.id}`;
      input.className = 'session-menu-rename-input';
      input.dataset.sessionId = session.id;
      input.value = editingSessionDraft ?? session.title;
      input.setAttribute('aria-label', `New title for ${session.title}`);
      input.addEventListener('click', (event) => {
        event.preventDefault();
        event.stopPropagation();
      });
      input.addEventListener('input', () => {
        editingSessionDraft = input.value;
      });
      input.addEventListener('keydown', (event) => {
        if (event.key === 'Enter') {
          event.preventDefault();
          event.stopPropagation();
          commitSessionRename(session.id, input.value);
        } else if (event.key === 'Escape') {
          event.preventDefault();
          event.stopPropagation();
          cancelSessionRename();
        }
      });
      text.append(input);
    } else {
      text.append(el('span', 'session-menu-title', session.title));
    }
    text.append(el('span', 'session-menu-subtitle', session.running ? 'In progress' : session.status));
    item.append(marker, text);
    item.addEventListener('click', () => {
      if (isEditing) return;
      composerDraft = '';
      vscode.postMessage({ type: 'selectSession', id: session.id });
    });
    const actions = el('span', 'session-menu-actions');
    if (isEditing) {
      const save = sessionMenuAction('check', `Save ${session.title}`, () =>
        commitSessionRename(session.id, editingSessionDraft ?? session.title),
      );
      const cancel = sessionMenuAction('remove', 'Cancel rename', cancelSessionRename);
      actions.append(save, cancel);
      actions.classList.add('editing');
    } else if (deletingSessionId === session.id) {
      const confirm = sessionMenuAction('check', 'Confirm delete', () => {
        deletingSessionId = undefined;
        editingSessionId = undefined;
        editingSessionDraft = undefined;
        editingSessionSelectOnFocus = undefined;
        vscode.postMessage({ type: 'deleteSession', id: session.id });
      });
      confirm.classList.add('session-menu-confirm-delete');
      const cancelDel = sessionMenuAction('remove', 'Cancel delete', () => {
        deletingSessionId = undefined;
        render(state ?? s);
      });
      const confirmLabel = el('span', 'session-menu-confirm-label', 'Delete?');
      actions.append(confirmLabel, confirm, cancelDel);
      actions.classList.add('confirming');
    } else {
      const rename = sessionMenuAction('edit', `Rename ${session.title}`, () => {
        editingSessionId = session.id;
        editingSessionDraft = session.title;
        editingSessionSelectOnFocus = session.id;
        deletingSessionId = undefined;
        sessionMenuOpen = true;
        render(state ?? s);
      });
      const remove = sessionMenuAction('trash', `Delete ${session.title}`, () => {
        deletingSessionId = session.id;
        sessionMenuOpen = true;
        render(state ?? s);
      });
      remove.classList.add('session-menu-delete');
      actions.append(rename, remove);
    }
    row.append(item, actions);
    menu.append(row);
  }

  details.append(menu);
  return details;
}

function sessionMenuAction(kind: string, label: string, onClick: () => void): HTMLElement {
  const button = el('button', 'session-menu-action');
  button.type = 'button';
  button.title = label;
  button.setAttribute('aria-label', label);
  button.innerHTML = iconSvg(kind);
  button.addEventListener('click', (event) => {
    event.preventDefault();
    event.stopPropagation();
    onClick();
  });
  return button;
}

function commitSessionRename(id: string, title: string): void {
  const trimmed = title.trim();
  if (!trimmed) return;
  editingSessionId = undefined;
  editingSessionDraft = undefined;
  editingSessionSelectOnFocus = undefined;
  sessionMenuOpen = true;
  vscode.postMessage({ type: 'renameSession', id, title: trimmed });
}

function cancelSessionRename(): void {
  editingSessionId = undefined;
  editingSessionDraft = undefined;
  editingSessionSelectOnFocus = undefined;
  deletingSessionId = undefined;
  sessionMenuOpen = true;
  if (state) render(state);
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
    case 'open':
      return `<svg viewBox="0 0 16 16" width="13" height="13" fill="none" stroke="currentColor" stroke-width="1.4" stroke-linecap="round" stroke-linejoin="round"><path d="M5.5 3.5H3.2A1.7 1.7 0 0 0 1.5 5.2v7.6a1.7 1.7 0 0 0 1.7 1.7h7.6a1.7 1.7 0 0 0 1.7-1.7v-2.3"/><path d="M8.5 1.5h6v6"/><path d="M7.5 8.5l7-7"/></svg>`;
    case 'check':
      return `<svg viewBox="0 0 16 16" width="13" height="13" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round"><path d="M3 8.5l3 3L13 4"/></svg>`;
    case 'edit':
      return `<svg viewBox="0 0 16 16" width="12" height="12" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"><path d="M3 13l1-3 7-7 2 2-7 7-3 1z"/></svg>`;
    case 'trash':
      return `<svg viewBox="0 0 16 16" width="12" height="12" fill="none" stroke="currentColor" stroke-width="1.4" stroke-linecap="round" stroke-linejoin="round"><path d="M3 4h10"/><path d="M6 4V2.8h4V4"/><path d="M5 6l.4 7h5.2L11 6"/><path d="M7 7.5v3.5M9 7.5v3.5"/></svg>`;
    case 'gear':
      return `<svg viewBox="0 0 16 16" width="14" height="14" fill="none" stroke="currentColor" stroke-width="1.3" stroke-linecap="round" stroke-linejoin="round"><circle cx="8" cy="8" r="2.2"/><path d="M8 1.5v1.3M8 13.2v1.3M3.4 3.4l.9.9M11.7 11.7l.9.9M1.5 8h1.3M13.2 8h1.3M3.4 12.6l.9-.9M11.7 4.3l.9-.9"/></svg>`;
    case 'codemap':
      return `<svg viewBox="0 0 16 16" width="14" height="14" fill="none" stroke="currentColor" stroke-width="1.3" stroke-linecap="round" stroke-linejoin="round"><path d="M2.5 4.5h4l1.5 2h5.5v6.5h-11z"/><path d="M4.5 9h7"/><path d="M4.5 11h4"/><path d="M5 2.5h5"/></svg>`;
    case 'skills':
      return `<svg viewBox="0 0 16 16" width="14" height="14" fill="none" stroke="currentColor" stroke-width="1.3" stroke-linecap="round" stroke-linejoin="round"><path d="M3 3.5h10"/><path d="M3 8h10"/><path d="M3 12.5h10"/><path d="M5.5 2.2v2.6"/><path d="M9.5 6.7v2.6"/><path d="M6.8 11.2v2.6"/></svg>`;
    case 'search':
      return `<svg viewBox="0 0 16 16" width="13" height="13" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round"><circle cx="7" cy="7" r="4.2"/><path d="M10.2 10.2 14 14"/></svg>`;
    case 'search-archive':
      return `<svg viewBox="0 0 16 16" width="13" height="13" fill="none" stroke="currentColor" stroke-width="1.4" stroke-linecap="round" stroke-linejoin="round"><path d="M2.5 4.5h7.4v6.8h-7.4z"/><path d="M1.8 2.5h8.8v2h-8.8z"/><path d="M4.7 7.3h3"/><circle cx="11" cy="11" r="2.1"/><path d="M12.5 12.5 14 14"/></svg>`;
    case 'pin':
      return `<svg viewBox="0 0 16 16" width="13" height="13" fill="none" stroke="currentColor" stroke-width="1.4" stroke-linecap="round" stroke-linejoin="round"><path d="M5.5 2.5h5l-1 4 2.5 2.5H4l2.5-2.5z"/><path d="M8 9v4.5"/></svg>`;
    case 'unpin':
      return `<svg viewBox="0 0 16 16" width="13" height="13" fill="none" stroke="currentColor" stroke-width="1.4" stroke-linecap="round" stroke-linejoin="round"><path d="M5.5 2.5h5l-1 4 2.5 2.5H7"/><path d="M8 9v4.5"/><path d="M2.5 2.5l11 11"/></svg>`;
    case 'archive':
      return `<svg viewBox="0 0 16 16" width="13" height="13" fill="none" stroke="currentColor" stroke-width="1.4" stroke-linecap="round" stroke-linejoin="round"><path d="M2.5 4.5h11v9h-11z"/><path d="M1.8 2.5h12.4v2h-12.4z"/><path d="M6 8h4"/></svg>`;
    case 'restore':
      return `<svg viewBox="0 0 16 16" width="13" height="13" fill="none" stroke="currentColor" stroke-width="1.4" stroke-linecap="round" stroke-linejoin="round"><path d="M2.5 4.5h11v9h-11z"/><path d="M1.8 2.5h12.4v2h-12.4z"/><path d="M8 11V7"/><path d="M5.8 9.2 8 7l2.2 2.2"/></svg>`;
    case 'attach':
      return `<svg viewBox="0 0 16 16" width="14" height="14" fill="none" stroke="currentColor" stroke-width="1.4" stroke-linecap="round" stroke-linejoin="round"><path d="M5.2 8.7l3.9-3.9a2.4 2.4 0 0 1 3.4 3.4l-5.1 5.1a3.6 3.6 0 0 1-5.1-5.1l5.5-5.5"/><path d="M6.3 9.8l4.2-4.2"/></svg>`;
    case 'session-new':
      return `<svg viewBox="0 0 16 16" width="14" height="14" fill="none" stroke="currentColor" stroke-width="1.4" stroke-linecap="round"><rect x="2.5" y="3" width="11" height="10" rx="1.5"/><path d="M8 5.5v5M5.5 8h5"/></svg>`;
    case 'session-switch':
      return `<svg viewBox="0 0 16 16" width="14" height="14" fill="none" stroke="currentColor" stroke-width="1.4" stroke-linecap="round" stroke-linejoin="round"><path d="M3 5.5h8"/><path d="M8.5 3 11 5.5 8.5 8"/><path d="M13 10.5H5"/><path d="M7.5 8 5 10.5 7.5 13"/></svg>`;
    case 'session-close':
      return `<svg viewBox="0 0 16 16" width="14" height="14" fill="none" stroke="currentColor" stroke-width="1.4" stroke-linecap="round" stroke-linejoin="round"><rect x="2.5" y="3" width="11" height="10" rx="1.5"/><path d="M5.5 6.5l5 5M10.5 6.5l-5 5"/></svg>`;
    case 'session-count':
      return `<svg viewBox="0 0 16 16" width="14" height="14" fill="none" stroke="currentColor" stroke-width="1.4" stroke-linecap="round" stroke-linejoin="round"><rect x="2.5" y="3" width="11" height="10" rx="1.5"/><path d="M5 6h6"/><path d="M5 8.5h6"/><path d="M5 11h3"/></svg>`;
    case 'session-detail':
      return `<svg viewBox="0 0 16 16" width="14" height="14" fill="none" stroke="currentColor" stroke-width="1.4" stroke-linecap="round" stroke-linejoin="round"><rect x="2.5" y="3" width="11" height="10" rx="1.5"/><path d="M5 6h6"/><path d="M5 8.5h4"/><circle cx="11" cy="11" r="1"/></svg>`;
    case 'session-locate':
      return `<svg viewBox="0 0 16 16" width="14" height="14" fill="none" stroke="currentColor" stroke-width="1.4" stroke-linecap="round" stroke-linejoin="round"><path d="M8 14s4.5-4.2 4.5-7.2A4.5 4.5 0 0 0 3.5 6.8C3.5 9.8 8 14 8 14z"/><circle cx="8" cy="6.8" r="1.6"/></svg>`;
    case 'session-resume':
      return `<svg viewBox="0 0 16 16" width="14" height="14" fill="none" stroke="currentColor" stroke-width="1.4" stroke-linecap="round" stroke-linejoin="round"><path d="M3 8a5 5 0 1 0 1.5-3.6"/><path d="M3 2.5v3.2h3.2"/><path d="M7 5.4 10.6 8 7 10.6z"/></svg>`;
    case 'session-rename':
      return `<svg viewBox="0 0 16 16" width="14" height="14" fill="none" stroke="currentColor" stroke-width="1.4" stroke-linecap="round" stroke-linejoin="round"><rect x="2.5" y="3" width="11" height="10" rx="1.5"/><path d="M5 6h5"/><path d="M5 9h3"/><path d="M9.5 11.5 13 8l1 1-3.5 3.5-1.5.5z"/></svg>`;
    case 'session-delete':
      return `<svg viewBox="0 0 16 16" width="14" height="14" fill="none" stroke="currentColor" stroke-width="1.4" stroke-linecap="round" stroke-linejoin="round"><rect x="2.5" y="3" width="11" height="10" rx="1.5"/><path d="M5 6h6"/><path d="M6 9l4 4M10 9l-4 4"/></svg>`;
    case 'sessions':
      return `<svg viewBox="0 0 16 16" width="14" height="14" fill="none" stroke="currentColor" stroke-width="1.4" stroke-linecap="round" stroke-linejoin="round"><rect x="2.5" y="3" width="11" height="2.8" rx="1"/><rect x="2.5" y="6.6" width="11" height="2.8" rx="1"/><rect x="2.5" y="10.2" width="11" height="2.8" rx="1"/></svg>`;
    case 'session-search':
      return `<svg viewBox="0 0 16 16" width="14" height="14" fill="none" stroke="currentColor" stroke-width="1.4" stroke-linecap="round" stroke-linejoin="round"><rect x="2.4" y="3" width="8" height="2.7" rx="1"/><rect x="2.4" y="6.5" width="8" height="2.7" rx="1"/><circle cx="10.5" cy="10.5" r="2.3"/><path d="M12.2 12.2 14 14"/></svg>`;
    case 'history':
      return `<svg viewBox="0 0 16 16" width="14" height="14" fill="none" stroke="currentColor" stroke-width="1.4" stroke-linecap="round" stroke-linejoin="round"><path d="M3.2 5.5A5.3 5.3 0 1 1 2.9 11"/><path d="M3.2 2.8v2.7h2.7"/><path d="M8 5.2V8l2.2 1.4"/></svg>`;
    case 'export':
      return `<svg viewBox="0 0 16 16" width="14" height="14" fill="none" stroke="currentColor" stroke-width="1.4" stroke-linecap="round" stroke-linejoin="round"><path d="M3 8.5v4h10v-4"/><path d="M8 2.5v7"/><path d="M5.2 5.6L8 2.8l2.8 2.8"/></svg>`;
    case 'import':
      return `<svg viewBox="0 0 16 16" width="14" height="14" fill="none" stroke="currentColor" stroke-width="1.4" stroke-linecap="round" stroke-linejoin="round"><path d="M3 8.5v4h10v-4"/><path d="M8 2.5v7"/><path d="M5.2 6.4 8 9.2l2.8-2.8"/></svg>`;
    case 'pr':
      return `<svg viewBox="0 0 16 16" width="14" height="14" fill="none" stroke="currentColor" stroke-width="1.4" stroke-linecap="round" stroke-linejoin="round"><circle cx="4" cy="4" r="1.6"/><circle cx="12" cy="12" r="1.6"/><path d="M4 5.8V12"/><path d="M12 10.2V8.8A4.8 4.8 0 0 0 7.2 4H6"/></svg>`;
    case 'ship':
      return `<svg viewBox="0 0 16 16" width="14" height="14" fill="none" stroke="currentColor" stroke-width="1.4" stroke-linecap="round" stroke-linejoin="round"><path d="M8 2.5v8"/><path d="M4.8 6.1L8 2.8l3.2 3.3"/><path d="M3 10.5v2.8h10v-2.8"/></svg>`;
    case 'merge':
      return `<svg viewBox="0 0 16 16" width="14" height="14" fill="none" stroke="currentColor" stroke-width="1.4" stroke-linecap="round" stroke-linejoin="round"><circle cx="4" cy="4" r="1.5"/><circle cx="4" cy="12" r="1.5"/><circle cx="12" cy="12" r="1.5"/><path d="M4 5.5v5"/><path d="M5.5 12H10"/><path d="M7 4a5 5 0 0 0 5 5v1"/></svg>`;
    default:
      return '';
  }
}

function phaseColor(phase: string): string {
  const lower = phase.toLowerCase();
  if (lower === 'recovering') return 'amber';
  if (lower === 'delegating') return 'blue';
  if (lower === 'done') return 'green';
  if (lower === 'checking') return 'blue';
  return 'gray';
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
  if (context.committeeMode && context.committeeMode !== 'off') {
    pills.append(pill(`committee ${context.committeeMode}`, 'mode'));
  }
  if (context.agents && (context.agents.ruleCount > 0 || context.agents.paths.length > 0)) {
    const agentsPill = pill(`AGENTS ${context.agents.ruleCount}`, 'mute');
    if (context.agents.paths.length > 0) {
      agentsPill.title = context.agents.paths.join('\n');
    }
    pills.append(agentsPill);
  }
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



function renderContextDock(s: SidebarState): HTMLElement {
  const context = s.hud.context;
  const dock = el('div', 'context-dock');
  if (!context || context.threshold <= 0) return dock;
  const pct = Math.min(1, context.tokensUsed / context.threshold);
  const pctText = `${Math.round(pct * 100)}%`;
  const exact = `${context.tokensUsed.toLocaleString()} / ${context.threshold.toLocaleString()} tokens`;
  const breakdown = [
    typeof context.contextTokens === 'number' ? `stored ${context.contextTokens.toLocaleString()}` : undefined,
    typeof context.messageTokens === 'number' ? `msg ${context.messageTokens.toLocaleString()}` : undefined,
    typeof context.systemTokens === 'number' ? `sys ${context.systemTokens.toLocaleString()}` : undefined,
    typeof context.toolSchemaTokens === 'number' ? `tools ${context.toolSchemaTokens.toLocaleString()}` : undefined,
    typeof context.overheadTokens === 'number' ? `wire ${context.overheadTokens.toLocaleString()}` : undefined,
  ].filter(Boolean).join(' · ');
  const donut = el('div', 'context-donut');
  const circumference = 62.832;
  donut.style.setProperty('--context-dash', `${(circumference * pct).toFixed(2)}`);
  donut.style.setProperty('--context-circ', `${circumference}`);
  if (pct >= 0.9) donut.classList.add('critical');
  else if (pct >= 0.75) donut.classList.add('warn');
  donut.tabIndex = 0;
  donut.setAttribute('role', 'img');
  donut.setAttribute('aria-label', `Request context ${exact} (${pctText})`);
  const svg = document.createElementNS('http://www.w3.org/2000/svg', 'svg');
  svg.setAttribute('class', 'context-ring');
  svg.setAttribute('viewBox', '0 0 24 24');
  svg.setAttribute('aria-hidden', 'true');
  const track = document.createElementNS('http://www.w3.org/2000/svg', 'circle');
  track.setAttribute('class', 'context-ring-track');
  track.setAttribute('cx', '12');
  track.setAttribute('cy', '12');
  track.setAttribute('r', '10');
  const value = document.createElementNS('http://www.w3.org/2000/svg', 'circle');
  value.setAttribute('class', 'context-ring-value');
  value.setAttribute('cx', '12');
  value.setAttribute('cy', '12');
  value.setAttribute('r', '10');
  svg.append(track, value);
  donut.append(svg);
  const tooltip = el('span', 'context-tooltip');
  tooltip.append(el('span', 'context-tooltip-label', 'Request'));
  tooltip.append(el('span', 'context-tooltip-value', pctText));
  tooltip.append(el('span', 'context-tooltip-detail', exact));
  if (breakdown.length > 0) {
    tooltip.append(el('span', 'context-tooltip-breakdown', breakdown));
  }
  donut.append(tooltip);
  dock.append(donut);
  return dock;
}

function renderComposerDocks(s: SidebarState): HTMLElement {
  const wrap = el('div', 'composer-docks');
  const metricDock = renderRunMetricsDock(s);
  if (metricDock) wrap.append(metricDock);
  wrap.append(renderContextDock(s));
  return wrap;
}

function renderRunMetricsDock(s: SidebarState): HTMLElement | undefined {
  const chips = runMetricChips(s.hud);
  if (chips.length === 0) return undefined;

  const dock = el('div', 'run-metrics-dock');
  for (const chip of chips) {
    const node = el('span', `run-metric-chip metric-${chip.tone}`);
    node.title = chip.title;
    node.append(el('span', 'run-metric-label', chip.label));
    node.append(el('span', 'run-metric-value', chip.value));
    dock.append(node);
  }
  return dock;
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

interface TranscriptScrollPlan {
  wrap: HTMLElement;
  mode: TranscriptScrollMode;
  previousScrollTop: number;
}

function renderTranscriptInto(wrap: HTMLElement, s: SidebarState): TranscriptScrollPlan {
  wrap.className = 'transcript';
  const transcriptKey = s.activeChatId ?? s.sessionId ?? 'draft';
  const sameTranscript = transcriptKey === lastTranscriptAnimationKey;
  const activePrompt = latestPendingPrompt(s);
  const pendingToolIndexes = pendingToolIndexSet(s.transcript);
  const scrollMode = transcriptScrollMode(lastRenderedState, s);
  const previousScrollTop = wrap.scrollTop;
  const animationStartIndex = forceTranscriptBottomOnce
    ? Math.max(0, s.transcript.length - 1)
    : sameTranscript
      ? lastTranscriptCount
      : s.transcript.length;
  bindTranscriptScrollTracking(wrap);
  if (!s.transcript || s.transcript.length === 0) {
    reconcileTranscriptChildren(wrap, [
      keyedTranscriptNode('empty', renderEmptyState(s.context), `empty:${s.context.workspace ?? ''}`),
    ]);
    lastTranscriptAnimationKey = transcriptKey;
    lastTranscriptCount = 0;
    transcriptScrollRestoreToken += 1;
    forceTranscriptBottomOnce = false;
    return { wrap, mode: 'none', previousScrollTop };
  }
  const nextNodes: HTMLElement[] = [];
  for (let index = 0; index < s.transcript.length; index += 1) {
    const item = s.transcript[index];
    if (isActivePromptItem(item, activePrompt)) continue;
    if (item.role === 'tool') {
      const tools: TranscriptItem[] = [];
      const stackStart = index;
      while (index < s.transcript.length && s.transcript[index].role === 'tool') {
        tools.push(s.transcript[index]);
        index += 1;
      }
      const stackEnd = index - 1;
      index -= 1;
      nextNodes.push(
        keyedTranscriptNode(
          toolStackKey(transcriptKey, stackStart),
          decorateTranscriptEntry(
            renderToolStack(tools, stackStart, pendingToolIndexes),
            representativeToolItem(tools, stackStart, pendingToolIndexes),
            stackEnd >= animationStartIndex,
          ),
          toolStackSignature(tools),
        ),
      );
    } else {
      nextNodes.push(
        keyedTranscriptNode(
          transcriptItemKey(transcriptKey, item, index),
          decorateTranscriptEntry(
            renderItem(item, `${transcriptKey}:${index}`),
            item,
            index >= animationStartIndex,
          ),
          transcriptItemSignature(item),
        ),
      );
    }
  }
  reconcileTranscriptChildren(wrap, nextNodes);
  lastTranscriptAnimationKey = transcriptKey;
  lastTranscriptCount = s.transcript.length;
  return { wrap, mode: scrollMode, previousScrollTop };
}

type TranscriptScrollMode = 'none' | 'preserve' | 'bottom';

function scheduleTranscriptScroll(
  wrap: HTMLElement,
  mode: TranscriptScrollMode,
  previousScrollTop: number,
): void {
  const restoreToken = ++transcriptScrollRestoreToken;
  if (mode === 'preserve') {
    // 'preserve': restore scroll once immediately after reconciliation.
    // Do NOT mark as programmatic — user scroll intent must remain respected.
    // Do NOT repeat on rAFs — that would fight user scrolling.
    const maxScrollTop = Math.max(0, wrap.scrollHeight - wrap.clientHeight);
    wrap.scrollTop = Math.min(previousScrollTop, maxScrollTop);
    forceTranscriptBottomOnce = false;
    return;
  }
  applyScrollToBottom(wrap);
  // Consume the one-shot pin flag now — it was honored on the synchronous
  // applyScrollToBottom above. Subsequent rAF follow-ups must check the
  // *current* pin state so the user can interrupt by scrolling up mid-chain.
  forceTranscriptBottomOnce = false;
  // Follow-up rAFs catch late layout shifts (e.g., images loading, fonts
  // settling). Only re-apply if the transcript is still pinned; if the user
  // scrolled up between rAFs the scroll handler has already set
  // transcriptPinnedToBottom=false and we must NOT yank them back to bottom.
  requestAnimationFrame(() => {
    if (!wrap.isConnected || restoreToken !== transcriptScrollRestoreToken) return;
    if (!transcriptPinnedToBottom) return;
    applyScrollToBottom(wrap);
    requestAnimationFrame(() => {
      if (!wrap.isConnected || restoreToken !== transcriptScrollRestoreToken) return;
      if (!transcriptPinnedToBottom) return;
      applyScrollToBottom(wrap);
    });
  });
}

function applyScrollToBottom(wrap: HTMLElement): void {
  wrap.scrollTop = wrap.scrollHeight;
}

function transcriptScrollMode(
  previous: SidebarState | undefined,
  next: SidebarState,
): TranscriptScrollMode {
  if (forceTranscriptBottomOnce) return 'bottom';
  if (isToolOnlyTranscriptChange(previous, next)) return 'preserve';
  return transcriptPinnedToBottom ? 'bottom' : 'preserve';
}

function isToolOnlyTranscriptChange(
  previous: SidebarState | undefined,
  next: SidebarState,
): boolean {
  if (!previous) return false;
  if ((previous.activeChatId ?? previous.sessionId) !== (next.activeChatId ?? next.sessionId)) {
    return false;
  }
  const previousWithoutTools = previous.transcript.filter((item) => item.role !== 'tool');
  const nextWithoutTools = next.transcript.filter((item) => item.role !== 'tool');
  return json(previousWithoutTools) === json(nextWithoutTools);
}

function keyedTranscriptNode(key: string, node: HTMLElement, signature: string): HTMLElement {
  node.dataset.transcriptKey = key;
  node.dataset.renderSignature = signature;
  return node;
}

function transcriptItemKey(transcriptKey: string, item: TranscriptItem, index: number): string {
  const stablePart =
    item.requestId ??
    item.toolName ??
    item.path ??
    item.commandResult?.kind ??
    '';
  return `${transcriptKey}:${index}:${item.role}:${stablePart}`;
}

function toolStackKey(transcriptKey: string, startIndex: number): string {
  return `${transcriptKey}:${startIndex}:tool-stack`;
}

function representativeToolItem(
  items: TranscriptItem[],
  startIndex: number,
  pendingIndexes: ReadonlySet<number>,
): TranscriptItem {
  for (let offset = items.length - 1; offset >= 0; offset -= 1) {
    if (items[offset]?.pending || pendingIndexes.has(startIndex + offset)) {
      return items[offset];
    }
  }
  return items[items.length - 1];
}

function toolStackSignature(items: TranscriptItem[]): string {
  return json(items.map(transcriptItemSignature));
}

function transcriptItemSignature(item: TranscriptItem): string {
  return json({
    role: item.role,
    text: item.text,
    statusKind: item.statusKind,
    detail: item.detail,
    commandResult: item.commandResult,
    requestId: item.requestId,
    request: item.request,
    path: item.path,
    line: item.line,
    column: item.column,
    before: item.before,
    after: item.after,
    toolName: item.toolName,
    reason: item.reason,
    parameters: item.parameters,
    approvalSessionId: item.approvalSessionId,
    pending: item.pending,
    toolParameters: item.toolParameters,
    toolResultSummary: item.toolResultSummary,
    riskClass: item.riskClass,
    compaction: item.compaction,
  });
}

function reconcileTranscriptChildren(wrap: HTMLElement, nextNodes: HTMLElement[]): void {
  const reusable = new Map<string, HTMLElement>();
  Array.from(wrap.children).forEach((child) => {
    if (child instanceof HTMLElement && child.dataset.transcriptKey) {
      reusable.set(child.dataset.transcriptKey, child);
    }
  });

  let cursor: ChildNode | null = wrap.firstChild;
  for (const next of nextNodes) {
    const key = next.dataset.transcriptKey;
    const previous = key ? reusable.get(key) : undefined;
    if (previous instanceof HTMLDetailsElement && next instanceof HTMLDetailsElement) {
      next.open = previous.open;
    }
    const node = reconcileTranscriptNode(previous, next);
    if (node !== cursor) {
      wrap.insertBefore(node, cursor);
    }
    cursor = node.nextSibling;
    if (key) reusable.delete(key);
  }

  for (const stale of reusable.values()) {
    stale.remove();
  }
  while (cursor) {
    const next = cursor.nextSibling;
    cursor.remove();
    cursor = next;
  }
}

function reconcileTranscriptNode(previous: HTMLElement | undefined, next: HTMLElement): HTMLElement {
  if (!previous) return next;
  if (previous.dataset.renderSignature === next.dataset.renderSignature) return previous;
  if (previous.matches('details.tool-stack') && next.matches('details.tool-stack')) {
    return updateToolStackNode(previous as HTMLDetailsElement, next as HTMLDetailsElement);
  }
  if (
    (previous.matches('.msg-assistant') && next.matches('.msg-assistant')) ||
    (previous.matches('.msg-user') && next.matches('.msg-user'))
  ) {
    return updateMessageNode(previous, next);
  }
  return next;
}

function updateToolStackNode(previous: HTMLDetailsElement, next: HTMLDetailsElement): HTMLElement {
  const wasOpen = previous.open;
  previous.className = stableTranscriptClassName(next);
  previous.dataset.renderSignature = next.dataset.renderSignature ?? '';
  previous.open = wasOpen;

  const previousSummary = previous.querySelector<HTMLElement>(':scope > .tool-summary');
  const nextSummary = next.querySelector<HTMLElement>(':scope > .tool-summary');
  if (!previousSummary || !nextSummary) return next;
  previousSummary.className = nextSummary.className;

  const previousToggle = previousSummary.querySelector<HTMLElement>('.tool-toggle');
  const nextToggle = nextSummary.querySelector<HTMLElement>('.tool-toggle');
  if (previousToggle && nextToggle) {
    previousToggle.className = nextToggle.className;
    previousToggle.innerHTML = nextToggle.innerHTML;
  }

  const previousName = previousSummary.querySelector<HTMLElement>('.tool-name');
  const nextName = nextSummary.querySelector<HTMLElement>('.tool-name');
  if (previousName && nextName) {
    updateToolName(previousName, nextName);
  }
  updateToolSummarySnippet(previousSummary, nextSummary);

  const nextHistory = next.querySelector<HTMLElement>(':scope > .tool-history');
  const previousHistory = previous.querySelector<HTMLElement>(':scope > .tool-history');
  if (nextHistory) {
    bindToolHistoryMotion(nextHistory);
    if (previousHistory) {
      reconcileToolHistory(previousHistory, nextHistory);
    } else {
      previous.append(nextHistory);
    }
  } else {
    previousHistory?.remove();
  }
  return previous;
}

function reconcileToolHistory(previousHistory: HTMLElement, nextHistory: HTMLElement): void {
  previousHistory.className = nextHistory.className;
  bindToolHistoryMotion(previousHistory);

  const reusable = new Map<string, HTMLElement>();
  Array.from(previousHistory.children).forEach((child) => {
    if (child instanceof HTMLElement && child.dataset.toolDetailKey) {
      reusable.set(child.dataset.toolDetailKey, child);
    }
  });

  let cursor: ChildNode | null = previousHistory.firstChild;
  Array.from(nextHistory.children).forEach((nextChild) => {
    if (!(nextChild instanceof HTMLElement)) return;
    const key = nextChild.dataset.toolDetailKey;
    const previousChild = key ? reusable.get(key) : undefined;
    const node = previousChild ? updateToolDetailNode(previousChild, nextChild) : nextChild;
    if (node !== cursor) {
      previousHistory.insertBefore(node, cursor);
    }
    cursor = node.nextSibling;
    if (key) reusable.delete(key);
  });

  for (const stale of reusable.values()) {
    stale.remove();
  }
  while (cursor) {
    const next = cursor.nextSibling;
    cursor.remove();
    cursor = next;
  }
}

function updateToolSummarySnippet(previousSummary: HTMLElement, nextSummary: HTMLElement): void {
  const previousSnippet = previousSummary.querySelector<HTMLElement>('.tool-summary-snippet');
  const nextSnippet = nextSummary.querySelector<HTMLElement>('.tool-summary-snippet');
  if (!nextSnippet) {
    previousSnippet?.remove();
    return;
  }
  if (!previousSnippet) {
    previousSummary.append(nextSnippet);
    return;
  }
  previousSnippet.className = nextSnippet.className;
  previousSnippet.textContent = nextSnippet.textContent;
  previousSnippet.title = nextSnippet.title;
}

function updateToolDetailNode(previous: HTMLElement, next: HTMLElement): HTMLElement {
  if (previous.dataset.renderSignature === next.dataset.renderSignature) return previous;
  previous.className = next.className;
  previous.dataset.renderSignature = next.dataset.renderSignature ?? '';
  previous.replaceChildren(...Array.from(next.childNodes));
  return previous;
}

function updateToolName(previousName: HTMLElement, nextName: HTMLElement): void {
  const previousText = previousName.textContent ?? '';
  const nextText = nextName.textContent ?? '';
  cancelToolNameSwap(previousName);
  previousName.className = nextName.className;
  if (previousText === nextText) return;
  if (window.matchMedia('(prefers-reduced-motion: reduce)').matches) {
    previousName.textContent = nextText;
    return;
  }
  const swapId = String(Number(previousName.dataset.swapId ?? '0') + 1);
  previousName.dataset.swapId = swapId;

  // Phase 1: fade out via CSS transition
  previousName.classList.add('tool-name-swap-out');

  const outTimer = window.setTimeout(() => {
    if (previousName.dataset.swapId !== swapId) return;

    // Phase 2: swap text while invisible, prepare fade-in start position
    previousName.classList.remove('tool-name-swap-out');
    previousName.textContent = nextText;
    previousName.className = nextName.className;
    previousName.classList.add('tool-name-swap-in');

    // Phase 3: next frame — remove swap-in class so CSS transition fades in
    requestAnimationFrame(() => {
      if (previousName.dataset.swapId !== swapId) return;
      previousName.classList.remove('tool-name-swap-in');
    });

    const doneTimer = window.setTimeout(() => {
      if (previousName.dataset.swapId !== swapId) return;
      toolNameSwapTimers.delete(previousName);
    }, 200);
    toolNameSwapTimers.set(previousName, doneTimer);
  }, 120);
  toolNameSwapTimers.set(previousName, outTimer);
}

function cancelToolNameSwap(name: HTMLElement): void {
  const timer = toolNameSwapTimers.get(name);
  if (timer !== undefined) {
    window.clearTimeout(timer);
    toolNameSwapTimers.delete(name);
  }
  name.dataset.swapId = String(Number(name.dataset.swapId ?? '0') + 1);
  name.classList.remove('tool-name-swap-out', 'tool-name-swap-in');
}

function updateMessageNode(previous: HTMLElement, next: HTMLElement): HTMLElement {
  previous.className = stableTranscriptClassName(next);
  previous.dataset.renderSignature = next.dataset.renderSignature ?? '';
  const previousBody = previous.querySelector<HTMLElement>('.msg-body');
  const nextBody = next.querySelector<HTMLElement>('.msg-body');
  if (previousBody && nextBody && previousBody.innerHTML !== nextBody.innerHTML) {
    reconcileMarkdownBody(previousBody, nextBody);
  }
  const previousFooter = previous.querySelector<HTMLElement>('.msg-footer');
  const nextFooter = next.querySelector<HTMLElement>('.msg-footer');
  if (previousFooter && nextFooter) {
    previousFooter.replaceWith(nextFooter);
  } else if (!previousFooter && nextFooter) {
    previous.append(nextFooter);
  } else if (previousFooter && !nextFooter) {
    previousFooter.remove();
  } else {
    const previousCopy = previous.querySelector<HTMLElement>('.copy-button');
    const nextCopy = next.querySelector<HTMLElement>('.copy-button');
    if (previousCopy && nextCopy) {
      previousCopy.replaceWith(nextCopy);
    }
  }
  return previous;
}

/**
 * Reconcile the rendered markdown body in place during streaming.
 *
 * The naive approach (`previousBody.innerHTML = nextBody.innerHTML`) blows
 * away every DOM child on every delta. That produces a visible flash on
 * each render — the entire bubble's contents are replaced ~30 times per
 * second. It also wipes any in-flight CSS animation that was running on
 * descendants.
 *
 * Instead we do child-by-child diff:
 *   - Children whose tag + class match are updated in-place by re-assigning
 *     their innerHTML (cheap — only the leaf paragraph at the streaming tail
 *     actually changes content most of the time).
 *   - Structural changes (different tag/class) are replaced wholesale.
 *   - New children at the end are appended; surplus children are removed.
 *
 * Net effect: every other block in the response remains the same DOM node
 * across renders, so there's nothing to repaint except the actively-growing
 * leaf. This is the "smooth Claude Desktop / Codex" feel.
 */
function reconcileMarkdownBody(previous: HTMLElement, next: HTMLElement): void {
  previous.className = next.className;
  const prevChildren = Array.from(previous.children) as HTMLElement[];
  const nextChildren = Array.from(next.children) as HTMLElement[];
  const sharedLen = Math.min(prevChildren.length, nextChildren.length);
  for (let i = 0; i < sharedLen; i++) {
    const p = prevChildren[i];
    const n = nextChildren[i];
    if (p.tagName !== n.tagName || p.className !== n.className) {
      p.replaceWith(n);
      continue;
    }
    if (p.innerHTML !== n.innerHTML) {
      p.innerHTML = n.innerHTML;
    }
  }
  for (let i = sharedLen; i < nextChildren.length; i++) {
    previous.appendChild(nextChildren[i]);
  }
  for (let i = prevChildren.length - 1; i >= sharedLen; i--) {
    prevChildren[i].remove();
  }
  // Re-run link/table/path post-processing on the updated tree. Cheap idempotent ops.
  postProcessMarkdownBody(previous);
}

function stableTranscriptClassName(node: HTMLElement): string {
  return node.className
    .split(/\s+/)
    .filter((className) => className.length > 0 && !className.startsWith('bubble-enter'))
    .join(' ');
}

function pendingToolIndexSet(items: TranscriptItem[]): ReadonlySet<number> {
  const indexes = new Set<number>();
  items.forEach((item, index) => {
    if (item.role === 'tool' && item.pending) indexes.add(index);
  });
  return indexes;
}

function decorateTranscriptEntry(
  node: HTMLElement,
  item: TranscriptItem,
  shouldAnimate: boolean,
): HTMLElement {
  if (!shouldAnimate) return node;
  node.classList.add('bubble-enter', `bubble-enter-${animationKindForItem(item)}`);
  scheduleBubbleEnterCleanup(node);
  return node;
}

function scheduleBubbleEnterCleanup(node: HTMLElement): void {
  const cleanup = (): void => {
    node.className = stableTranscriptClassName(node);
  };
  node.addEventListener('animationend', cleanup, { once: true });
  window.setTimeout(cleanup, 360);
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
  const body = renderMarkdownBody(item.text);
  assistantTextByKey.set(streamKey, { markdown: item.text, visibleText: body.textContent ?? '' });
  wrap.append(body);
  const footer = el('div', 'msg-footer');
  const copy = el('button', 'copy-button', '');
  copy.type = 'button';
  copy.title = 'Copy response';
  copy.setAttribute('aria-label', 'Copy response');
  copy.innerHTML = iconSvg('copy');
  copy.addEventListener('click', () => {
    void markCopied(copy, item.text);
  });
  footer.append(copy);
  wrap.append(footer);
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
  vscode.postMessage({ type: 'copyText', text: String(text) });
}

function renderMarkdownBody(markdown: string): HTMLElement {
  const body = el('div', 'msg-body markdown-body');
  body.innerHTML = markdownRenderer.render(markdown);
  postProcessMarkdownBody(body);
  return body;
}

function postProcessMarkdownBody(body: HTMLElement): void {
  body.querySelectorAll('a[href]').forEach((link) => {
    link.setAttribute('target', '_blank');
    link.setAttribute('rel', 'noreferrer noopener');
  });
  body.querySelectorAll('table').forEach((table) => {
    if (table.parentElement?.classList.contains('md-table-wrap')) return;
    const wrap = el('div', 'md-table-wrap');
    table.replaceWith(wrap);
    wrap.append(table);
  });
  linkifyFilePaths(body);
}

// Match file paths like `src/foo.rs:10`, `src/foo.rs:10-20`, `src/foo.rs`
// inside inline code (backtick) or parentheses. Heuristic: must contain `/`
// and end with a recognised source extension, optionally followed by `:line`
// or `:line-line`.
const FILE_PATH_RE =
  /(?:^|(?<=[\s(`]))([a-zA-Z0-9_./-]+\/[a-zA-Z0-9_.-]+\.(?:rs|ts|tsx|js|jsx|json|toml|yaml|yml|py|go|java|c|cpp|h|hpp|css|html|md|sh|sql|proto|graphql|svelte|vue))(?::(\d+)(?:-(\d+))?)?(?=$|[\s)`,;.])/g;

function linkifyFilePaths(root: HTMLElement): void {
  const walker = document.createTreeWalker(root, NodeFilter.SHOW_TEXT);
  const targets: { node: Text; matches: RegExpMatchArray[] }[] = [];
  while (walker.nextNode()) {
    const node = walker.currentNode as Text;
    if (!node.nodeValue) continue;
    // Skip if already inside a link or code block
    if (node.parentElement?.closest('a, pre')) continue;
    const matches = [...node.nodeValue.matchAll(FILE_PATH_RE)];
    if (matches.length > 0) targets.push({ node, matches });
  }
  for (const { node, matches } of targets) {
    const text = node.nodeValue ?? '';
    const fragment = document.createDocumentFragment();
    let cursor = 0;
    for (const match of matches) {
      const matchStart = match.index ?? 0;
      const fullMatch = match[0];
      const path = match[1];
      const lineStr = match[2];
      const line = lineStr ? Number(lineStr) : undefined;
      if (matchStart > cursor) {
        fragment.append(document.createTextNode(text.slice(cursor, matchStart)));
      }
      const link = document.createElement('button');
      link.className = 'link-button file-link inline-file-link';
      link.type = 'button';
      link.textContent = fullMatch;
      link.title = line ? `Open ${path}:${line}` : `Open ${path}`;
      link.addEventListener('click', (event) => {
        event.preventDefault();
        event.stopPropagation();
        vscode.postMessage({ type: 'openFile', path, line });
      });
      fragment.append(link);
      cursor = matchStart + fullMatch.length;
    }
    if (cursor < text.length) {
      fragment.append(document.createTextNode(text.slice(cursor)));
    }
    node.replaceWith(fragment);
  }
}

function toolSnippetText(item: TranscriptItem): string | undefined {
  if (item.path) return item.path;
  if (!item.toolParameters || typeof item.toolParameters !== 'object') return undefined;
  const params = item.toolParameters as Record<string, unknown>;
  const path = params.path ?? params.file_path ?? params.target_file;
  if (typeof path === 'string') return path;
  const command = params.command ?? params.cmd;
  if (typeof command === 'string') {
    const trimmed = command.trim();
    return trimmed.length > 80 ? trimmed.slice(0, 77) + '...' : trimmed;
  }
  const query = params.query ?? params.pattern ?? params.search ?? params.url;
  if (typeof query === 'string') {
    const trimmed = query.trim();
    return trimmed.length > 80 ? trimmed.slice(0, 77) + '...' : trimmed;
  }
  return undefined;
}

function renderToolBlock(item: TranscriptItem): HTMLElement {
  return renderToolStack([item]);
}

function renderToolStack(
  items: TranscriptItem[],
  startIndex = 0,
  pendingIndexes: ReadonlySet<number> = new Set(),
): HTMLElement {
  let activeOffset = -1;
  for (let offset = items.length - 1; offset >= 0; offset -= 1) {
    if (items[offset]?.pending || pendingIndexes.has(startIndex + offset)) {
      activeOffset = offset;
      break;
    }
  }
  const active = activeOffset >= 0 ? items[activeOffset] : undefined;
  const latest = active ?? items[items.length - 1];
  const isRunning = Boolean(active);
  const details = el('details', `tool-stack ${isRunning ? 'tool-stack-running' : ''}`);
  details.addEventListener('toggle', () => {
    if (details.open) {
      ensureToolHistoryRendered(details, items, startIndex, pendingIndexes);
    }
  });
  const summary = el('summary', 'tool-summary');
  const name = el(
    'span',
    `tool-name ${isRunning ? 'text-gradient-active' : ''}`,
    latest.toolName || latest.text,
  );
  const toggle = el('span', 'tool-toggle');
  summary.append(toggle, name);
  // Risk-class chip — surfaces the tool's potential harm class so the
  // user can tell at a glance "this is a destructive shell call" vs "this
  // is just a read." Class strings match the Rust `RiskClass::label()`
  // values; missing means the daemon didn't send one and we render no chip.
  const risk = riskChipView(latest.riskClass);
  if (risk) {
    const chip = el('span', risk.className, risk.label);
    chip.title = risk.title;
    summary.append(chip);
  }
  const snippet = toolSnippetText(latest);
  if (snippet) {
    const snippetEl = el('span', 'tool-summary-snippet', snippet);
    snippetEl.title = snippet;
    summary.append(snippetEl);
  }
  details.append(summary);
  ensureToolHistoryRendered(details, items, startIndex, pendingIndexes);
  return details;
}

function ensureToolHistoryRendered(
  details: HTMLElement,
  items: TranscriptItem[],
  startIndex = 0,
  pendingIndexes: ReadonlySet<number> = new Set(),
): void {
  if (details.querySelector('.tool-history')) return;
  const history = el('div', 'tool-history');
  bindToolHistoryMotion(history);
  items.forEach((item, offset) => {
    const detail = renderToolDetail(item, item.pending || pendingIndexes.has(startIndex + offset));
    detail.dataset.toolDetailKey = toolDetailKey(startIndex, offset);
    detail.dataset.renderSignature = transcriptItemSignature(item);
    history.append(detail);
  });
  details.append(history);
}

function toolDetailKey(startIndex: number, offset: number): string {
  return `tool-detail:${startIndex + offset}`;
}

function renderThinkingBlock(item: TranscriptItem): HTMLElement {
  const details = el('details', 'thinking-block thinking-active');
  const summary = el('summary', 'thinking-summary');
  const label = el('span', 'thinking-label text-gradient-active', 'Thinking');
  const state = el('span', 'thinking-state', 'reasoning trace');
  summary.append(label, state);
  details.append(summary);
  const body = el('pre', 'thinking-body');
  body.textContent = item.text;
  details.append(body);
  return details;
}

function renderToolDetail(item: TranscriptItem, isActive = false): HTMLElement {
  const wrap = el('div', 'tool-detail-item');
  const header = el('div', 'tool-detail-header');
  header.append(
    el('span', `tool-detail-name ${isActive ? 'text-gradient-active' : ''}`, item.toolName || item.text),
  );
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

function actionRow(...children: HTMLElement[]): HTMLElement {
  const row = el('div', 'msg-actions');
  row.append(...children);
  return row;
}

function actionButton(
  variant: 'primary' | 'secondary',
  label: string,
  onClick: () => void,
): HTMLButtonElement {
  const button = el(
    'button',
    `${variant === 'primary' ? 'primary-button' : 'secondary-button'} compact-button`,
    label,
  ) as HTMLButtonElement;
  button.type = 'button';
  button.addEventListener('click', onClick);
  return button;
}

function selectControl<Value extends string>(
  className: string,
  title: string,
  options: readonly SelectOption<Value>[],
  current: Value,
  id?: string,
): HTMLSelectElement {
  const select = document.createElement('select');
  select.className = className;
  if (id) select.id = id;
  select.title = title;
  appendSelectOptions(select, options, current);
  return select;
}

function appendSelectOptions<Value extends string>(
  select: HTMLSelectElement,
  options: readonly SelectOption<Value>[],
  current: Value,
): void {
  for (const [value, label] of options) {
    const option = document.createElement('option');
    option.value = value;
    option.textContent = label;
    if (current === value) option.selected = true;
    select.append(option);
  }
}

function renderStatusLine(item: TranscriptItem): HTMLElement {
  if (item.compaction) return renderCompactionStatus(item);
  if (item.statusKind === 'completion') return renderCompletionStatus(item);
  const wrap = el('div', 'status-line');
  wrap.append(el('span', 'status-dot'));
  wrap.append(el('span', 'status-text', item.text));
  if (item.detail) wrap.append(el('span', 'status-detail', `· ${item.detail}`));
  return wrap;
}

function renderCompletionStatus(item: TranscriptItem): HTMLElement {
  const wrap = el('div', 'status-line completion-status');
  wrap.append(el('span', 'status-dot completion-dot'));
  wrap.append(el('span', 'status-text', item.text));
  if (item.detail) wrap.append(el('span', 'status-detail', `· ${item.detail}`));
  return wrap;
}

function renderCompactionStatus(item: TranscriptItem): HTMLElement {
  const snapshot = item.compaction;
  const wrap = el('div', 'status-line status-line-compactable');
  const row = el('div', 'status-line-row');
  row.append(el('span', 'status-dot'));
  row.append(el('span', 'status-text', item.text));
  wrap.append(row);
  if (snapshot) wrap.append(renderCompactionDetails(snapshot));
  return wrap;
}

function renderCompactionDetails(snapshot: CompactionSnapshotView): HTMLElement {
  const details = el('details', 'compaction-details');
  const summary = el('summary', 'compaction-summary');
  summary.append(el('span', 'compaction-toggle'));
  summary.append(el('span', 'compaction-summary-text', 'Snapshot'));
  summary.append(
    el(
      'span',
      'compaction-summary-counts',
      `${snapshot.filesRead.length + snapshot.filesChanged.length} files · ${snapshot.decisions.length} decisions`,
    ),
  );
  details.append(summary);

  const body = el('div', 'compaction-body');
  if (snapshot.narrative) {
    body.append(el('div', 'compaction-narrative', snapshot.narrative));
  }
  const sections = [
    ['Decisions', snapshot.decisions],
    ['Files read', snapshot.filesRead],
    ['Files changed', snapshot.filesChanged],
    ['Verifications', snapshot.verifications],
    ['Open todos', snapshot.openTodos],
    ['Approvals', snapshot.approvals],
    ['Untrusted inputs', snapshot.untrustedInputs],
  ] as const;
  for (const [title, items] of sections) {
    if (items.length > 0) body.append(renderCompactionSection(title, items));
  }
  if (body.childElementCount === 0) {
    body.append(el('div', 'compaction-empty', 'No structured details'));
  }
  details.append(body);
  return details;
}

function renderCompactionSection(title: string, items: readonly CompactionDetailItem[]): HTMLElement {
  const section = el('section', 'compaction-section');
  const header = el('div', 'compaction-section-title');
  header.append(el('span', 'compaction-section-name', title));
  header.append(el('span', 'compaction-section-count', String(items.length)));
  section.append(header);
  const list = el('ul', 'compaction-list');
  for (const item of items) {
    const li = el('li', 'compaction-item');
    if (item.path) {
      li.append(renderFilePathButton(item.path, 'compaction-path', item.line));
      if (item.line) {
        const suffix = item.endLine && item.endLine !== item.line ? `:${item.line}-${item.endLine}` : `:${item.line}`;
        li.append(el('span', 'compaction-line-range', suffix));
      }
    } else {
      li.append(el('span', 'compaction-label', item.label));
    }
    if (item.detail) li.append(el('span', 'compaction-detail', item.detail));
    list.append(li);
  }
  section.append(list);
  return section;
}

function renderErrorLine(item: TranscriptItem): HTMLElement {
  const wrap = el('div', 'error-line');
  wrap.append(el('span', 'error-icon', '!'));
  wrap.append(el('span', 'error-text', item.text));
  return wrap;
}

function renderAskUserBubble(item: TranscriptItem): HTMLElement {
  const wrap = renderPromptPanel('ask-user', 'Input requested', item.text);
  wrap.append(renderAskUserForm(item));
  return wrap;
}

function renderApprovalBubble(item: TranscriptItem): HTMLElement {
  const wrap = renderPromptPanel('approval', 'Approval requested', item.toolName || item.text);
  const risk = riskChipView(item.riskClass);
  if (risk) {
    const chip = el('span', risk.className, risk.label);
    chip.title = risk.title;
    wrap.querySelector('.prompt-header')?.append(chip);
  }
  if (item.reason) wrap.append(el('div', 'prompt-reason', item.reason));

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

  const scope = selectControl(
    'scope-select',
    'Approval scope',
    APPROVAL_SCOPE_OPTIONS,
    'once',
  );

  const approve = actionButton('primary', 'Approve', () => {
    vscode.postMessage({
      type: 'approvalRespond',
      approved: true,
      scope: scope.value as ApprovalScopeValue,
      toolName: item.toolName,
      reason: item.reason,
      parameters: item.parameters,
      sessionId: item.approvalSessionId,
    });
  });
  const deny = actionButton('secondary', 'Deny', () => {
    vscode.postMessage({
      type: 'approvalRespond',
      approved: false,
      scope: scope.value as ApprovalScopeValue,
      toolName: item.toolName,
      reason: item.reason,
      parameters: item.parameters,
      sessionId: item.approvalSessionId,
    });
  });
  wrap.append(actionRow(scope, approve, deny));
  return wrap;
}

function latestPendingApproval(s: SidebarState): TranscriptItem | undefined {
  return s.pendingApproval ?? [...s.transcript].reverse().find((item) => item.role === 'approval');
}

function latestPendingInteraction(s: SidebarState): TranscriptItem | undefined {
  return [...s.transcript]
    .reverse()
    .find((item) => item.role === 'interaction' && Boolean(item.requestId));
}

function latestPendingPrompt(s: SidebarState): TranscriptItem | undefined {
  return latestPendingApproval(s) ?? latestPendingInteraction(s);
}

function isActivePromptItem(item: TranscriptItem, active: TranscriptItem | undefined): boolean {
  if (!active) return false;
  if (item === active) return true;
  if (item.role !== active.role) return false;
  if (item.role === 'interaction') return Boolean(item.requestId && item.requestId === active.requestId);
  if (item.role === 'approval') {
    return (
      item.toolName === active.toolName &&
      item.reason === active.reason &&
      json(item.parameters) === json(active.parameters)
    );
  }
  return false;
}

function renderPromptDock(item: TranscriptItem): HTMLElement {
  const wrap = el('div', 'prompt-dock');
  wrap.append(item.role === 'approval' ? renderApprovalBubble(item) : renderAskUserBubble(item));
  return wrap;
}

function renderPromptPanel(kind: 'approval' | 'ask-user', label: string, title: string): HTMLElement {
  const wrap = el('section', `prompt-panel prompt-panel-${kind}`);
  const header = el('div', 'prompt-header');
  header.append(el('span', 'prompt-label', label));
  header.append(el('span', 'prompt-title', title));
  wrap.append(header);
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
    appendOtherChoice(wrap, item.requestId ?? 'ask-user');
  } else if (kind === 'multi_select') {
    options.forEach((option, index) => {
      const label = el('label', 'choice');
      const input = document.createElement('input');
      input.type = 'checkbox';
      input.value = String(index);
      label.append(input, document.createTextNode(option));
      wrap.append(label);
    });
    appendOtherChoice(wrap);
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

  const send = actionButton('primary', 'Send', sendAnswer);
  const cancel = actionButton('secondary', 'Cancel', () => {
    if (!item.requestId) return;
    vscode.postMessage({
      type: 'askUserRespond',
      requestId: item.requestId,
      answer: { kind: 'cancelled' },
    });
  });
  wrap.append(actionRow(send, cancel));
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

function appendOtherChoice(wrap: HTMLElement, radioName?: string): void {
  const label = el('label', 'choice choice-other');
  const toggle = document.createElement('input');
  toggle.type = radioName ? 'radio' : 'checkbox';
  if (radioName) toggle.name = radioName;
  toggle.value = '__other__';
  toggle.dataset.otherToggle = 'true';

  const content = el('div', 'choice-other-body');
  content.append(el('span', 'choice-other-label', 'Other'));
  const input = el('input', 'inline-text-input ask-other-input') as HTMLInputElement;
  input.type = 'text';
  input.placeholder = 'Write your answer';
  input.dataset.otherText = 'true';
  input.disabled = true;
  content.append(input);

  toggle.addEventListener('change', () => {
    input.disabled = !toggle.checked;
    if (toggle.checked) setTimeout(() => input.focus(), 0);
  });
  input.addEventListener('focus', () => {
    toggle.checked = true;
    input.disabled = false;
  });

  label.append(toggle, content);
  wrap.append(label);
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
    const otherToggle = wrap.querySelector<HTMLInputElement>('[data-other-toggle="true"]:checked');
    if (otherToggle) {
      return { kind: 'text', text: otherTextValue(wrap) };
    }
    const selected = wrap.querySelector<HTMLInputElement>('input[type="radio"]:checked');
    const index = selected ? Number(selected.value) : Number(request.default_index ?? 0);
    return { kind: 'selected', index, text: String(options[index] ?? '') };
  }
  if (request.kind === 'multi_select') {
    const otherToggle = wrap.querySelector<HTMLInputElement>('[data-other-toggle="true"]:checked');
    const indices = Array.from(wrap.querySelectorAll<HTMLInputElement>('input[type="checkbox"]:checked'))
      .map((input) => Number(input.value))
      .filter((value) => Number.isFinite(value));
    if (otherToggle) {
      const selectedText = indices.map((index) => options[index]).filter(Boolean);
      const otherText = otherTextValue(wrap);
      return {
        kind: 'text',
        text: selectedText.length > 0
          ? [...selectedText, otherText].filter((value) => value.trim().length > 0).join(', ')
          : otherText,
      };
    }
    return { kind: 'multi_selected', indices };
  }
  const input = wrap.querySelector<HTMLInputElement>('[data-freeform="true"]');
  return { kind: 'text', text: input ? input.value : '' };
}

function otherTextValue(wrap: HTMLElement): string {
  const input = wrap.querySelector<HTMLInputElement>('[data-other-text="true"]');
  return input ? input.value : '';
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
  if (result?.kind === 'codemap') return renderCodeMapBlock(item);
  if (result?.kind === 'codemap_status') return renderCodeMapStatusBlock(item);
  if (result?.kind === 'attach') return renderAttachmentBlock(item);
  if (result?.kind === 'attachments') return renderAttachmentInventoryBlock(item);
  if (result?.kind === 'detach') return renderDetachBlock(item);
  if (result?.kind === 'session_export') return renderSessionExportBlock(item);
  if (result?.kind === 'session_import') return renderSessionImportBlock(item);
  if (result?.kind === 'session_locate') return renderSessionLocateBlock(item);
  if (result?.kind === 'note' || result?.kind === 'notes' || result?.kind === 'notes_clear') {
    return renderNotesBlock(item);
  }
  if (result?.kind === 'skills') return renderSkillsBlock(item);
  if (result?.kind === 'skill_detail') return renderSkillDetailBlock(item);
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

function renderAttachmentBlock(item: TranscriptItem): HTMLElement {
  const result = item.commandResult;
  const attachment = attachmentFromResult(result);
  const path = attachment.path ?? 'attachment';
  const bytes = typeof attachment.bytes === 'number' ? attachment.bytes : undefined;
  const mediaType = attachment.media_type ?? attachment.mediaType ?? 'text/plain';
  const inlined = attachment.inlined === true;
  const content = typeof attachment.content === 'string' ? attachment.content : undefined;
  const wrap = el('section', 'command-block attachment-block');
  const header = el('div', 'attachment-header');
  const title = el('div', 'attachment-title');
  title.append(el('span', 'command-title', result?.title ?? 'Attachment'));
  const chips = el('div', 'attachment-chips');
  chips.append(el('span', 'command-chip', inlined ? 'inlined' : 'placeholder'));
  if (bytes !== undefined) chips.append(el('span', 'command-chip', `${bytes} bytes`));
  chips.append(el('span', 'command-chip', mediaType));
  title.append(chips);
  header.append(title);

  const actions = el('div', 'attachment-actions');
  const open = iconButton('open', `Open ${path}`, () => {
    vscode.postMessage({ type: 'openFile', path });
  });
  actions.append(open);
  const copyPath = iconButton('copy', `Copy ${path}`, () => {
    void markCopied(copyPath, path);
  });
  actions.append(copyPath);
  if (content !== undefined) {
    const copyContent = iconButton('copy', `Copy attached content from ${path}`, () => {
      void markCopied(copyContent, content);
    });
    actions.append(copyContent);
  }
  actions.append(iconButton('remove', `Detach ${path}`, () => {
    vscode.postMessage({ type: 'detachAttachment', path });
  }));
  header.append(actions);
  wrap.append(header);

  const pathRow = el('div', 'attachment-path-row');
  pathRow.append(renderFilePathButton(path, 'command-path'));
  wrap.append(pathRow);

  if (content !== undefined) {
    const preview = el('pre', 'attachment-preview');
    preview.textContent = previewAttachmentContent(content);
    wrap.append(preview);
  } else {
    wrap.append(el('div', 'attachment-placeholder', `${mediaType} attachment placeholder`));
  }
  return wrap;
}

function renderDetachBlock(item: TranscriptItem): HTMLElement {
  const result = item.commandResult;
  const wrap = el('section', 'command-block attachment-block attachment-inventory-block');
  const header = el('div', 'attachment-header');
  const title = el('div', 'attachment-title');
  title.append(el('span', 'command-title', result?.title ?? 'Detach Attachment'));
  const chips = el('div', 'attachment-chips');
  const removed = result?.removed_count ?? result?.removedCount ?? 0;
  const remaining = result?.remaining_count ?? result?.remainingCount;
  chips.append(el('span', 'command-chip', `${removed} removed`));
  if (typeof remaining === 'number') chips.append(el('span', 'command-chip', `${remaining} remaining`));
  title.append(chips);
  header.append(title);
  wrap.append(header);
  if (result?.message) wrap.append(el('div', 'command-message', result.message));
  const removedItems = Array.isArray(result?.removed) ? result.removed : [];
  if (removedItems.length > 0) {
    const removedList = el('div', 'attachment-list');
    removedItems.forEach((attachment) => {
      removedList.append(renderRemovedAttachmentRow(attachment));
    });
    wrap.append(removedList);
  }
  const remainingItems = attachmentsFromResult(result);
  if (remainingItems.length > 0) {
    const remainingTitle = el('div', 'codemap-group-title', `Remaining · ${remainingItems.length}`);
    wrap.append(remainingTitle);
    const remainingList = el('div', 'attachment-list');
    remainingItems.forEach((attachment) => {
      remainingList.append(renderAttachmentInventoryRow(attachment));
    });
    wrap.append(remainingList);
  }
  return wrap;
}

function renderRemovedAttachmentRow(attachment: AttachmentView): HTMLElement {
  const path = attachment.path ?? 'attachment';
  const bytes = typeof attachment.bytes === 'number' ? `${attachment.bytes} bytes` : '';
  const mediaType = attachment.media_type ?? attachment.mediaType ?? 'text/plain';
  const row = el('div', 'attachment-inventory-row attachment-removed-row');
  const main = el('div', 'attachment-row-main');
  const top = el('div', 'attachment-path-row');
  top.append(el('span', 'command-row-label', path));
  const meta = [bytes, mediaType, 'removed'].filter(Boolean).join(' · ');
  if (meta) top.append(el('span', 'command-row-meta', meta));
  main.append(top);
  row.append(main);
  return row;
}

function renderAttachmentInventoryBlock(item: TranscriptItem): HTMLElement {
  const result = item.commandResult;
  const attachments = attachmentsFromResult(result);
  const wrap = el('section', 'command-block attachment-block attachment-inventory-block');
  const header = el('div', 'attachment-header');
  const title = el('div', 'attachment-title');
  title.append(el('span', 'command-title', result?.title ?? 'Session Attachments'));
  const chips = el('div', 'attachment-chips');
  chips.append(el('span', 'command-chip', `${result?.total ?? attachments.length} files`));
  title.append(chips);
  header.append(title);
  wrap.append(header);
  if (result?.message) wrap.append(el('div', 'command-message', result.message));
  if (attachments.length === 0) {
    wrap.append(el('div', 'attachment-placeholder', 'No files attached to this session.'));
    return wrap;
  }
  const list = el('div', 'attachment-list');
  attachments.forEach((attachment) => {
    list.append(renderAttachmentInventoryRow(attachment));
  });
  wrap.append(list);
  return wrap;
}

function renderAttachmentInventoryRow(attachment: AttachmentView): HTMLElement {
  const path = attachment.path ?? 'attachment';
  const bytes = typeof attachment.bytes === 'number' ? `${attachment.bytes} bytes` : '';
  const mediaType = attachment.media_type ?? attachment.mediaType ?? 'text/plain';
  const mode = attachment.inlined ? 'inlined' : 'placeholder';
  const content = typeof attachment.content === 'string' ? attachment.content : undefined;
  const row = el('div', 'attachment-inventory-row');
  const main = el('div', 'attachment-row-main');
  const top = el('div', 'attachment-path-row');
  top.append(renderFilePathButton(path, 'command-path'));
  const meta = [bytes, mediaType, mode].filter(Boolean).join(' · ');
  if (meta) top.append(el('span', 'command-row-meta', meta));
  main.append(top);
  row.append(main);
  const actions = el('div', 'attachment-actions');
  actions.append(iconButton('open', `Open ${path}`, () => {
    vscode.postMessage({ type: 'openFile', path });
  }));
  const copyPath = iconButton('copy', `Copy ${path}`, () => {
    void markCopied(copyPath, path);
  });
  actions.append(copyPath);
  if (content !== undefined) {
    const copyContent = iconButton('copy', `Copy attached content from ${path}`, () => {
      void markCopied(copyContent, content);
    });
    actions.append(copyContent);
  }
  actions.append(iconButton('remove', `Detach ${path}`, () => {
    vscode.postMessage({ type: 'detachAttachment', path });
  }));
  row.append(actions);
  return row;
}

function renderSessionExportBlock(item: TranscriptItem): HTMLElement {
  const result = item.commandResult;
  const artifacts = Array.isArray(result?.artifacts) ? result.artifacts : [];
  const destination = typeof result?.destination === 'string' ? result.destination : undefined;
  const wrap = el('section', 'command-block attachment-block');
  const header = el('div', 'attachment-header');
  const title = el('div', 'attachment-title');
  title.append(el('span', 'command-title', result?.title ?? 'Session Artifact Export'));
  const chips = el('div', 'attachment-chips');
  chips.append(el('span', 'command-chip', `${artifacts.length} files`));
  title.append(chips);
  header.append(title);
  if (destination) {
    const actions = el('div', 'attachment-actions');
    actions.append(iconButton('open', `Open ${destination}`, () => {
      vscode.postMessage({ type: 'openPath', path: destination });
    }));
    actions.append(iconButton('copy', `Copy ${destination}`, () => {
      vscode.postMessage({ type: 'copyText', text: destination });
    }));
    header.append(actions);
  }
  wrap.append(header);
  if (result?.message) wrap.append(el('div', 'command-message', result.message));
  if (destination) wrap.append(el('div', 'attachment-path-row', destination));
  if (artifacts.length > 0) {
    const list = el('div', 'attachment-list');
    artifacts.forEach((artifact) => {
      const row = el('div', 'attachment-inventory-row');
      const main = el('div', 'attachment-row-main');
      const top = el('div', 'attachment-path-row');
      top.append(el('span', 'attachment-path', artifact.path ?? 'artifact'));
      const meta = [
        artifact.class,
        typeof artifact.count === 'number' ? `${artifact.count} entries` : '',
      ].filter(Boolean);
      if (meta.length > 0) top.append(el('span', 'attachment-meta', meta.join(' · ')));
      main.append(top);
      row.append(main);
      list.append(row);
    });
    wrap.append(list);
  }
  return wrap;
}

function renderSessionImportBlock(item: TranscriptItem): HTMLElement {
  const result = item.commandResult;
  const files = Array.isArray(result?.files)
    ? result.files.filter((file): file is string => typeof file === 'string')
    : [];
  const source = typeof result?.source === 'string' ? result.source : undefined;
  const destination = typeof result?.destination === 'string' ? result.destination : undefined;
  const sessionId = typeof result?.id === 'string' ? result.id : result?.session_id;
  const wrap = el('section', 'command-block attachment-block');
  const header = el('div', 'attachment-header');
  const title = el('div', 'attachment-title');
  title.append(el('span', 'command-title', result?.title ?? 'Session Artifact Import'));
  const chips = el('div', 'attachment-chips');
  chips.append(el('span', 'command-chip', `${files.length || result?.total || 0} files`));
  if (sessionId) chips.append(el('span', 'command-chip', sessionId));
  title.append(chips);
  header.append(title);
  if (destination) {
    const actions = el('div', 'attachment-actions');
    actions.append(iconButton('open', `Open ${destination}`, () => {
      vscode.postMessage({ type: 'openPath', path: destination });
    }));
    actions.append(iconButton('copy', `Copy ${destination}`, () => {
      vscode.postMessage({ type: 'copyText', text: destination });
    }));
    header.append(actions);
  }
  wrap.append(header);
  if (result?.message) wrap.append(el('div', 'command-message', result.message));
  if (source) wrap.append(el('div', 'attachment-path-row', `from ${source}`));
  if (destination) wrap.append(el('div', 'attachment-path-row', `to ${destination}`));
  if (files.length > 0) {
    const list = el('div', 'attachment-list');
    files.forEach((file) => {
      const row = el('div', 'attachment-inventory-row');
      const main = el('div', 'attachment-row-main');
      const top = el('div', 'attachment-path-row');
      top.append(el('span', 'attachment-path', file));
      main.append(top);
      row.append(main);
      list.append(row);
    });
    wrap.append(list);
  }
  return wrap;
}

function renderSessionLocateBlock(item: TranscriptItem): HTMLElement {
  const result = item.commandResult;
  const sessionId = typeof result?.session_id === 'string' ? result.session_id : undefined;
  const directory = typeof result?.path === 'string' ? result.path : undefined;
  const exists = result?.exists === true;
  const wrap = el('section', `command-block attachment-block ${result?.severity === 'error' ? 'error' : ''}`);
  const header = el('div', 'attachment-header');
  const title = el('div', 'attachment-title');
  title.append(el('span', 'command-title', result?.title ?? 'Session Locate'));
  const chips = el('div', 'attachment-chips');
  if (sessionId) chips.append(el('span', 'command-chip', sessionId));
  chips.append(el('span', 'command-chip', exists ? 'present' : 'not present'));
  title.append(chips);
  header.append(title);
  if (directory) {
    const actions = el('div', 'attachment-actions');
    actions.append(iconButton('open', `Open ${directory}`, () => {
      vscode.postMessage({ type: 'openPath', path: directory });
    }));
    actions.append(iconButton('copy', `Copy ${directory}`, () => {
      vscode.postMessage({ type: 'copyText', text: directory });
    }));
    header.append(actions);
  }
  wrap.append(header);
  if (result?.message) wrap.append(el('div', 'command-message', result.message));
  if (directory) wrap.append(el('div', 'attachment-path-row', directory));
  return wrap;
}

function renderNotesBlock(item: TranscriptItem): HTMLElement {
  const result = item.commandResult;
  const notes = Array.isArray(result?.items) ? result.items : [];
  const wrap = el('section', 'command-block attachment-block');
  const header = el('div', 'attachment-header');
  const title = el('div', 'attachment-title');
  title.append(el('span', 'command-title', result?.title ?? 'Session Notes'));
  const chips = el('div', 'attachment-chips');
  if (typeof result?.total === 'number') chips.append(el('span', 'command-chip', `${result.total} total`));
  if (result?.session_id) chips.append(el('span', 'command-chip', result.session_id));
  title.append(chips);
  header.append(title);
  wrap.append(header);
  if (result?.message) wrap.append(el('div', 'command-message', result.message));
  if (notes.length === 0) {
    wrap.append(el('div', 'attachment-placeholder', 'No notes for this session.'));
    return wrap;
  }
  const list = el('div', 'attachment-list');
  notes.forEach((note) => {
    const row = el('div', 'attachment-inventory-row');
    const main = el('div', 'attachment-row-main');
    const top = el('div', 'attachment-path-row');
    const ts = typeof note.ts === 'number' ? note.ts : 0;
    const text = note.text ?? note.detail ?? '';
    top.append(el('span', 'command-row-label', ts > 0 ? `[${ts}]` : 'note'));
    main.append(top);
    if (text) main.append(el('div', 'command-row-detail', text));
    row.append(main);
    if (text) {
      const actions = el('div', 'attachment-actions');
      const copy = iconButton('copy', 'Copy note', () => {
        void markCopied(copy, text);
      });
      actions.append(copy);
      row.append(actions);
    }
    list.append(row);
  });
  wrap.append(list);
  return wrap;
}

function renderSkillsBlock(item: TranscriptItem): HTMLElement {
  const result = item.commandResult;
  const skills = Array.isArray(result?.items) ? result.items : [];
  const archivedList = result?.archived === true;
  const wrap = el('section', 'command-block attachment-block');
  const header = el('div', 'attachment-header');
  const title = el('div', 'attachment-title');
  title.append(el('span', 'command-title', result?.title ?? 'Skills'));
  const chips = el('div', 'attachment-chips');
  chips.append(el('span', 'command-chip', `${result?.total ?? skills.length} ${archivedList ? 'archived' : 'active'}`));
  title.append(chips);
  header.append(title);
  wrap.append(header);
  if (result?.message) wrap.append(el('div', 'command-message', result.message));
  if (skills.length === 0) {
    wrap.append(el('div', 'attachment-placeholder', archivedList ? 'No archived skills in this workspace.' : 'No active skills in this workspace.'));
    return wrap;
  }
  const list = el('div', 'attachment-list');
  skills.forEach((skill) => {
    const row = el('div', 'attachment-inventory-row');
    const main = el('div', 'attachment-row-main');
    const top = el('div', 'attachment-path-row');
    const label = skill.label ?? 'skill';
    const archived = archivedList || skill.archived === true;
    const lastUsed = typeof skill.last_used_at_unix === 'number'
      ? skill.last_used_at_unix
      : skill.lastUsedAtUnix;
    const archivedAt = typeof skill.archived_at_unix === 'number'
      ? skill.archived_at_unix
      : skill.archivedAtUnix;
    top.append(el('span', 'command-row-label', label));
    const meta = [
      skill.scope,
      archived ? 'archived' : '',
      skill.pinned ? 'pinned' : '',
      typeof archivedAt === 'number' && archivedAt > 0
        ? `archived ${archivedAt}`
        : '',
      typeof lastUsed === 'number' && lastUsed > 0
        ? `used ${lastUsed}`
        : '',
    ].filter(Boolean).join(' · ');
    if (meta) top.append(el('span', 'command-row-meta', meta));
    main.append(top);
    if (skill.detail) main.append(el('div', 'command-row-detail', skill.detail));
    row.append(main);
    const actions = el('div', 'attachment-actions');
    const command = String(label);
    const name = command.replace(/^\/+/, '');
    if (archived) {
      actions.append(iconButton('open', `Show ${command}`, () => {
        vscode.postMessage({ type: 'showSkill', name });
      }));
      actions.append(iconButton('restore', `Restore ${command}`, () => {
        vscode.postMessage({ type: 'restoreSkill', name });
      }));
    } else {
      actions.append(iconButton('send', `Use ${command}`, () => {
        vscode.postMessage({ type: 'useSkill', name });
      }));
      actions.append(iconButton('open', `Show ${command}`, () => {
        vscode.postMessage({ type: 'showSkill', name });
      }));
      const pin = iconButton(skill.pinned ? 'unpin' : 'pin', skill.pinned ? `Unpin ${command}` : `Pin ${command}`, () => {
        vscode.postMessage({ type: 'toggleSkillPin', name, pinned: !skill.pinned });
      });
      actions.append(pin);
    }
    const copy = iconButton('copy', `Copy ${command}`, () => {
      void markCopied(copy, command);
    });
    actions.append(copy);
    if (!archived) {
      actions.append(iconButton('archive', `Archive ${command}`, () => {
        vscode.postMessage({ type: 'archiveSkill', name });
      }));
    }
    row.append(actions);
    list.append(row);
  });
  wrap.append(list);
  return wrap;
}

function renderSkillDetailBlock(item: TranscriptItem): HTMLElement {
  const result = item.commandResult;
  const label = result?.label ?? (result?.name ? `/${result.name}` : 'skill');
  const body = typeof result?.body === 'string' ? result.body : '';
  const archived = result?.archived === true;
  const archivedAt = typeof result?.archived_at_unix === 'number'
    ? result.archived_at_unix
    : result?.archivedAtUnix;
  const lastUsed = typeof result?.last_used_at_unix === 'number'
    ? result.last_used_at_unix
    : result?.lastUsedAtUnix;
  const wrap = el('section', 'command-block attachment-block');
  const header = el('div', 'attachment-header');
  const title = el('div', 'attachment-title');
  title.append(el('span', 'command-title', result?.title ?? label));
  const chips = el('div', 'attachment-chips');
  if (result?.scope) chips.append(el('span', 'command-chip', result.scope));
  if (archived) chips.append(el('span', 'command-chip', 'archived'));
  if (result?.pinned) chips.append(el('span', 'command-chip', 'pinned'));
  if (typeof archivedAt === 'number' && archivedAt > 0) {
    chips.append(el('span', 'command-chip', `archived ${archivedAt}`));
  }
  if (typeof lastUsed === 'number' && lastUsed > 0) {
    chips.append(el('span', 'command-chip', `used ${lastUsed}`));
  }
  title.append(chips);
  header.append(title);
  const actions = el('div', 'attachment-actions');
  const command = String(label);
  const name = command.replace(/^\/+/, '');
  if (archived) {
    actions.append(iconButton('restore', `Restore ${command}`, () => {
      vscode.postMessage({ type: 'restoreSkill', name });
    }));
  } else {
    actions.append(iconButton('send', `Use ${command}`, () => {
      vscode.postMessage({ type: 'useSkill', name });
    }));
  }
  const copy = iconButton('copy', `Copy ${command}`, () => {
    void markCopied(copy, command);
  });
  actions.append(copy);
  if (body) {
    const copyBody = iconButton('copy', `Copy ${command} body`, () => {
      void markCopied(copyBody, body);
    });
    actions.append(copyBody);
  }
  if (!archived) {
    actions.append(iconButton('archive', `Archive ${command}`, () => {
      vscode.postMessage({ type: 'archiveSkill', name });
    }));
  }
  header.append(actions);
  wrap.append(header);
  if (result?.detail) wrap.append(el('div', 'command-message', result.detail));
  const pre = el('pre', 'attachment-preview');
  pre.textContent = body.trim() || '(empty skill body)';
  wrap.append(pre);
  return wrap;
}

function attachmentFromResult(result: TranscriptItem['commandResult']): AttachmentView {
  if (result?.attachment) return result.attachment;
  const row = Array.isArray(result?.items)
    ? result.items.find((item) => item.source === 'attachment') ?? result.items[0]
    : undefined;
  return {
    path: row?.path ?? row?.label,
    bytes: row?.bytes,
    media_type: row?.media_type ?? row?.mediaType,
    inlined: row?.inlined,
  };
}

function attachmentsFromResult(result: TranscriptItem['commandResult']): AttachmentView[] {
  if (Array.isArray(result?.attachments)) return result.attachments;
  if (!Array.isArray(result?.items)) return [];
  return result.items
    .filter((item) => item.source === 'attachment')
    .map((item) => ({
      path: item.path ?? item.label,
      bytes: item.bytes,
      media_type: item.media_type ?? item.mediaType,
      inlined: item.inlined,
    }));
}

function previewAttachmentContent(content: string): string {
  const lines = content.split(/\r?\n/);
  const preview = lines.slice(0, 20).join('\n');
  if (lines.length > 20) return `${preview}\n...`;
  return preview;
}

function renderCodeMapBlock(item: TranscriptItem): HTMLElement {
  const result = item.commandResult;
  const rows = Array.isArray(result?.items) ? result.items : [];
  const symbols = rows.filter((row) => row.source === 'symbol');
  const todos = rows.filter((row) => row.source === 'todo');
  const references = rows.filter((row) => row.source === 'reference');
  const wrap = el('section', 'command-block codemap-block');
  const header = el('div', 'codemap-header');
  const title = el('div', 'codemap-title');
  title.append(el('span', 'command-title', result?.title ?? 'Workspace Code Map'));
  const chips = el('div', 'codemap-chips');
  chips.append(el('span', 'command-chip', `${result?.symbol_count ?? symbols.length} symbols`));
  chips.append(el('span', 'command-chip', `${result?.todo_count ?? todos.length} todos`));
  if (references.length > 0 || typeof result?.reference_count === 'number') {
    chips.append(el('span', 'command-chip', `${result?.reference_count ?? references.length} refs`));
  }
  if (typeof result?.walked_files === 'number') {
    chips.append(el('span', 'command-chip', `${result.walked_files} files`));
  }
  if (typeof result?.generated_at_unix === 'number') {
    chips.append(el('span', 'command-chip', result.refreshed ? 'refreshed' : 'cached'));
  }
  if (typeof result?.query === 'string' && result.query.trim().length > 0) {
    chips.append(el('span', 'command-chip', `query: ${result.query.trim()}`));
  }
  title.append(chips);
  header.append(title);
  wrap.append(header);
  if (result?.message) wrap.append(el('div', 'command-message', result.message));

  const controls = el('div', 'codemap-controls');
  const filter = document.createElement('input');
  filter.type = 'search';
  filter.className = 'codemap-filter';
  filter.placeholder = 'Filter symbols, TODOs, or paths';
  filter.setAttribute('aria-label', 'Filter workspace code map');
  controls.append(filter);
  wrap.append(controls);

  const body = el('div', 'codemap-body');
  wrap.append(body);

  const renderRows = () => {
    const query = filter.value.trim().toLowerCase();
    const symbolMatches = symbols.filter((row) => codemapRowMatches(row, query));
    const todoMatches = todos.filter((row) => codemapRowMatches(row, query));
    const referenceMatches = references.filter((row) => codemapRowMatches(row, query));
    body.replaceChildren();
    appendCodeMapGroup(body, 'Symbols', symbolMatches);
    appendCodeMapGroup(body, 'TODO Markers', todoMatches);
    appendCodeMapGroup(body, 'References', referenceMatches);
    if (symbolMatches.length === 0 && todoMatches.length === 0 && referenceMatches.length === 0) {
      body.append(el('div', 'codemap-empty', 'No code map entries match this filter.'));
    }
  };

  filter.addEventListener('input', renderRows);
  renderRows();
  if (result?.truncated) {
    wrap.append(el('div', 'command-footnote', 'further code map entries truncated'));
  }
  return wrap;
}

function renderCodeMapStatusBlock(item: TranscriptItem): HTMLElement {
  const result = item.commandResult;
  const wrap = el('section', `command-block codemap-block ${result?.stale ? 'warning' : ''}`);
  const header = el('div', 'codemap-header');
  const title = el('div', 'codemap-title');
  title.append(el('span', 'command-title', result?.title ?? 'Workspace Code Map Status'));
  const chips = el('div', 'codemap-chips');
  chips.append(el('span', 'command-chip', result?.index_exists ? 'indexed' : 'missing'));
  chips.append(el('span', 'command-chip', result?.stale ? 'stale' : 'fresh'));
  if (typeof result?.source_files === 'number') {
    chips.append(el('span', 'command-chip', `${result.source_files} source files`));
  }
  if (typeof result?.walked_files === 'number') {
    chips.append(el('span', 'command-chip', `${result.walked_files} indexed files`));
  }
  if (typeof result?.symbol_count === 'number') {
    chips.append(el('span', 'command-chip', `${result.symbol_count} symbols`));
  }
  if (typeof result?.todo_count === 'number') {
    chips.append(el('span', 'command-chip', `${result.todo_count} todos`));
  }
  title.append(chips);
  header.append(title);
  wrap.append(header);
  if (result?.message) wrap.append(el('div', 'command-message', result.message));
  if (result?.stale) {
    const action = document.createElement('button');
    action.type = 'button';
    action.className = 'secondary-button';
    action.textContent = 'Refresh index';
    action.addEventListener('click', () => vscode.postMessage({ type: 'refreshCodeMap' }));
    wrap.append(action);
  }
  return wrap;
}

function appendCodeMapGroup(parent: HTMLElement, title: string, rows: CommandResultItem[]): void {
  if (rows.length === 0) return;
  const group = el('section', 'codemap-group');
  group.append(el('div', 'codemap-group-title', `${title} · ${rows.length}`));
  const list = el('div', 'codemap-list');
  rows.forEach((row) => list.append(renderCodeMapRow(row)));
  group.append(list);
  parent.append(group);
}

function renderCodeMapRow(row: CommandResultItem): HTMLElement {
  const line = el('div', `codemap-row codemap-row-${row.source ?? 'entry'}`);
  line.append(el('span', 'codemap-source', codemapSourceLabel(row.source)));
  const main = el('div', 'codemap-row-main');
  const top = el('div', 'codemap-row-top');
  if (row.path) {
    top.append(renderFilePathButton(row.path, 'command-path', row.line, row.column));
  }
  if (row.label && row.label !== row.path) {
    top.append(el('span', 'command-row-label', row.label));
  }
  const meta = [
    typeof row.line === 'number' ? `:${row.line}` : '',
    row.transport,
    typeof row.turn_id === 'number' ? `turn ${row.turn_id}` : '',
  ].filter(Boolean);
  if (meta.length > 0) top.append(el('span', 'command-row-meta', meta.join(' · ')));
  main.append(top);
  if (row.detail) main.append(el('div', 'command-row-detail', row.detail));
  line.append(main);
  return line;
}

function codemapSourceLabel(source?: string): string {
  if (source === 'todo') return 'todo';
  if (source === 'reference') return 'ref';
  return 'symbol';
}

function codemapRowMatches(row: CommandResultItem, query: string): boolean {
  if (!query) return true;
  const haystack = [row.source, row.label, row.detail, row.path, row.transport]
    .filter((value): value is string => typeof value === 'string')
    .join(' ')
    .toLowerCase();
  return haystack.includes(query);
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
  optionsRow.append(renderComposerDocks(s));
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
    composerDrafts.set(lastComposerSessionKey, composerDraft);
    composerHistory.resetNavigation(lastComposerSessionKey);
    persistComposerWebviewState();
    autoresize(textarea);
    updateSlashPicker(textarea, slashPicker);
  });
  textarea.addEventListener('keydown', (event) => {
    if (isSlashPickerOpen(slashPicker)) {
      if (event.key === 'ArrowDown' || event.key === 'ArrowUp') {
        event.preventDefault();
        const itemCount = slashPickerItemCount(textarea.value);
        if (itemCount > 0) {
          slashPickerSelected = Math.max(
            0,
            Math.min(
              itemCount - 1,
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
    if (composerHistoryDirectionForEvent(event)) {
      const direction = event.key === 'ArrowUp' ? 'previous' : 'next';
      if (
        canNavigateComposerHistory(
          textarea.value,
          textarea.selectionStart,
          textarea.selectionEnd,
          direction,
        )
      ) {
        const historyDraft = composerHistory.navigate(
          lastComposerSessionKey,
          direction,
          textarea.value,
        );
        if (historyDraft !== undefined) {
          event.preventDefault();
          textarea.value = historyDraft;
          textarea.selectionStart = textarea.value.length;
          textarea.selectionEnd = textarea.value.length;
          composerDraft = textarea.value;
          composerDrafts.set(lastComposerSessionKey, composerDraft);
          persistComposerWebviewState();
          autoresize(textarea);
          updateSlashPicker(textarea, slashPicker);
          return;
        }
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
    composerHistory.record(lastComposerSessionKey, value);
    persistComposerWebviewState();
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
    composerDrafts.set(lastComposerSessionKey, composerDraft);
    persistComposerWebviewState();
    autoresize(textarea);
  }

  return wrap;
}

interface ComposerWebviewState {
  composerDrafts?: Record<string, string>;
  composerHistory?: ComposerHistorySnapshot;
}

function readComposerWebviewState(raw: unknown): ComposerWebviewState {
  if (!isRecord(raw)) return {};
  const drafts = isRecord(raw.composerDrafts)
    ? Object.fromEntries(
        Object.entries(raw.composerDrafts).filter(
          (entry): entry is [string, string] => typeof entry[1] === 'string',
        ),
      )
    : undefined;
  const history = isRecord(raw.composerHistory)
    ? (raw.composerHistory as ComposerHistorySnapshot)
    : undefined;
  return {
    ...(drafts ? { composerDrafts: drafts } : {}),
    ...(history ? { composerHistory: history } : {}),
  };
}

function persistComposerWebviewState(): void {
  vscode.setState({
    composerDrafts: Object.fromEntries(
      [...composerDrafts.entries()].filter(([, draft]) => draft.length > 0),
    ),
    composerHistory: composerHistory.snapshot(),
  });
}

function composerHistoryDirectionForEvent(
  event: KeyboardEvent,
): ComposerHistoryDirection | undefined {
  if (event.altKey || event.ctrlKey || event.metaKey || event.shiftKey || event.isComposing) {
    return undefined;
  }
  if (event.key === 'ArrowUp') return 'previous';
  if (event.key === 'ArrowDown') return 'next';
  return undefined;
}

function filteredSlashCommands(input: string): SlashCommandSpec[] {
  return filterSlashCommands(input, slashCommands);
}

function slashPickerItemCount(input: string): number {
  return countSlashPickerItems(
    input,
    slashCommands,
    state?.sessions ?? [],
    state?.context.mcpServers ?? [],
    state?.context.modelSuggestions ?? [],
    state?.context.branchSnapshots ?? [],
  );
}

function updateSlashPicker(textarea: HTMLTextAreaElement, picker: HTMLElement): void {
  const argumentContext = slashArgumentContext(textarea.value);
  if (argumentContext) {
    renderSlashArgumentOptions(textarea, picker, argumentContext);
    return;
  }

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

function renderSlashArgumentOptions(
  textarea: HTMLTextAreaElement,
  picker: HTMLElement,
  context: SlashArgumentContext,
): void {
  if (context.options.length === 0) {
    slashPickerSelected = 0;
    picker.classList.add('hidden');
    picker.replaceChildren();
    return;
  }
  slashPickerSelected = Math.min(slashPickerSelected, context.options.length - 1);
  picker.classList.remove('hidden');
  picker.replaceChildren();
  const start = Math.min(
    Math.max(0, slashPickerSelected - 5),
    Math.max(0, context.options.length - 6),
  );
  context.options.slice(start, start + 6).forEach((option, offset) => {
    const index = start + offset;
    const row = el('button', `slash-option${index === slashPickerSelected ? ' selected' : ''}`);
    row.type = 'button';
    row.addEventListener('mousedown', (event) => {
      event.preventDefault();
      slashPickerSelected = index;
      acceptSlashSelection(textarea, picker);
    });
    row.append(el('span', 'slash-name', option));
    row.append(el('span', 'slash-description', context.command.name));
    picker.append(row);
  });
}

function isSlashPickerOpen(picker: HTMLElement): boolean {
  return !picker.classList.contains('hidden');
}

function acceptSlashSelection(textarea: HTMLTextAreaElement, picker: HTMLElement): void {
  const argumentContext = slashArgumentContext(textarea.value);
  if (argumentContext) {
    const optionIndex = Math.min(slashPickerSelected, argumentContext.options.length - 1);
    const option = argumentContext.options[optionIndex];
    if (!option) return;
    textarea.value = `${argumentContext.command.name} ${option}${argumentContext.appendSpace ? ' ' : ''}`;
    textarea.selectionStart = textarea.value.length;
    textarea.selectionEnd = textarea.value.length;
    composerDraft = textarea.value;
    autoresize(textarea);
    picker.classList.add('hidden');
    picker.replaceChildren();
    textarea.focus();
    return;
  }

  const matches = filteredSlashCommands(textarea.value);
  const command = matches[slashPickerSelected];
  if (!command) return;
  if (command.argHint) slashPickerSelected = 0;
  textarea.value = acceptedSlashCommandText(command);
  textarea.selectionStart = textarea.value.length;
  textarea.selectionEnd = textarea.value.length;
  composerDraft = textarea.value;
  autoresize(textarea);
  updateSlashPicker(textarea, picker);
  textarea.focus();
}

function slashExactSelectionIsRunnable(input: string): boolean {
  return isSlashExactSelectionRunnable(
    input,
    slashCommands,
    slashPickerSelected,
    state?.sessions ?? [],
    state?.context.mcpServers ?? [],
    state?.context.modelSuggestions ?? [],
    state?.context.branchSnapshots ?? [],
  );
}

function slashArgumentContext(input: string): SlashArgumentContext | undefined {
  return resolveSlashArgumentContext(
    input,
    slashCommands,
    state?.sessions ?? [],
    state?.context.mcpServers ?? [],
    state?.context.modelSuggestions ?? [],
    state?.context.branchSnapshots ?? [],
  );
}

function modeSelect(opts: RunOptions): HTMLSelectElement {
  const current = composerModeOverride ?? opts.mode;
  return selectControl(
    'composer-select',
    'Execution mode',
    [
      ['execute', 'Execute'],
      ['plan', 'Plan'],
      ['goal', 'Goal'],
    ] as const,
    current,
    'composer-mode',
  );
}

function permissionSelect(opts: RunOptions): HTMLSelectElement {
  const current = composerPermissionOverride ?? opts.permission;
  return selectControl(
    'composer-select',
    'Permission',
    [
      ['auto', 'Auto'],
      ['safe', 'Safe'],
      ['yolo', 'Yolo'],
    ] as const,
    current,
    'composer-permission',
  );
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
  return selectControl(
    'composer-select composer-model composer-model-select',
    'ChatGPT model',
    CHATGPT_MODELS.map((model) => [model, model] as const),
    current,
    'composer-model',
  );
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
