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
  // Tool grouping: tool_started/finished are folded into a single transcript
  // entry. `pending` is true between started and finished.
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

export interface SidebarState {
  running: boolean;
  sessionId?: string;
  status: string;
  context: SidebarContext;
  transcript: TranscriptItem[];
  runOptions: RunOptions;
  hud: HudState;
}

/** Messages the webview sends to the extension host. */
export type OutboundMessage =
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
  | { type: 'openFile'; path: string };

/** Messages the extension host posts back to the webview. */
export type InboundMessage = { type: 'state'; state: SidebarState };
