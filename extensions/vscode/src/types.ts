// Types shared between the extension host (src/) and the webview bundle
// (webview/). Anything that crosses the postMessage boundary lives here so
// the two bundles stay in lockstep.

export type Mode = 'execute' | 'plan' | 'goal';
export type Permission = 'auto' | 'safe' | 'yolo';

export interface RunOptions {
  mode: Mode;
  permission: Permission;
  model?: string;
}

export type AskUserAnswer =
  | { kind: 'selected'; index: number; text: string }
  | { kind: 'multi_selected'; indices: number[] }
  | { kind: 'text'; text: string }
  | { kind: 'cancelled' };

export type ApprovalScope = 'once' | 'session' | 'command' | 'path';

export interface ApprovalResponse {
  approved: boolean;
  scope: ApprovalScope;
  toolName?: string;
  reason?: string;
  parameters?: unknown;
}

export type TranscriptRole =
  | 'user'
  | 'assistant'
  | 'tool'
  | 'status'
  | 'error'
  | 'interaction'
  | 'diff'
  | 'approval';

export interface TranscriptItem {
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
  pending?: boolean;
  toolParameters?: unknown;
  toolResultSummary?: string;
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

export interface UsageSlice {
  inputTokens: number;
  outputTokens: number;
  cacheReadTokens?: number;
  cacheCreationTokens?: number;
  costUsd?: number;
}

export interface BudgetSlice {
  costUsed: number;
  costLimit?: number;
  turnsUsed: number;
  turnsLimit?: number;
}

export interface ContextSlice {
  tokensUsed: number;
  threshold: number;
}

export interface PlanStepView {
  text: string;
  status?: string;
}

export interface PlanSlice {
  steps: PlanStepView[];
  current?: number;
}

export interface CommitteeRoleSlice {
  tokens: number;
  costUsd: number;
}

export interface HudState {
  usage?: UsageSlice;
  budget?: BudgetSlice;
  context?: ContextSlice;
  plan?: PlanSlice;
  committee?: Record<string, CommitteeRoleSlice>;
}

export interface QueuedMessage {
  id: string;
  text: string;
}

export interface ChatSessionSummary {
  id: string;
  title: string;
  status: string;
  running: boolean;
  active: boolean;
}

export type SidebarView = 'landing' | 'session';
export type LandingScreen = 'home' | 'openrouter' | 'localLlm' | 'claude' | 'openai';

export interface SidebarState {
  view: SidebarView;
  landing: LandingScreen;
  running: boolean;
  activeChatId?: string;
  sessionId?: string;
  status: string;
  context: SidebarContext;
  transcript: TranscriptItem[];
  sessions: ChatSessionSummary[];
  queue: QueuedMessage[];
  runOptions: RunOptions;
  hud: HudState;
  authBusy: boolean;
  authError?: string;
}

/** Identifier of the provider the user opts into from the landing UI. */
export type ProviderChoice = 'chatgpt' | 'openrouter' | 'localLlm' | 'claude' | 'openai';

/** Messages the webview sends to the extension host. */
export type OutboundMessage =
  | { type: 'ready' }
  | { type: 'run'; task: string; options: RunOptions }
  | { type: 'cancel' }
  | { type: 'loginOpenAi' }
  | { type: 'refreshStatus' }
  | { type: 'askUserRespond'; requestId: string; answer: AskUserAnswer }
  | {
      type: 'approvalRespond';
      approved: boolean;
      scope: ApprovalScope;
      toolName?: string;
      reason?: string;
      parameters?: unknown;
    }
  | { type: 'openFile'; path: string }
  | { type: 'registerProvider'; provider: ProviderChoice; params: Record<string, string> }
  | { type: 'showLanding'; screen?: LandingScreen }
  | { type: 'showSession' }
  | { type: 'newSession' }
  | { type: 'selectSession'; id: string }
  | { type: 'queueAdd'; task: string }
  | { type: 'queueRemove'; id: string }
  | { type: 'queueEdit'; id: string; text: string }
  | { type: 'queueClear' };

/** Messages the extension host posts back to the webview. */
export type InboundMessage = { type: 'state'; state: SidebarState };
