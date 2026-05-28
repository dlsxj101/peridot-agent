import * as vscode from 'vscode';
import {
  ApprovalResponse,
  AskUserAnswer,
  BudgetSlice,
  ChatSessionSummary,
  CommitteeRoleSlice,
  CompactionSnapshotView,
  CommandResultView,
  ContextSlice,
  DaemonSessionSummary,
  HudState,
  InboundMessage,
  OutboundMessage,
  PlanSlice,
  PlanStepView,
  ProviderChoice,
  QueuedMessage,
  RunOptions,
  SidebarContext,
  SidebarState,
  SlashCommandSpec,
  SlashStateDeltaView,
  TranscriptItem,
  UsageSlice,
} from './types';
import { localSlashAction } from './localSlashAction';
import { staleDaemonBackedSessionIds } from './sessionReconcile';
import {
  agentTranscriptItemForEvent,
  shouldSuppressAgentEventFallback,
} from './agentEventTranscript';
import {
  isAskUserWaitingEvent,
  isTerminalAgentEvent,
  terminalStatusForEvent,
} from './agentEventLifecycle';
import { agentsSummaryForLoadedEvent, mcpServersForStatusEvent } from './agentEventContext';

export type {
  ApprovalResponse,
  ApprovalScope,
  AskUserAnswer,
  DaemonSessionSummary,
  ProviderChoice,
  RunOptions,
} from './types';

export interface SidebarHandlers {
  runTask: (task: string, options: RunOptions) => Promise<void>;
  runSlashCommand: (command: string, options: RunOptions) => Promise<CommandResultView>;
  cancelTask: () => Promise<void>;
  clearSession: (options?: { skipDaemonCancel?: boolean }) => Promise<void>;
  loginOpenAi: () => Promise<void>;
  refreshStatus: () => Promise<void>;
  refreshSlashCatalog: () => Promise<void>;
  showCodeMap: () => Promise<void>;
  showCodeMapStatus: () => Promise<void>;
  refreshCodeMap: () => Promise<void>;
  searchCodeMap: () => Promise<void>;
  outlineCurrentFile: () => Promise<void>;
  findSymbolReferences: () => Promise<void>;
  showSkills: () => Promise<void>;
  showArchivedSkills: () => Promise<void>;
  searchSkills: () => Promise<void>;
  searchArchivedSkills: () => Promise<void>;
  showSkill: (name: string) => Promise<void>;
  useSkill: (name: string) => Promise<void>;
  toggleSkillPin: (name: string, pinned: boolean) => Promise<void>;
  archiveSkill: (name: string) => Promise<void>;
  restoreSkill: (name: string) => Promise<void>;
  attachFile: () => Promise<void>;
  detachAttachment: (path: string) => Promise<void>;
  showAttachments: () => Promise<void>;
  showSessions: () => Promise<void>;
  pruneSessions: () => Promise<void>;
  replaySessionTimeline: () => Promise<void>;
  exportSessionArtifacts: () => Promise<void>;
  importSessionArtifacts: () => Promise<void>;
  showPrStatus: () => Promise<void>;
  shipChanges: () => Promise<void>;
  mergePr: () => Promise<void>;
  respondAskUser: (requestId: string, answer: AskUserAnswer) => Promise<boolean>;
  respondApproval: (decision: ApprovalResponse) => Promise<void>;
  openFile: (relativePath: string, line?: number, column?: number, projectRoot?: string) => Promise<void>;
  openPath: (path: string) => Promise<void>;
  registerProvider: (provider: ProviderChoice, params: Record<string, string>) => Promise<void>;
  deleteSession: (clientSessionId: string, daemonSessionId?: string) => Promise<void>;
  finishDaemonSession: (daemonSessionId: string) => Promise<void>;
  copyText: (text: string) => Promise<void>;
  /**
   * Ask the daemon to LLM-generate a short title for a session from its first
   * task. Resolves to `null` if the provider call fails or returns empty — the
   * sidebar then surfaces `"No title"` rather than the raw truncated task.
   */
  generateSessionTitle: (task: string) => Promise<string | null>;
}

interface DaemonEventParams {
  session_id?: string;
  event?: unknown;
}

interface StoredChatSession {
  id: string;
  title: string;
  daemonSessionId?: string;
  status: string;
  running: boolean;
  transcript: TranscriptItem[];
  hud: HudState;
  runOptions: RunOptions;
  pendingApproval?: TranscriptItem;
  runStartedAtMs?: number;
  lastRunElapsedMs?: number;
  /**
   * True once the user has manually renamed this session (via the
   * session-menu rename action or `/session rename`). The async LLM
   * title-generation path must not overwrite a user-chosen title.
   */
  userRenamed?: boolean;
}

interface PersistedSidebarSnapshot {
  version: 1;
  activeChatId?: string;
  nextSessionOrdinal: number;
  runOptions: RunOptions;
  context: SidebarContext;
  view: SidebarState['view'];
  landing: SidebarState['landing'];
  queue: QueuedMessage[];
  sessions: StoredChatSession[];
}

const PERSISTENCE_KEY = 'peridot.sidebarState.v1';

export interface PreparedTask {
  clientSessionId: string;
  continueSessionId?: string;
}

export class PeridotSidebarProvider implements vscode.WebviewViewProvider {
  public static readonly viewType = 'peridot.chatView';

  private view: vscode.WebviewView | undefined;
  private state: SidebarState = freshState();
  private sessions = new Map<string, StoredChatSession>();
  private nextSessionOrdinal = 1;
  private streamCoalesceTimer: ReturnType<typeof setTimeout> | undefined;
  private streamCoalescePending = false;
  private persistTimer: ReturnType<typeof setTimeout> | undefined;

  public constructor(
    private readonly extensionUri: vscode.Uri,
    private readonly storage: vscode.Memento,
    private readonly handlers: SidebarHandlers,
  ) {
    this.restorePersistedState();
  }

  public resolveWebviewView(webviewView: vscode.WebviewView): void {
    this.view = webviewView;
    webviewView.webview.options = {
      enableScripts: true,
      localResourceRoots: [
        vscode.Uri.joinPath(this.extensionUri, 'dist'),
        vscode.Uri.joinPath(this.extensionUri, 'resources'),
      ],
    };
    webviewView.webview.html = this.html(webviewView.webview);
    webviewView.onDidDispose(() => {
      if (this.view === webviewView) this.view = undefined;
    });
    webviewView.webview.onDidReceiveMessage((message: OutboundMessage) => {
      void this.receive(message);
    });
    this.publish();
  }

  public prepareForTask(task: string, workspace: string): PreparedTask {
    const session = this.ensureActiveSession();
    const continueSessionId = session.daemonSessionId;
    // Placeholder title: show the truncated task immediately so the user gets
    // visual feedback before the async LLM title call resolves. The real
    // title (or `"No title"` on failure) lands later via `applyGeneratedTitle`.
    if (session.title.startsWith('New session') && !session.userRenamed) {
      session.title = taskTitle(task);
      // Kick off LLM title generation in the background. We don't await —
      // session.start latency must not depend on a title round-trip.
      const targetId = session.id;
      void this.handlers
        .generateSessionTitle(task)
        .then((title) => this.applyGeneratedTitle(targetId, title))
        .catch((err: unknown) => {
          const message = err instanceof Error ? err.message : String(err);
          console.warn('[peridot] session.generate_title failed:', message);
          this.applyGeneratedTitle(targetId, null);
        });
    }
    this.state.view = 'session';
    this.state.context = {
      ...this.state.context,
      workspace,
      status: 'Starting daemon',
      problem: undefined,
      running: true,
    };
    this.state.running = true;
    this.state.status = 'Starting daemon';
    this.state.runStartedAtMs = Date.now();
    this.state.lastRunElapsedMs = undefined;
    this.state.pendingApproval = undefined;
    this.state.transcript.push({ role: 'user', text: task });
    this.publish();
    return {
      clientSessionId: session.id,
      continueSessionId,
    };
  }

  public setSession(sessionId: string): void {
    const clientSessionId = this.state.activeChatId;
    if (clientSessionId) {
      this.setSessionFor(clientSessionId, sessionId);
    }
  }

  public setSessionFor(clientSessionId: string, sessionId: string): void {
    const previousActive = this.state.activeChatId;
    const temporarilyLoaded = clientSessionId !== previousActive && this.sessions.has(clientSessionId);
    if (temporarilyLoaded) {
      this.saveActiveSession();
      this.loadSessionIntoState(clientSessionId, false);
    }
    this.state.sessionId = sessionId;
    this.state.status = `Running ${sessionId}`;
    this.state.context = { ...this.state.context, status: 'Running', running: true };
    const session = this.activeStoredSession();
    if (session) session.daemonSessionId = sessionId;
    if (temporarilyLoaded) {
      this.saveActiveSession();
      if (previousActive) this.loadSessionIntoState(previousActive, false);
      else this.loadDraftSessionIntoState();
    }
    this.publish();
  }

  public appendNotificationFor(
    clientSessionId: string | undefined,
    params: DaemonEventParams,
  ): void {
    const previousActive = this.state.activeChatId;
    const temporarilyLoaded = Boolean(
      clientSessionId && clientSessionId !== previousActive && this.sessions.has(clientSessionId),
    );
    if (temporarilyLoaded) {
      this.saveActiveSession();
      this.loadSessionIntoState(clientSessionId, false);
    }
    const event = params.event;
    if (!isRecord(event)) {
      this.append({ role: 'status', text: 'Event' });
      if (temporarilyLoaded) {
        this.saveActiveSession();
        if (previousActive) this.loadSessionIntoState(previousActive, false);
        else this.loadDraftSessionIntoState();
        this.publish();
      }
      return;
    }
    const kind = typeof event.kind === 'string' ? event.kind : '';

    this.applyHudSideEffects(kind, event);

    const approvalPayload =
      approvalPayloadForEvent(event) ??
      (kind === 'approval_waiting' ? this.approvalPayloadFromPendingTool() : undefined);

    if (kind === 'run_started') {
      this.state.status = 'Running';
      this.state.context = { ...this.state.context, status: this.state.status, running: true };
      this.state.running = true;
      this.publish();
    } else if (kind === 'assistant_started') {
      this.state.status = 'Waiting for model';
      this.state.context = { ...this.state.context, status: this.state.status, running: true };
      this.publish();
    } else if (kind === 'assistant_delta') {
      const item = transcriptItemForEvent(kind, event);
      if (item) this.append(item);
    } else if (kind === 'assistant_finished') {
      this.state.status = 'Running';
      this.state.context = { ...this.state.context, status: this.state.status, running: true };
      this.publish();
    } else if (kind === 'tool_started') {
      this.appendToolStarted(event);
    } else if (kind === 'tool_finished') {
      this.appendToolFinished(event);
    } else if (kind === 'error') {
      const markedFailed = this.markLatestPendingToolFailed(stringField(event, 'message'));
      const item = transcriptItemForEvent(kind, event);
      if (item) {
        this.append(item);
      } else if (markedFailed) {
        this.publish();
      }
    } else if (kind === 'recovery') {
      // Recovery events are still written to the VS Code Output channel by
      // the notification handler. Keep them out of the user transcript.
      this.publish();
    } else if (kind === 'phase_changed') {
      const to = stringField(event, 'to') ?? 'unknown';
      // Phase transitions are high-volume internal state-machine signals.
      // Keep the header chip current, but do not add them to the transcript.
      this.state.phase = displayPhaseLabel(to);
      this.publish();
    } else if (kind === 'context_compacted') {
      const compacted = isRecord(event.compacted) ? event.compacted : {};
      const snapshot = compactionSnapshotView(compacted);
      const summary = [
        `${snapshot.filesRead.length} files read`,
        `${snapshot.filesChanged.length} changed`,
        `${snapshot.decisions.length} decisions`,
        `${snapshot.untrustedInputs.length} untrusted`,
      ].join(', ');
      this.append({
        role: 'status',
        text: `Context compacted (${summary})`,
        compaction: snapshot,
      });
    } else if (approvalPayload) {
      this.appendApproval(approvalPayload, params.session_id);
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
    if (isAskUserWaitingEvent(event)) {
      this.state.running = true;
      this.state.status = 'Waiting for user response';
      this.state.context = {
        ...this.state.context,
        status: this.state.status,
        running: true,
      };
      this.publish();
    }
    if (kind === 'approval_resumed' || kind === 'approval_denied') {
      this.state.pendingApproval = undefined;
      this.publish();
    }
    if (isTerminalAgentEvent(event)) {
      const status = terminalStatusForEvent(event);
      this.state.pendingApproval = undefined;
      this.finishRunTimer(status);
      this.state.running = false;
      this.state.status = status;
      this.state.context = {
        ...this.state.context,
        status: this.state.status,
        running: false,
      };
      this.publish();
      // Session ended — flush any throttled persistence immediately so the
      // final transcript is safely written to disk.
      this.flushPersist();
    }
    if (temporarilyLoaded) {
      this.saveActiveSession();
      if (previousActive) this.loadSessionIntoState(previousActive, false);
      else this.loadDraftSessionIntoState();
      this.publish();
    }
  }

  public appendSystem(text: string, detail?: string): void {
    this.append({ role: 'status', text, detail });
  }

  public appendError(text: string): void {
    this.finishRunTimer();
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
    if (this.state.running) this.finishRunTimer();
    this.state.running = false;
    this.state.status = status;
    this.state.context = { ...this.state.context, status, running: false };
    this.publish();
  }

  public setContext(context: SidebarContext): void {
    const reasoningEffort = normalizeReasoning(context.reasoningEffort);
    const currentOptions = this.state.runOptions;
    this.state.context = {
      ...this.state.context,
      ...context,
      mode: currentOptions.mode,
      permission: currentOptions.permission,
      model: currentOptions.model ?? context.model ?? this.state.context.model,
      reasoningEffort: currentOptions.reasoningEffort ?? reasoningEffort,
      serviceTier:
        currentOptions.serviceTier ?? (context.serviceTier as RunOptions['serviceTier'] | undefined),
    };
    this.state.running = Boolean(context.running ?? this.state.running);
    if (context.status) {
      this.state.status = context.status;
    }
    // The landing screen flips to session as soon as we know auth is
    // configured; if it's not configured (or we don't know yet), keep the
    // user on landing so they can pick an auth method.
    if (context.authConfigured === true && this.state.view === 'landing') {
      this.state.view = 'session';
      this.state.landing = 'home';
    } else if (
      context.authConfigured === false &&
      !this.state.running &&
      this.state.transcript.length === 0
    ) {
      this.state.view = 'landing';
    }
    this.publish();
  }

  public setSlashCommands(commands: SlashCommandSpec[]): void {
    this.state.slashCommands = commands;
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

  public setAuthBusy(busy: boolean, error?: string): void {
    this.state.authBusy = busy;
    if (error !== undefined) this.state.authError = error;
    this.publish();
  }

  /** Pulls the head of the queue and signals the host to run it. */
  public takeNextQueued(): QueuedMessage | undefined {
    const next = this.state.queue.shift();
    if (next) this.publish();
    return next;
  }

  public hasQueue(): boolean {
    return this.state.queue.length > 0;
  }

  public currentRunOptions(): RunOptions {
    return { ...this.state.runOptions };
  }

  public currentDaemonSessionId(): string | undefined {
    return this.state.sessionId;
  }

  public currentClientSessionId(): string | undefined {
    return this.state.activeChatId;
  }

  public markSessionFailed(clientSessionId: string, message: string): void {
    const previousActive = this.state.activeChatId;
    const temporarilyLoaded = clientSessionId !== previousActive && this.sessions.has(clientSessionId);
    if (temporarilyLoaded) {
      this.saveActiveSession();
      this.loadSessionIntoState(clientSessionId, false);
    }
    if (temporarilyLoaded) {
      this.state.running = false;
      this.state.status = 'Failed';
      this.finishRunTimer();
      this.state.context = {
        ...this.state.context,
        status: 'Failed',
        problem: message,
        running: false,
      };
      this.state.transcript.push({ role: 'error', text: message });
      this.saveActiveSession();
      if (previousActive) this.loadSessionIntoState(previousActive, false);
      else this.loadDraftSessionIntoState();
      this.publish();
    } else {
      this.appendError(message);
    }
  }

  public appendCommandResult(result: CommandResultView): void {
    if (result.kind === 'branch_picker') {
      this.state.branchPicker = result;
    }
    this.append({
      role: result.severity === 'error' ? 'error' : 'command',
      text: result.message ?? result.title ?? result.kind ?? 'Command',
      commandResult: result,
    });
  }

  public appendAuthLink(url: string): void {
    this.append({
      role: 'assistant',
      text: [
        'ChatGPT 로그인 브라우저가 자동으로 열리지 않으면 아래 링크를 열어주세요.',
        '',
        `[Sign in with ChatGPT](${url})`,
        '',
        url,
      ].join('\n'),
    });
  }

  public createNewSession(workspace?: string): void {
    this.saveActiveSession();
    this.state.phase = undefined;
    this.loadDraftSessionIntoState(workspace);
    this.publish();
  }

  public selectSession(id: string): void {
    this.saveActiveSession();
    this.loadSessionIntoState(id, true);
  }

  public renameSession(id: string, title: string): boolean {
    const nextTitle = title.trim();
    const session = this.sessions.get(id);
    if (!session || nextTitle.length === 0) return false;
    session.title = nextTitle;
    // Mark as user-chosen so the async LLM title-generation handler doesn't
    // overwrite it on a later turn.
    session.userRenamed = true;
    this.publish();
    return true;
  }

  /**
   * Apply an LLM-generated title to the session. Only takes effect if the
   * user has not manually renamed the session in the meantime — that
   * invariant is the whole point of the `userRenamed` flag.
   *
   * Called by the extension after `session.generate_title` resolves. Pass
   * `null` to indicate the LLM call failed; the session title falls back to
   * `"No title"`, but again only if the user hasn't taken over the name.
   */
  public applyGeneratedTitle(clientSessionId: string, title: string | null): void {
    const session = this.sessions.get(clientSessionId);
    if (!session) return;
    if (session.userRenamed) return;
    const cleaned = title?.trim();
    session.title = cleaned && cleaned.length > 0 ? cleaned : 'No title';
    this.publish();
  }

  public deleteSession(id: string): boolean {
    const session = this.sessions.get(id);
    if (!session) return false;
    const wasActive = session.id === this.state.activeChatId;
    this.sessions.delete(session.id);
    if (wasActive) {
      const next = this.sessions.values().next().value as StoredChatSession | undefined;
      if (next) {
        this.loadSessionIntoState(next.id, false);
      } else {
        this.loadDraftSessionIntoState(this.state.context.workspace);
      }
    }
    this.publish();
    return true;
  }

  public reconcileDaemonSessions(
    remoteSessions: DaemonSessionSummary[],
    options: { pruneMissing?: boolean } = {},
  ): void {
    if (remoteSessions.length === 0 && options.pruneMissing !== true) return;
    this.saveActiveSession();
    const byDaemonId = new Map<string, StoredChatSession>();
    for (const session of this.sessions.values()) {
      if (session.daemonSessionId) {
        byDaemonId.set(session.daemonSessionId, session);
      }
    }

    for (const remote of remoteSessions) {
      const daemonId = remote.id?.trim();
      if (!daemonId) continue;
      let session = byDaemonId.get(daemonId) ?? this.sessions.get(daemonId);
      if (!session) {
        session = this.createSession(remoteSessionTitle(remote));
        this.sessions.delete(session.id);
        session.id = daemonId;
        this.sessions.set(session.id, session);
      } else if (!session.userRenamed) {
        session.title = remoteSessionTitle(remote);
      }
      session.daemonSessionId = daemonId;
      session.status = remoteSessionStatus(remote);
      session.running = Boolean(remote.running);
      if (session.running && typeof session.runStartedAtMs !== 'number') {
        session.runStartedAtMs = Date.now();
      }
      if (!session.running) {
        session.runStartedAtMs = undefined;
      }
    }

    if (options.pruneMissing === true) {
      for (const id of staleDaemonBackedSessionIds(this.sessions.values(), remoteSessions)) {
        this.sessions.delete(id);
      }
    }

    if (this.state.activeChatId && this.sessions.has(this.state.activeChatId)) {
      this.loadSessionIntoState(this.state.activeChatId, false);
    } else if (this.state.activeChatId) {
      const next = this.sessions.values().next().value as StoredChatSession | undefined;
      if (next) {
        this.loadSessionIntoState(next.id, false);
      } else {
        this.loadDraftSessionIntoState(this.state.context.workspace);
      }
    }
    this.publish();
  }

  private appendToolStarted(event: Record<string, unknown>): void {
    const name = stringField(event, 'name');
    if (name === 'agent_done') {
      return;
    }
    // `risk_class` is optional on the wire — older daemons (pre-#7) omit it.
    // The webview chip renderer treats `undefined` as "no chip".
    const riskClass = pickString(event, 'risk_class');
    if (name === 'agent_ask_user') {
      this.append({
        role: 'tool',
        text: name,
        detail: compactAskUserToolDetail(event.parameters),
        toolName: name,
        pending: true,
        riskClass,
      });
      return;
    }
    this.append({
      role: 'tool',
      text: name,
      detail: undefined,
      toolName: name,
      path: pickString(event.parameters, 'path'),
      line: pickNumber(event.parameters, 'line') ?? pickNumber(event.parameters, 'start_line'),
      column: pickNumber(event.parameters, 'column'),
      toolParameters: event.parameters,
      pending: true,
      riskClass,
    });
  }

  private appendToolFinished(event: Record<string, unknown>): void {
    const name = stringField(event, 'name');
    const summary = summarizeToolResult(event);
    if (name === 'agent_done') {
      this.removePendingTool(name);
      if (summary.trim().length > 0 && !this.lastAssistantTextMatches(summary)) {
        this.append({ role: 'assistant', text: summary });
      } else {
        this.publish();
      }
      return;
    }
    for (let i = this.state.transcript.length - 1; i >= 0; i -= 1) {
      const item = this.state.transcript[i];
      if (item.role === 'tool' && item.pending && (item.toolName === name || !item.toolName)) {
        item.pending = false;
        item.text = name;
        item.toolResultSummary = summary;
        item.detail = undefined;
        this.publish();
        return;
      }
    }
    this.append({
      role: 'tool',
      text: name,
      detail: summary,
      toolName: name,
      pending: false,
      toolResultSummary: summary,
    });
  }

  private removePendingTool(toolName: string): void {
    for (let i = this.state.transcript.length - 1; i >= 0; i -= 1) {
      const item = this.state.transcript[i];
      if (item.role === 'tool' && item.pending && item.toolName === toolName) {
        this.state.transcript.splice(i, 1);
        return;
      }
    }
  }

  private markLatestPendingToolFailed(message: string): boolean {
    for (let i = this.state.transcript.length - 1; i >= 0; i -= 1) {
      const item = this.state.transcript[i];
      if (item.role === 'tool' && item.pending) {
        item.pending = false;
        item.toolResultSummary = compactToolFailureSummary(message);
        item.detail = undefined;
        return true;
      }
    }
    return false;
  }

  private lastAssistantTextMatches(candidate: string): boolean {
    const normalized = candidate.trim();
    const lastAssistant = [...this.state.transcript]
      .reverse()
      .find((item) => item.role === 'assistant');
    return lastAssistant?.text.trim() === normalized;
  }

  private appendApproval(event: Record<string, unknown>, sessionId?: string): void {
    const toolName = stringField(event, 'tool_name');
    const reason = pickString(event, 'reason') ?? 'Approval required';
    const parameters = event.parameters;
    const riskClass = pickString(event, 'risk_class') ?? this.riskClassFromPendingTool(toolName);
    this.markToolWaitingForApproval(toolName);
    if (this.hasPendingApproval(toolName, reason, parameters)) {
      if (!this.state.pendingApproval) {
        this.state.pendingApproval = this.pendingApprovalFromTranscript(toolName, reason, parameters);
      }
      this.publish();
      return;
    }
    const item: TranscriptItem = {
      role: 'approval',
      text: toolName,
      detail: reason,
      toolName,
      reason,
      parameters,
      approvalSessionId: sessionId,
      riskClass,
    };
    const path = pickString(parameters, 'path');
    if (path) {
      item.path = path;
    }
    this.state.pendingApproval = item;
    this.append(item);

    if ((toolName === 'file_write' || toolName === 'file_patch') && path) {
      void this.enrichApprovalDiff(item, toolName, parameters);
    }
  }

  private approvalPayloadFromPendingTool(): Record<string, unknown> | undefined {
    const item = [...this.state.transcript]
      .reverse()
      .find((entry) => entry.role === 'tool' && entry.pending);
    if (!item?.toolName) return undefined;
    return {
      tool_name: item.toolName,
      reason: 'Approval required',
      parameters: item.toolParameters ?? {},
      risk_class: item.riskClass,
    };
  }

  private riskClassFromPendingTool(toolName: string): string | undefined {
    return [...this.state.transcript]
      .reverse()
      .find((entry) => entry.role === 'tool' && entry.pending && (entry.toolName === toolName || !entry.toolName))
      ?.riskClass;
  }

  private markToolWaitingForApproval(toolName: string): void {
    for (let i = this.state.transcript.length - 1; i >= 0; i -= 1) {
      const item = this.state.transcript[i];
      if (item.role === 'tool' && item.pending && (item.toolName === toolName || !item.toolName)) {
        item.pending = false;
        item.text = toolName;
        item.detail = undefined;
        item.toolResultSummary = 'waiting for approval';
        return;
      }
    }
  }

  private hasPendingApproval(toolName: string, reason: string, parameters: unknown): boolean {
    const serializedParameters = json(parameters);
    const matches = (item: TranscriptItem): boolean =>
      item.role === 'approval' &&
      item.toolName === toolName &&
      item.reason === reason &&
      json(item.parameters) === serializedParameters;
    return (
      Boolean(this.state.pendingApproval && matches(this.state.pendingApproval)) ||
      this.state.transcript.some(matches)
    );
  }

  private pendingApprovalFromTranscript(
    toolName: string,
    reason: string,
    parameters: unknown,
  ): TranscriptItem | undefined {
    const serializedParameters = json(parameters);
    return [...this.state.transcript].reverse().find(
      (item) =>
        item.role === 'approval' &&
        item.toolName === toolName &&
        item.reason === reason &&
        json(item.parameters) === serializedParameters,
    );
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
            cacheReadTokens: optionalNumber(usage, 'cache_read_tokens') ?? optionalNumber(usage, 'cache_read_input_tokens'),
            cacheCreationTokens: optionalNumber(usage, 'cache_creation_tokens') ?? optionalNumber(usage, 'cache_creation_input_tokens'),
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
        const contextTokens = optionalNumber(event, 'context_tokens');
        const messageTokens = optionalNumber(event, 'message_tokens');
        const systemTokens = optionalNumber(event, 'system_tokens');
        const toolSchemaTokens = optionalNumber(event, 'tool_schema_tokens');
        const overheadTokens = optionalNumber(event, 'overhead_tokens');
        if (typeof contextTokens === 'number') next.contextTokens = contextTokens;
        if (typeof messageTokens === 'number') next.messageTokens = messageTokens;
        if (typeof systemTokens === 'number') next.systemTokens = systemTokens;
        if (typeof toolSchemaTokens === 'number') next.toolSchemaTokens = toolSchemaTokens;
        if (typeof overheadTokens === 'number') next.overheadTokens = overheadTokens;
        this.state.hud.context = next;
        this.publish();
        return;
      }
      case 'mcp_status_changed': {
        const mcpServers = mcpServersForStatusEvent(event);
        if (mcpServers) {
          this.state.context = {
            ...this.state.context,
            mcpServers,
          };
          this.publish();
        }
        return;
      }
      case 'agents_md_loaded': {
        const agents = agentsSummaryForLoadedEvent(event);
        if (agents) {
          this.state.context = {
            ...this.state.context,
            agents,
          };
          this.publish();
        }
        return;
      }
      case 'plan_updated': {
        const stepsRaw = Array.isArray(event.steps) ? event.steps : [];
        const current = optionalNumber(event, 'current');
        const steps: PlanStepView[] = stepsRaw.map((entry, index) => {
          if (!isRecord(entry)) return { text: stringField({ value: entry }, 'value') };
          const explicitStatus = typeof entry.status === 'string' ? entry.status : undefined;
          const done = booleanField(entry, 'done');
          const status =
            explicitStatus ??
            (done === true ? 'done' : typeof current === 'number' && current === index ? 'in_progress' : 'pending');
          return {
            text: pickString(entry, 'text') ?? pickString(entry, 'label') ?? 'Untitled step',
            status,
          };
        });
        const next: PlanSlice = { steps };
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
    switch (message.type) {
      case 'ready':
        this.publish();
        return;
      case 'run': {
        const task = message.task.trim();
        if (task.length === 0) return;
        this.state.runOptions = message.options;
        if (task.startsWith('/')) {
          await this.handleSlashCommand(task, message.options);
          return;
        }
        await this.handlers.runTask(task, message.options);
        return;
      }
      case 'cancel':
        await this.handlers.cancelTask();
        return;
      case 'loginOpenAi':
        await this.handlers.loginOpenAi();
        return;
      case 'refreshStatus':
        await this.handlers.refreshStatus();
        return;
      case 'showCodeMap':
        await this.handlers.showCodeMap();
        return;
      case 'showCodeMapStatus':
        await this.handlers.showCodeMapStatus();
        return;
      case 'refreshCodeMap':
        await this.handlers.refreshCodeMap();
        return;
      case 'searchCodeMap':
        await this.handlers.searchCodeMap();
        return;
      case 'outlineCurrentFile':
        await this.handlers.outlineCurrentFile();
        return;
      case 'findSymbolReferences':
        await this.handlers.findSymbolReferences();
        return;
      case 'showSkills':
        await this.handlers.showSkills();
        return;
      case 'showArchivedSkills':
        await this.handlers.showArchivedSkills();
        return;
      case 'searchSkills':
        await this.handlers.searchSkills();
        return;
      case 'searchArchivedSkills':
        await this.handlers.searchArchivedSkills();
        return;
      case 'showSkill':
        await this.handlers.showSkill(message.name);
        return;
      case 'useSkill':
        await this.handlers.useSkill(message.name);
        return;
      case 'toggleSkillPin':
        await this.handlers.toggleSkillPin(message.name, message.pinned);
        return;
      case 'archiveSkill':
        await this.handlers.archiveSkill(message.name);
        return;
      case 'restoreSkill':
        await this.handlers.restoreSkill(message.name);
        return;
      case 'attachFile':
        await this.handlers.attachFile();
        return;
      case 'detachAttachment':
        await this.handlers.detachAttachment(message.path);
        return;
      case 'showSessions':
        await this.handlers.showSessions();
        return;
      case 'pruneSessions':
        await this.handlers.pruneSessions();
        return;
      case 'replaySessionTimeline':
        await this.handlers.replaySessionTimeline();
        return;
      case 'exportSessionArtifacts':
        await this.handlers.exportSessionArtifacts();
        return;
      case 'importSessionArtifacts':
        await this.handlers.importSessionArtifacts();
        return;
      case 'showPrStatus':
        await this.handlers.showPrStatus();
        return;
      case 'shipChanges':
        await this.handlers.shipChanges();
        return;
      case 'mergePr':
        await this.handlers.mergePr();
        return;
      case 'askUserRespond':
        if (message.requestId) {
          const accepted = await this.handlers.respondAskUser(message.requestId, message.answer);
          if (accepted) {
            this.resolveInteraction(message.requestId, answerLabel(message.answer));
          }
        }
        return;
      case 'approvalRespond':
        await this.handlers.respondApproval({
          approved: message.approved,
          scope: message.scope,
          toolName: message.toolName,
          reason: message.reason,
          parameters: message.parameters,
          sessionId: message.sessionId,
        });
        this.resolveApproval(message.approved);
        return;
      case 'openFile':
        await this.handlers.openFile(message.path, message.line, message.column, this.state.context.workspace);
        return;
      case 'openPath':
        await this.handlers.openPath(message.path);
        return;
      case 'registerProvider':
        await this.handlers.registerProvider(message.provider, message.params);
        return;
      case 'openSettings':
        void vscode.commands.executeCommand('peridot.openSettings');
        return;
      case 'showLanding':
        this.state.view = 'landing';
        this.state.landing = message.screen ?? 'home';
        this.publish();
        return;
      case 'showSession':
        this.state.view = 'session';
        this.publish();
        return;
      case 'newSession':
        this.createNewSession(this.state.context.workspace);
        return;
      case 'selectSession':
        this.selectSession(message.id);
        return;
      case 'renameSession':
        if (!this.renameSession(message.id, message.title)) {
          this.appendError('Could not rename that session.');
        }
        return;
      case 'deleteSession': {
        const session = this.sessions.get(message.id);
        if (!session) {
          this.appendError('Could not find that session.');
          return;
        }
        await this.handlers.deleteSession(session.id, session.daemonSessionId);
        this.deleteSession(session.id);
        return;
      }
      case 'copyText':
        await this.handlers.copyText(message.text);
        return;
      case 'queueAdd':
        if (message.task.trim().length > 0) {
          this.state.queue = [...this.state.queue, { id: queueId(), text: message.task.trim() }];
          this.publish();
        }
        return;
      case 'queueRemove':
        this.state.queue = this.state.queue.filter((item) => item.id !== message.id);
        this.publish();
        return;
      case 'queueEdit':
        this.state.queue = this.state.queue.map((item) =>
          item.id === message.id ? { ...item, text: message.text } : item,
        );
        this.publish();
        return;
      case 'queueClear':
        this.state.queue = [];
        this.publish();
        return;
      case 'dismissBranchPicker':
        this.state.branchPicker = undefined;
        this.publish();
        return;
    }
  }

  private append(item: TranscriptItem): void {
    if (shouldSuppress(item)) {
      return;
    }
    const last = this.state.transcript[this.state.transcript.length - 1];
    if (item.role === 'assistant' && last?.role === 'assistant') {
      // Streaming delta: accumulate text in memory immediately.
      last.text += item.text;
      // Leading-edge + trailing-edge throttle. The first delta after a quiet
      // period publishes *immediately* so the user sees the first tokens with
      // no perceptible latency. Subsequent deltas within the 32ms window are
      // coalesced and emitted together on the trailing edge — that's enough
      // for ~30 fps streaming, which feels smooth without burning serialization
      // CPU on every token.
      if (this.streamCoalesceTimer === undefined) {
        this.publishStreaming();
        this.streamCoalesceTimer = setTimeout(() => {
          const hadPending = this.streamCoalescePending;
          this.streamCoalesceTimer = undefined;
          this.streamCoalescePending = false;
          if (hadPending) this.publishStreaming();
        }, 32);
      } else {
        this.streamCoalescePending = true;
      }
    } else {
      this.state.transcript.push(item);
      this.publish();
    }
  }

  private resolveInteraction(requestId: string, detail: string): void {
    const item = this.state.transcript.find((entry) => entry.requestId === requestId);
    if (item) {
      item.role = 'status';
      item.text = 'User response sent';
      item.detail = detail;
      item.request = undefined;
    }
    this.state.status = 'Running';
    this.state.context = {
      ...this.state.context,
      status: this.state.status,
      running: true,
    };
    this.publish();
  }

  private resolveApproval(approved: boolean): void {
    const item = [...this.state.transcript].reverse().find((entry) => entry.role === 'approval');
    if (item) {
      item.role = 'status';
      item.text = approved ? 'Approval sent' : 'Approval denied';
      item.detail = item.toolName;
    }
    this.state.pendingApproval = undefined;
    this.publish();
  }

  /** Full publish: cancels any pending streaming coalesce, refreshes session
   *  list, schedules (or flushes) persistence, and posts state to the webview. */
  private publish(): void {
    // Cancel any pending coalesced streaming publish — this full publish
    // supersedes it.
    if (this.streamCoalesceTimer !== undefined) {
      clearTimeout(this.streamCoalesceTimer);
      this.streamCoalesceTimer = undefined;
    }
    this.streamCoalescePending = false;
    this.saveActiveSession();
    this.refreshSessionSummaries();
    // Persist immediately for decisive events (session end, user actions).
    // During high-frequency streaming the streaming path uses schedulePersist.
    this.schedulePersist();
    this.postState();
  }

  /** Lightweight publish for streaming deltas: skips session-list rebuild
   *  (unchanged during streaming) and uses throttled persistence. */
  private publishStreaming(): void {
    this.saveActiveSession();
    this.schedulePersist();
    this.postState();
  }

  private postState(): void {
    const message: InboundMessage = { type: 'state', state: this.state };
    this.view?.webview.postMessage(message);
  }

  /** Throttled persistence — writes full state to Memento (SQLite) at most
   *  once every 2 seconds so disk I/O doesn't block the event loop. */
  private schedulePersist(): void {
    if (this.persistTimer !== undefined) return; // already scheduled
    this.persistTimer = setTimeout(() => {
      this.persistTimer = undefined;
      this.persistState();
    }, 2000);
  }

  /** Flush any pending persistence immediately (call on session end). */
  private flushPersist(): void {
    if (this.persistTimer !== undefined) {
      clearTimeout(this.persistTimer);
      this.persistTimer = undefined;
    }
    this.persistState();
  }

  private async handleSlashCommand(input: string, options: RunOptions): Promise<void> {
    await this.executeDaemonSlash(input, options);
  }

  private async executeDaemonSlash(input: string, options: RunOptions): Promise<void> {
    try {
      const result = await this.handlers.runSlashCommand(input, options);
      if (result.action === 'clear') {
        await this.handlers.clearSession({
          skipDaemonCancel: result.cancelled === true || result.deleted === true,
        });
        this.clearActiveSession();
        this.append({ role: 'status', text: 'clear: transcript + context wiped, new session' });
        return;
      }
      this.applySlashCommandState(input, result, options);
      if (result.action === 'local' && this.handleLocalClientAction(input)) {
        return;
      }
      this.applyRewindResult(result);
      this.applySessionMutationResult(result);
      if (sessionResultClosesDaemonRun(result) && result.session_id) {
        await this.handlers.finishDaemonSession(result.session_id);
      }
      this.appendCommandResult(result);
      if (slashCommandChangesSkillCatalog(input)) {
        await this.handlers.refreshSlashCatalog();
      }
      if (slashCommandChangesBranchSnapshots(input)) {
        await this.handlers.refreshStatus();
      }
      const task = taskStartedByCommandResult(result);
      if (task) {
        await this.handlers.runTask(task, this.state.runOptions);
      }
    } catch (err) {
      this.appendError(err instanceof Error ? err.message : String(err));
    }
  }

  private handleLocalClientAction(input: string): boolean {
    switch (localSlashAction(input)) {
      case 'showInfo':
        this.showInfo();
        return true;
      default:
        return false;
    }
  }

  private applySlashCommandState(
    input: string,
    result: CommandResultView,
    options: RunOptions,
  ): void {
    const delta = result.state_delta ?? result.stateDelta;
    const next = delta ? applyRunOptionDelta(options, delta) : undefined;
    if (next) {
      this.state.runOptions = next;
      this.state.context = {
        ...this.state.context,
        mode: next.mode,
        permission: next.permission,
        model: next.model ?? this.state.context.model,
        reasoningEffort: next.reasoningEffort,
        serviceTier: next.serviceTier,
      };
    }
    if (delta) {
      const committeeMode = delta.committee_mode ?? delta.committeeMode;
      this.state.context = {
        ...this.state.context,
        ...(committeeMode ? { committeeMode } : {}),
        modelSuggestions: appendModelSuggestions(
          this.state.context.modelSuggestions,
          delta.model,
          delta.subagent_default_model ?? delta.subagentDefaultModel,
        ),
      };
    }
    if (delta?.provider) {
      this.state.context = { ...this.state.context, provider: delta.provider };
    }
    if (input.startsWith('/branch turn ') || input.startsWith('/branch switch ')) {
      this.state.branchPicker = undefined;
    }
    if (result.kind === 'setting' || result.kind === 'note') {
      this.state.context = { ...this.state.context, problem: undefined };
    }
  }

  private applySessionMutationResult(result: CommandResultView): void {
    if (result.kind === 'session_list') {
      const filtered =
        typeof result.status_filter === 'string' || typeof result.statusFilter === 'string';
      this.reconcileDaemonSessions(Array.isArray(result.sessions) ? result.sessions : [], {
        pruneMissing: !filtered,
      });
      return;
    }
    if (result.kind === 'session_save' || result.kind === 'session_import') {
      this.reconcileSessionSaveResult(result);
      return;
    }
    if (result.kind === 'session_new') {
      const session = this.reconcileSessionNewResult(result);
      if (session) {
        this.selectSession(session.id);
      } else {
        this.createNewSession(this.state.context.workspace);
      }
      return;
    }
    if (result.kind === 'session_switch' && result.switched === true) {
      const session =
        this.reconcileSessionSwitchResult(result) ?? this.ensureSessionFromSwitchResult(result);
      if (session) {
        this.selectSession(session.id);
      }
      return;
    }
    if (result.kind === 'session_rename' && result.renamed !== false) {
      const session =
        this.reconcileSessionRenameResult(result) ?? this.findSessionByResultId(result);
      const title = (result.session_title ?? result.sessionTitle ?? '').trim();
      if (session && title.length > 0) {
        this.renameSession(session.id, title);
      }
      return;
    }
    if (
      (result.kind === 'session_delete' || result.kind === 'session_close') &&
      (result.deleted === true || result.cancelled === true)
    ) {
      const session = this.findSessionByResultId(result);
      if (session) {
        this.deleteSession(session.id);
      }
    }
  }

  private reconcileSessionSaveResult(result: CommandResultView): void {
    const id = result.session_id?.trim();
    if (!id) return;
    const title =
      (result.label ?? result.session_title ?? result.sessionTitle ?? result.summary ?? id).trim() || id;
    this.reconcileDaemonSessions([
      {
        id,
        title,
        summary: result.summary,
        status: result.status,
        running: result.running,
        updated_at_unix: result.updated_at_unix,
        total_tokens: result.total_tokens,
        total_cost_usd: result.total_cost_usd,
        turns_used: result.turns_used,
      },
    ]);
  }

  private reconcileSessionNewResult(result: CommandResultView): StoredChatSession | undefined {
    const id = result.session_id?.trim();
    if (!id) return undefined;
    const title =
      (result.session_title ?? result.sessionTitle ?? result.summary ?? id).trim() || id;
    this.reconcileDaemonSessions([
      {
        id,
        title,
        summary: result.summary,
        status: result.status,
        running: result.running,
        updated_at_unix: result.updated_at_unix,
        total_tokens: result.total_tokens,
        total_cost_usd: result.total_cost_usd,
        turns_used: result.turns_used,
      },
    ]);
    return this.findSessionByResultId(result);
  }

  private reconcileSessionSwitchResult(result: CommandResultView): StoredChatSession | undefined {
    const id = result.session_id?.trim();
    if (!id) return undefined;
    const title =
      (result.session_title ?? result.sessionTitle ?? result.summary ?? id).trim() || id;
    this.reconcileDaemonSessions([
      {
        id,
        title,
        summary: result.summary,
        status: result.status,
        running: result.running,
        updated_at_unix: result.updated_at_unix,
        total_tokens: result.total_tokens,
        total_cost_usd: result.total_cost_usd,
        turns_used: result.turns_used,
      },
    ]);
    return this.findSessionByResultId(result);
  }

  private reconcileSessionRenameResult(result: CommandResultView): StoredChatSession | undefined {
    const id = result.session_id?.trim();
    if (!id) return undefined;
    const title =
      (result.session_title ?? result.sessionTitle ?? result.summary ?? id).trim() || id;
    this.reconcileDaemonSessions([
      {
        id,
        title,
        summary: result.summary,
        status: result.status,
        running: result.running,
        updated_at_unix: result.updated_at_unix,
        total_tokens: result.total_tokens,
        total_cost_usd: result.total_cost_usd,
        turns_used: result.turns_used,
      },
    ]);
    return this.findSessionByResultId(result);
  }

  private ensureSessionFromSwitchResult(result: CommandResultView): StoredChatSession | undefined {
    const id = result.session_id?.trim();
    if (!id) return undefined;
    const existing = this.findSessionByResultId(result);
    if (existing) return existing;
    const title = (result.session_title ?? result.sessionTitle ?? id).trim() || id;
    const session = this.createSession(title);
    this.sessions.delete(session.id);
    session.id = id;
    session.daemonSessionId = id;
    session.status = remoteSessionStatus({
      id,
      title,
      status: result.status,
      running: result.running,
    });
    session.running = result.running === true;
    this.sessions.set(session.id, session);
    return session;
  }

  private findSessionByResultId(result: CommandResultView): StoredChatSession | undefined {
    const id = result.session_id?.trim();
    if (!id) return undefined;
    return Array.from(this.sessions.values()).find(
      (session) => session.id === id || session.daemonSessionId === id,
    );
  }

  private applyRewindResult(result: CommandResultView): void {
    if (result.kind !== 'rewind') return;
    const restored = (result.restored_prompt ?? result.restoredPrompt ?? '').trim();
    this.rewindLastExchange(restored.length > 0 ? restored : undefined);
  }

  private showInfo(): void {
    const c = this.state.context;
    const o = this.state.runOptions;
    this.append({
      role: 'assistant',
      text: [
        `Status: ${this.state.status}`,
        `Workspace: ${c.workspace ?? 'unknown'}`,
        `Provider: ${c.provider ?? 'unknown'}`,
        `Model: ${o.model ?? c.model ?? 'default'}`,
        `Mode: ${o.mode}`,
        `Permission: ${o.permission}`,
        `Reasoning: ${o.reasoningEffort ?? c.reasoningEffort ?? 'default'}`,
        `Service tier: ${o.serviceTier ?? c.serviceTier ?? 'standard'}`,
        `Sessions: ${this.sessions.size}`,
      ].join('\n'),
    });
  }

  private rewindLastExchange(restoredPrompt?: string): void {
    const lastUser = this.state.transcript.map((item) => item.role).lastIndexOf('user');
    if (lastUser < 0) {
      this.append({ role: 'status', text: 'rewind: nothing to rewind' });
      return;
    }
    this.state.transcript = this.state.transcript.slice(0, lastUser);
    if (restoredPrompt) {
      this.state.composerDraft = restoredPrompt;
      this.state.composerDraftVersion = (this.state.composerDraftVersion ?? 0) + 1;
    }
    this.append({ role: 'status', text: 'rewind: last exchange removed' });
  }

  private ensureActiveSession(): StoredChatSession {
    if (this.state.activeChatId && this.sessions.has(this.state.activeChatId)) {
      return this.sessions.get(this.state.activeChatId)!;
    }
    const session = this.createSession();
    this.state.activeChatId = session.id;
    this.state.sessionId = session.daemonSessionId;
    this.state.transcript = session.transcript;
    this.state.hud = session.hud;
    this.state.status = session.status;
    this.state.running = session.running;
    return session;
  }

  private finishRunTimer(finalStatus?: string): void {
    const startedAt = this.state.runStartedAtMs;
    if (typeof startedAt !== 'number') return;
    const elapsed = Math.max(0, Date.now() - startedAt);
    this.state.lastRunElapsedMs = elapsed;
    this.state.runStartedAtMs = undefined;
    if (finalStatus) {
      const label = finalStatus === 'Failed'
        ? 'Stopped after'
        : finalStatus === 'Interrupted'
          ? 'Interrupted after'
          : 'Finished in';
      this.state.transcript.push({
        role: 'status',
        statusKind: 'completion',
        text: `${label} ${formatElapsed(elapsed)}`,
      });
    }
  }

  private activeStoredSession(): StoredChatSession | undefined {
    return this.state.activeChatId ? this.sessions.get(this.state.activeChatId) : undefined;
  }

  private createSession(title?: string): StoredChatSession {
    const id = `chat-${Date.now()}-${this.nextSessionOrdinal}`;
    const session: StoredChatSession = {
      id,
      title: title ?? `New session ${this.nextSessionOrdinal}`,
      status: 'Idle',
      running: false,
      transcript: [],
      hud: {},
      runOptions: { ...this.state.runOptions },
      pendingApproval: undefined,
      runStartedAtMs: undefined,
      lastRunElapsedMs: undefined,
    };
    this.nextSessionOrdinal += 1;
    this.sessions.set(id, session);
    return session;
  }

  private saveActiveSession(): void {
    const id = this.state.activeChatId;
    if (!id) return;
    const session = this.sessions.get(id);
    if (!session) return;
    session.daemonSessionId = this.state.sessionId;
    session.status = this.state.status;
    session.running = this.state.running;
    session.transcript = this.state.transcript;
    session.hud = this.state.hud;
    session.runOptions = { ...this.state.runOptions };
    session.pendingApproval = this.state.pendingApproval;
    session.runStartedAtMs = this.state.runStartedAtMs;
    session.lastRunElapsedMs = this.state.lastRunElapsedMs;
  }

  private restorePersistedState(): void {
    const snapshot = this.storage.get<PersistedSidebarSnapshot>(PERSISTENCE_KEY);
    if (!snapshot || snapshot.version !== 1 || !Array.isArray(snapshot.sessions)) {
      return;
    }
    this.sessions.clear();
    for (const raw of snapshot.sessions) {
      const session: StoredChatSession = {
        ...raw,
        status: raw.running ? 'Idle' : raw.status,
        running: false,
        transcript: Array.isArray(raw.transcript) ? raw.transcript : [],
        hud: raw.hud ?? {},
        runOptions: raw.runOptions ?? freshState().runOptions,
        pendingApproval: raw.pendingApproval,
        runStartedAtMs: undefined,
        lastRunElapsedMs: raw.lastRunElapsedMs,
      };
      this.sessions.set(session.id, session);
    }
    this.nextSessionOrdinal = Math.max(1, snapshot.nextSessionOrdinal);
    this.state = {
      ...freshState(),
      view: snapshot.view ?? 'landing',
      landing: snapshot.landing ?? 'home',
      activeChatId: snapshot.activeChatId,
      context: {
        ...(snapshot.context ?? {}),
        status: 'Idle',
        running: false,
        problem: undefined,
      },
      queue: Array.isArray(snapshot.queue) ? snapshot.queue : [],
      runOptions: snapshot.runOptions ?? freshState().runOptions,
    };
    if (this.state.activeChatId && this.sessions.has(this.state.activeChatId)) {
      this.loadSessionIntoState(this.state.activeChatId, false);
    }
    this.refreshSessionSummaries();
  }

  private persistState(): void {
    const sessions = Array.from(this.sessions.values()).map(
      (session): StoredChatSession => ({
        ...session,
        status: session.running ? 'Idle' : session.status,
        running: false,
        pendingApproval: session.pendingApproval,
      }),
    );
    const snapshot: PersistedSidebarSnapshot = {
      version: 1,
      activeChatId: this.state.activeChatId,
      nextSessionOrdinal: this.nextSessionOrdinal,
      runOptions: this.state.runOptions,
      context: {
        ...this.state.context,
        status: this.state.running ? 'Idle' : this.state.status,
        running: false,
      },
      view: this.state.view,
      landing: this.state.landing,
      queue: this.state.queue,
      sessions,
    };
    void this.storage.update(PERSISTENCE_KEY, snapshot);
  }

  private loadSessionIntoState(id: string | undefined, publish: boolean): void {
    if (!id) return;
    const session = this.sessions.get(id);
    if (!session) return;
    this.state.activeChatId = id;
    this.state.sessionId = session.daemonSessionId;
    this.state.status = session.status;
    this.state.running = session.running;
    this.state.transcript = session.transcript;
    this.state.hud = session.hud;
    this.state.runOptions = { ...session.runOptions };
    this.state.pendingApproval = session.pendingApproval;
    this.state.runStartedAtMs = session.runStartedAtMs;
    this.state.lastRunElapsedMs = session.lastRunElapsedMs;
    this.state.context = {
      ...this.state.context,
      status: session.status,
      running: session.running,
      problem: undefined,
      mode: session.runOptions.mode,
      permission: session.runOptions.permission,
      model: session.runOptions.model ?? this.state.context.model,
      reasoningEffort: session.runOptions.reasoningEffort,
      serviceTier: session.runOptions.serviceTier,
    };
    this.state.view = 'session';
    if (publish) this.publish();
  }

  private loadDraftSessionIntoState(workspace?: string): void {
    this.state.activeChatId = undefined;
    this.state.sessionId = undefined;
    this.state.status = 'Idle';
    this.state.running = false;
    this.state.transcript = [];
    this.state.hud = {};
    this.state.branchPicker = undefined;
    this.state.pendingApproval = undefined;
    this.state.runStartedAtMs = undefined;
    this.state.lastRunElapsedMs = undefined;
    this.state.runOptions = freshState().runOptions;
    this.state.context = {
      ...this.state.context,
      ...(workspace ? { workspace } : {}),
      status: 'Idle',
      running: false,
      problem: undefined,
      mode: this.state.runOptions.mode,
      permission: this.state.runOptions.permission,
      reasoningEffort: undefined,
      serviceTier: undefined,
    };
    this.state.view = 'session';
  }

  private refreshSessionSummaries(): void {
    const active = this.state.activeChatId;
    this.state.sessions = Array.from(this.sessions.values()).map(
      (session): ChatSessionSummary => ({
        id: session.id,
        title: session.title,
        status: session.status,
        running: session.running,
        active: session.id === active,
      }),
    );
  }

  private clearActiveSession(): void {
    this.ensureActiveSession();
    this.state.sessionId = undefined;
    this.state.status = 'Idle';
    this.state.running = false;
    this.state.transcript = [];
    this.state.hud = {};
    this.state.pendingApproval = undefined;
    this.state.context = { ...this.state.context, status: 'Idle', running: false };
    const session = this.activeStoredSession();
    if (session) {
      session.daemonSessionId = undefined;
      session.status = 'Idle';
      session.running = false;
      session.transcript = [];
      session.hud = {};
      session.pendingApproval = undefined;
      session.runStartedAtMs = undefined;
      session.lastRunElapsedMs = undefined;
    }
    this.publish();
  }

  private html(webview: vscode.Webview): string {
    const nonce = nonceValue();
    const scriptUri = webview.asWebviewUri(
      vscode.Uri.joinPath(this.extensionUri, 'dist', 'webview.js'),
    );
    const styleUri = webview.asWebviewUri(
      vscode.Uri.joinPath(this.extensionUri, 'dist', 'webview.css'),
    );
    const iconUri = webview.asWebviewUri(
      vscode.Uri.joinPath(this.extensionUri, 'resources', 'peridot-icon.png'),
    );
    return /* html */ `<!DOCTYPE html>
<html lang="en">
<head>
  <meta charset="UTF-8">
  <meta
    http-equiv="Content-Security-Policy"
    content="default-src 'none'; img-src ${webview.cspSource} data: https: http:; style-src ${webview.cspSource} 'unsafe-inline'; script-src 'nonce-${nonce}';"
  >
  <meta name="viewport" content="width=device-width, initial-scale=1.0">
  <title>Peridot</title>
  <link href="${styleUri}" rel="stylesheet" />
</head>
<body>
  <div class="app" id="app" data-mascot="${iconUri}">
    <!-- Webview bundle owns layout; index.ts populates #app based on
         the SidebarState received over postMessage. -->
  </div>
  <script nonce="${nonce}" src="${scriptUri}"></script>
</body>
</html>`;
  }
}

function displayPhaseLabel(phase: string): string {
  const lower = phase.toLowerCase();
  if (lower === 'verifying') return 'checking';
  return lower;
}

function remoteSessionTitle(session: DaemonSessionSummary): string {
  const raw = session.title ?? session.last_task ?? session.summary ?? session.id;
  const title = raw.trim();
  return title.length > 0 ? title : session.id;
}

function remoteSessionStatus(session: DaemonSessionSummary): string {
  const status = session.status?.trim();
  if (!status) return session.running ? 'Running' : 'Idle';
  return status.charAt(0).toUpperCase() + status.slice(1);
}

function parseReasoningEffort(value: string): RunOptions['reasoningEffort'] | undefined {
  switch (value.trim().toLowerCase()) {
    case 'off':
    case 'none':
    case 'false':
    case '0':
      return 'off';
    case 'low':
    case 'min':
    case 'minimal':
      return 'low';
    case 'medium':
    case 'med':
    case 'default':
    case 'true':
      return 'medium';
    case 'high':
    case 'max':
    case 'maximum':
      return 'high';
    case 'xhigh':
    case 'x-high':
    case 'extra-high':
    case 'extra_high':
      return 'xhigh';
    default:
      return undefined;
  }
}

function normalizeReasoning(
  value: string | undefined,
): RunOptions['reasoningEffort'] | undefined {
  return value ? parseReasoningEffort(value) : undefined;
}

function sessionResultClosesDaemonRun(result: CommandResultView): boolean {
  return (
    (result.kind === 'session_delete' || result.kind === 'session_close') &&
    result.cancelled === true
  );
}

function slashCommandChangesSkillCatalog(input: string): boolean {
  const [command, subcommand] = input
    .slice(1)
    .trim()
    .split(/\s+/)
    .map((part) => part.toLowerCase());
  return command === 'skills' && (subcommand === 'archive' || subcommand === 'restore');
}

function slashCommandChangesBranchSnapshots(input: string): boolean {
  const [command, subcommand] = input
    .slice(1)
    .trim()
    .split(/\s+/)
    .map((part) => part.toLowerCase());
  return command === 'branch' && subcommand === 'save';
}

function taskStartedByCommandResult(result: CommandResultView): string | undefined {
  if (result.kind !== 'start_task' && result.kind !== 'session_new') return undefined;
  const task = result.task?.trim();
  return task && task.length > 0 ? task : undefined;
}

function appendModelSuggestions(
  current: string[] | undefined,
  ...models: Array<string | null | undefined>
): string[] | undefined {
  const next = [...(current ?? [])];
  for (const model of models) {
    if (typeof model !== 'string') continue;
    const trimmed = model.trim();
    if (!trimmed || next.some((entry) => entry.toLowerCase() === trimmed.toLowerCase())) continue;
    next.push(trimmed);
  }
  return next.length > 0 ? next.sort((a, b) => a.localeCompare(b)) : current;
}

function applyRunOptionDelta(
  options: RunOptions,
  delta: SlashStateDeltaView,
): RunOptions | undefined {
  let next: RunOptions | undefined;
  const update = (): RunOptions => {
    next ??= { ...options };
    return next;
  };

  if (isMode(delta.mode)) {
    update().mode = delta.mode;
  }
  if (isPermission(delta.permission)) {
    update().permission = delta.permission;
  }
  if (typeof delta.model === 'string' && delta.model.length > 0) {
    update().model = delta.model;
  }

  const reasoningValue = delta.reasoning_effort ?? delta.reasoningEffort;
  const reasoning =
    typeof reasoningValue === 'string' ? parseReasoningEffort(reasoningValue) : undefined;
  if (reasoning) {
    update().reasoningEffort = reasoning;
  }

  const serviceTierValue = pickDeltaValue(delta, 'service_tier', 'serviceTier');
  const serviceTier = normalizeServiceTier(serviceTierValue);
  if (serviceTier) {
    update().serviceTier = serviceTier;
  }

  return next;
}

function pickDeltaValue<T extends object>(
  delta: T,
  snakeKey: keyof T,
  camelKey: keyof T,
): unknown {
  if (Object.prototype.hasOwnProperty.call(delta, snakeKey)) {
    return delta[snakeKey];
  }
  if (Object.prototype.hasOwnProperty.call(delta, camelKey)) {
    return delta[camelKey];
  }
  return undefined;
}

function normalizeServiceTier(value: unknown): RunOptions['serviceTier'] | undefined {
  if (value === null) {
    return 'standard';
  }
  if (typeof value !== 'string') {
    return undefined;
  }
  switch (value.trim().toLowerCase()) {
    case 'fast':
    case 'priority':
      return 'fast';
    case 'standard':
    case 'default':
    case 'none':
    case 'off':
      return 'standard';
    default:
      return undefined;
  }
}

function isMode(value: unknown): value is RunOptions['mode'] {
  return value === 'execute' || value === 'plan' || value === 'goal';
}

function isPermission(value: unknown): value is RunOptions['permission'] {
  return value === 'auto' || value === 'safe' || value === 'yolo';
}

function freshState(): SidebarState {
  return {
    view: 'landing',
    landing: 'home',
    running: false,
    status: 'Idle',
    context: {},
    transcript: [],
    sessions: [],
    queue: [],
    runOptions: {
      mode: 'execute',
      permission: 'auto',
    },
    hud: {} as HudState,
    slashCommands: [],
    authBusy: false,
    runStartedAtMs: undefined,
    lastRunElapsedMs: undefined,
    composerDraft: undefined,
    composerDraftVersion: 0,
  };
}

function shouldSuppress(item: TranscriptItem): boolean {
  if (item.role !== 'status') return false;
  const noisy = ['agents md loaded', 'turn started', 'turn ended', 'assistant started'];
  const lowered = item.text.toLowerCase();
  return noisy.some((needle) => lowered.includes(needle));
}

function transcriptItemForEvent(
  kind: string,
  event: Record<string, unknown>,
): TranscriptItem | undefined {
  const agentEventItem = agentTranscriptItemForEvent(kind, event);
  if (agentEventItem) return agentEventItem;

  switch (kind) {
    case 'started':
      return { role: 'status', text: 'Daemon started' };
    case 'run_started':
      return undefined;
    case 'agents_md_loaded':
    case 'turn_started':
    case 'turn_ended':
    case 'assistant_started':
    case 'assistant_finished':
    case 'context_utilization_changed':
    case 'usage_updated':
    case 'budget_updated':
    case 'mcp_status_changed':
    case 'committee_role_usage':
      return undefined;
    case 'assistant_delta':
      return { role: 'assistant', text: stringField(event, 'delta') };
    case 'thinking': {
      const text = stringField(event, 'text');
      return text.trim().length > 0 ? { role: 'thinking', text } : undefined;
    }
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
      return undefined;
    case 'command_result':
      return undefined;
    case 'file_diff': {
      const path = stringField(event, 'path');
      return {
        role: 'diff',
        text: path || 'file',
        detail: stringField(event, 'tool_name'),
        path,
        before: typeof event.before === 'string' ? event.before : null,
        after: typeof event.after === 'string' ? event.after : '',
      };
    }
    case 'finished':
      return undefined;
    case 'error':
      return { role: 'error', text: stringField(event, 'message') };
    case 'recovery':
      return undefined;
    case 'interrupted':
      return { role: 'status', text: 'Interrupted', detail: stringField(event, 'stage') };
    default:
      return shouldSuppressAgentEventFallback(kind) ? undefined : { role: 'status', text: 'Event' };
  }
}

function compactionSnapshotView(compacted: Record<string, unknown>): CompactionSnapshotView {
  const narrative =
    typeof compacted.narrative === 'string' ? compacted.narrative.trim() : undefined;
  return {
    narrative,
    decisions: arrayField(compacted, 'decisions').map(compactionDecisionItem),
    filesRead: arrayField(compacted, 'files_read').map(compactionFileReadItem),
    filesChanged: arrayField(compacted, 'files_changed').map(compactionFileChangedItem),
    verifications: arrayField(compacted, 'verifications').map(compactionVerificationItem),
    openTodos: arrayField(compacted, 'open_todos').map(compactionTodoItem),
    approvals: arrayField(compacted, 'approvals').map(compactionApprovalItem),
    untrustedInputs: arrayField(compacted, 'untrusted_inputs').map(compactionUntrustedItem),
  };
}

function arrayField(record: Record<string, unknown>, key: string): unknown[] {
  const value = record[key];
  return Array.isArray(value) ? value : [];
}

function compactionDecisionItem(value: unknown) {
  if (!isRecord(value)) return { label: compactLabel(value) };
  const label = textField(value, 'summary') ?? compactLabel(value);
  const turn = numberField(value, 'turn_id');
  return {
    label,
    detail: turn > 0 ? `turn ${turn}` : undefined,
  };
}

function compactionFileReadItem(value: unknown) {
  if (!isRecord(value)) return { label: compactLabel(value) };
  const path = pathField(value, 'path');
  const [line, endLine] = lineRangeField(value, 'line_range');
  const digest = textField(value, 'digest');
  return {
    label: path ?? compactLabel(value),
    detail: digest ? `digest ${digest.slice(0, 12)}` : undefined,
    path,
    line,
    endLine,
  };
}

function compactionFileChangedItem(value: unknown) {
  if (!isRecord(value)) return { label: compactLabel(value) };
  const path = pathField(value, 'path');
  const tool = textField(value, 'tool');
  const before = textField(value, 'before_digest');
  const after = textField(value, 'after_digest');
  const digest = before || after ? `${shortDigest(before)} -> ${shortDigest(after)}` : undefined;
  return {
    label: path ?? compactLabel(value),
    detail: [tool, digest].filter(Boolean).join(' · ') || undefined,
    path,
  };
}

function compactionVerificationItem(value: unknown) {
  if (!isRecord(value)) return { label: compactLabel(value) };
  const kind = textField(value, 'kind') ?? 'verification';
  const passed = booleanField(value, 'passed');
  const summary = textField(value, 'summary');
  return {
    label: kind,
    detail: [passed === undefined ? undefined : passed ? 'passed' : 'failed', summary]
      .filter(Boolean)
      .join(' · ') || undefined,
  };
}

function compactionTodoItem(value: unknown) {
  if (!isRecord(value)) return { label: compactLabel(value) };
  const text = textField(value, 'text') ?? compactLabel(value);
  const status = textField(value, 'status');
  const id = numberField(value, 'id');
  return {
    label: text,
    detail: [id > 0 ? `#${id}` : undefined, status].filter(Boolean).join(' · ') || undefined,
  };
}

function compactionApprovalItem(value: unknown) {
  if (!isRecord(value)) return { label: compactLabel(value) };
  const tool = textField(value, 'tool') ?? 'approval';
  const scope = textField(value, 'scope');
  const detail = textField(value, 'detail');
  return {
    label: tool,
    detail: [scope, detail].filter(Boolean).join(' · ') || undefined,
  };
}

function compactionUntrustedItem(value: unknown) {
  if (!isRecord(value)) return { label: compactLabel(value) };
  const label = textField(value, 'label') ?? compactLabel(value);
  return {
    label,
    detail: textField(value, 'kind'),
  };
}

function textField(record: Record<string, unknown>, key: string): string | undefined {
  const value = record[key];
  return typeof value === 'string' && value.trim().length > 0 ? value.trim() : undefined;
}

function pathField(record: Record<string, unknown>, key: string): string | undefined {
  const value = record[key];
  if (typeof value === 'string' && value.trim().length > 0) return value;
  return undefined;
}

function lineRangeField(record: Record<string, unknown>, key: string): [number | undefined, number | undefined] {
  const value = record[key];
  if (!Array.isArray(value) || value.length < 1) return [undefined, undefined];
  const start = typeof value[0] === 'number' && Number.isFinite(value[0]) ? value[0] : undefined;
  const end = typeof value[1] === 'number' && Number.isFinite(value[1]) ? value[1] : undefined;
  return [start, end];
}

function shortDigest(value: string | undefined): string | undefined {
  return value ? value.slice(0, 12) : undefined;
}

function compactLabel(value: unknown): string {
  const text = json(value);
  return text.length > 120 ? `${text.slice(0, 117)}...` : text;
}

function questionForAskUser(request: unknown): string {
  if (isRecord(request) && typeof request.question === 'string') {
    return request.question;
  }
  return 'Peridot needs your input';
}

function compactToolFailureSummary(message: string): string {
  const normalized = message.trim();
  if (!normalized) return 'failed';
  const toolErrorIndex = normalized.toLowerCase().indexOf('tool error:');
  if (toolErrorIndex >= 0) {
    return normalized.slice(toolErrorIndex);
  }
  const firstLine = normalized.split(/\r?\n/, 1)[0]?.trim();
  return firstLine ? `failed: ${firstLine}` : 'failed';
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
      const output = result.output;
      if (isRecord(output) && typeof output.workspace_mutated === 'boolean') {
        return `${summary} · mutated=${output.workspace_mutated}`;
      }
      return summary;
    }
  }
  return json(event.result ?? event);
}

function isApprovalWaitingEvent(event: unknown): boolean {
  return (
    isRecord(event) &&
    (event.kind === 'approval_requested' || event.kind === 'approval_waiting')
  );
}

function approvalPayloadForEvent(event: Record<string, unknown>): Record<string, unknown> | undefined {
  if (approvalRecord(event)) return event;
  if (event.kind === 'approval_waiting') {
    return approvalRecord(event.request);
  }
  return undefined;
}

function approvalRecord(value: unknown): Record<string, unknown> | undefined {
  return isRecord(value) && typeof value.tool_name === 'string' ? value : undefined;
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

function booleanField(record: Record<string, unknown>, key: string): boolean | undefined {
  const value = record[key];
  return typeof value === 'boolean' ? value : undefined;
}

function pickString(value: unknown, key: string): string | undefined {
  if (!isRecord(value)) return undefined;
  const inner = value[key];
  return typeof inner === 'string' ? inner : undefined;
}

function pickNumber(value: unknown, key: string): number | undefined {
  if (!isRecord(value)) return undefined;
  const inner = value[key];
  return typeof inner === 'number' && Number.isFinite(inner) ? inner : undefined;
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

function queueId(): string {
  return `q-${Date.now()}-${Math.floor(Math.random() * 1e6)}`;
}

/**
 * Build a short placeholder title from the user's first task.
 *
 * This is *only* used as an immediate visual placeholder while
 * `session.generate_title` runs in the background. The final title is set by
 * `applyGeneratedTitle` — either the LLM's reply or `"No title"` if that
 * call fails. Do not treat this truncation as the final fallback; that's
 * `"No title"`, per the documented session-title contract.
 */
function taskTitle(task: string): string {
  const title = task.replace(/\s+/g, ' ').trim();
  return title.length > 42 ? `${title.slice(0, 39)}...` : title || 'New session';
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
