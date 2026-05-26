// Types shared between the extension host (src/) and the webview bundle
// (webview/). Anything that crosses the postMessage boundary lives here so
// the two bundles stay in lockstep.

export type Mode = 'execute' | 'plan' | 'goal';
export type Permission = 'auto' | 'safe' | 'yolo';
export type ReasoningEffort = 'off' | 'low' | 'medium' | 'high' | 'xhigh';
export type ServiceTier = 'standard' | 'fast';

export interface RunOptions {
  mode: Mode;
  permission: Permission;
  model?: string;
  reasoningEffort?: ReasoningEffort;
  serviceTier?: ServiceTier;
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
  sessionId?: string;
}

export type TranscriptRole =
  | 'user'
  | 'assistant'
  | 'tool'
  | 'status'
  | 'error'
  | 'interaction'
  | 'thinking'
  | 'diff'
  | 'command'
  | 'approval';

export interface CommandResultItem {
  label?: string;
  detail?: string;
  path?: string;
  line?: number;
  column?: number;
  tokens?: number;
  turn_id?: number;
  source?: string;
  transport?: string;
}

export interface SlashStateDeltaView {
  mode?: Mode;
  permission?: Permission;
  model?: string;
  provider?: string;
  reasoning_effort?: ReasoningEffort | string;
  reasoningEffort?: ReasoningEffort | string;
  service_tier?: ServiceTier | string | null;
  serviceTier?: ServiceTier | string | null;
  committee_mode?: string;
  committeeMode?: string;
  locale?: string;
  subagent_default_model?: string | null;
  subagentDefaultModel?: string | null;
}

export interface CommandResultView {
  kind?: string;
  title?: string;
  message?: string;
  severity?: 'info' | 'error';
  command?: string;
  action?: string;
  task?: string;
  label?: string;
  diff?: string;
  items?: CommandResultItem[];
  source_totals?: Record<string, number>;
  truncated?: boolean;
  state_delta?: SlashStateDeltaView;
  stateDelta?: SlashStateDeltaView;
}

export interface SlashCommandSpec {
  name: string;
  description: string;
  argHint?: string;
  category?: string;
}

export interface TranscriptItem {
  role: TranscriptRole;
  text: string;
  detail?: string;
  commandResult?: CommandResultView;
  requestId?: string;
  request?: unknown;
  path?: string;
  line?: number;
  column?: number;
  before?: string | null;
  after?: string;
  toolName?: string;
  reason?: string;
  parameters?: unknown;
  approvalSessionId?: string;
  pending?: boolean;
  toolParameters?: unknown;
  toolResultSummary?: string;
  /**
   * Tool risk class label forwarded from the daemon's `tool_started`
   * event (`AgentRunEvent::ToolStarted::risk_class`). Stable strings
   * matching the Rust [`RiskClass`] variants in `peridot-common`:
   * `read_only`, `local_write`, `build_or_test`, `external_network`,
   * `destructive`, `secret_adjacent`. The webview renders this as a
   * coloured chip next to the tool name; missing/unknown values fall
   * back to no chip rather than blocking the display.
   */
  riskClass?: string;
}

export interface SidebarContext {
  workspace?: string;
  provider?: string;
  model?: string;
  mode?: string;
  permission?: string;
  reasoningEffort?: string;
  serviceTier?: string;
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
  contextTokens?: number;
  messageTokens?: number;
  systemTokens?: number;
  toolSchemaTokens?: number;
  overheadTokens?: number;
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
  slashCommands: SlashCommandSpec[];
  branchPicker?: CommandResultView;
  pendingApproval?: TranscriptItem;
  authBusy: boolean;
  authError?: string;
  phase?: string;
  runStartedAtMs?: number;
  lastRunElapsedMs?: number;
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
      sessionId?: string;
    }
  | { type: 'openFile'; path: string; line?: number; column?: number }
  | { type: 'registerProvider'; provider: ProviderChoice; params: Record<string, string> }
  | { type: 'showLanding'; screen?: LandingScreen }
  | { type: 'showSession' }
  | { type: 'newSession' }
  | { type: 'selectSession'; id: string }
  | { type: 'renameSession'; id: string; title: string }
  | { type: 'deleteSession'; id: string }
  | { type: 'copyText'; text: string }
  | { type: 'queueAdd'; task: string }
  | { type: 'queueRemove'; id: string }
  | { type: 'queueEdit'; id: string; text: string }
  | { type: 'queueClear' }
  | { type: 'dismissBranchPicker' }
  | { type: 'openSettings' };

/** Messages the extension host posts back to the webview. */
export type InboundMessage = { type: 'state'; state: SidebarState };
