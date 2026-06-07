// Ship / merge command handlers for the extension host.
//
// `shipChangesToPr` drives `peridot ship` (commit + push + optional PR) with a
// dry-run preview/confirm step, and `mergeGitHubPr` shells out to `gh pr merge`.
// Split out of `extension.ts`; both reuse the shared CLI helpers from ./cli and
// the host's exported refreshStatus. The prompt/arg/preview/result-shaping
// helpers stay private to this module.

import * as vscode from 'vscode';

import { refreshStatus } from '../extension';
import type { PeridotSidebarProvider } from '../sidebar';
import type { CommandResultView } from '../types';
import { execFile, execPeridotCli, nonEmpty, parseJson } from './cli';

export async function shipChangesToPr(
  output: vscode.OutputChannel,
  sidebar: PeridotSidebarProvider,
): Promise<void> {
  const folder = vscode.workspace.workspaceFolders?.[0]?.uri.fsPath;
  if (!folder) {
    const message = 'Open a workspace folder before shipping changes.';
    sidebar.setWorkspaceProblem(message);
    await vscode.window.showWarningMessage(message);
    return;
  }
  const options = await promptShipOptions();
  if (!options) return;

  const previewArgs = buildShipArgs(options, true);
  await vscode.commands.executeCommand('peridot.chatView.focus');
  try {
    const preview = await vscode.window.withProgress(
      {
        location: vscode.ProgressLocation.Window,
        title: 'Peridot: previewing ship plan',
      },
      async () => execPeridotCli(previewArgs, folder),
    );
    const previewJson = parseJson(preview.stdout);
    const confirmed = await vscode.window.showWarningMessage(
      shipPreviewText(previewJson),
      { modal: true },
      'Ship Changes',
    );
    if (confirmed !== 'Ship Changes') {
      sidebar.appendCommandResult(shipResultView(previewJson, 'Ship Preview', '/ship --dry-run'));
      return;
    }

    const result = await vscode.window.withProgress(
      {
        location: vscode.ProgressLocation.Notification,
        title: 'Peridot: shipping changes to PR',
      },
      async () => execPeridotCli(buildShipArgs(options, false), folder),
    );
    sidebar.appendCommandResult(shipResultView(parseJson(result.stdout), 'Ship Changes', 'peridot ship'));
    void refreshStatus(output, sidebar, { force: true });
  } catch (err) {
    const message = err instanceof Error ? err.message : String(err);
    output.appendLine(`[peridot] ship failed: ${message}`);
    sidebar.appendError(message);
    await vscode.window.showErrorMessage(vscode.l10n.t('Peridot ship failed: {0}', message));
  }
}

export async function showGitHubPrStatus(
  output: vscode.OutputChannel,
  sidebar: PeridotSidebarProvider,
): Promise<void> {
  const folder = vscode.workspace.workspaceFolders?.[0]?.uri.fsPath;
  if (!folder) {
    const message = 'Open a workspace folder before checking GitHub PR status.';
    sidebar.setWorkspaceProblem(message);
    await vscode.window.showWarningMessage(message);
    return;
  }
  await vscode.commands.executeCommand('peridot.chatView.focus');
  try {
    const { stdout, stderr } = await vscode.window.withProgress(
      {
        location: vscode.ProgressLocation.Window,
        title: 'Peridot: checking GitHub PR status',
      },
      async () => execFile('gh', ['pr', 'status'], folder),
    );
    sidebar.appendCommandResult({
      kind: 'pr_status',
      title: 'GitHub PR Status',
      message: (stdout || stderr || 'No PR status output.').trim(),
      severity: 'info',
      command: 'gh pr status',
    });
  } catch (err) {
    const message = err instanceof Error ? err.message : String(err);
    output.appendLine(`[peridot] gh pr status failed: ${message}`);
    sidebar.appendError(message);
    await vscode.window.showErrorMessage(vscode.l10n.t('GitHub PR status failed: {0}', message));
  }
}

export async function mergeGitHubPr(
  output: vscode.OutputChannel,
  sidebar: PeridotSidebarProvider,
): Promise<void> {
  const folder = vscode.workspace.workspaceFolders?.[0]?.uri.fsPath;
  if (!folder) {
    const message = 'Open a workspace folder before merging a GitHub PR.';
    sidebar.setWorkspaceProblem(message);
    await vscode.window.showWarningMessage(message);
    return;
  }
  const options = await promptMergeOptions();
  if (!options) return;
  const args = buildMergeArgs(options);
  const confirmed = await vscode.window.showWarningMessage(
    mergePreviewText(options),
    { modal: true },
    'Merge PR',
  );
  if (confirmed !== 'Merge PR') return;

  await vscode.commands.executeCommand('peridot.chatView.focus');
  try {
    const { stdout, stderr } = await vscode.window.withProgress(
      {
        location: vscode.ProgressLocation.Notification,
        title: 'Peridot: merging GitHub PR',
      },
      async () => execFile('gh', args, folder),
    );
    sidebar.appendCommandResult({
      kind: 'pr_merge',
      title: 'GitHub PR Merge',
      message: (stdout || stderr || 'PR merge completed.').trim(),
      severity: 'info',
      command: `gh ${args.join(' ')}`,
    });
    void refreshStatus(output, sidebar, { force: true });
  } catch (err) {
    const message = err instanceof Error ? err.message : String(err);
    output.appendLine(`[peridot] gh pr merge failed: ${message}`);
    sidebar.appendError(message);
    await vscode.window.showErrorMessage(vscode.l10n.t('GitHub PR merge failed: {0}', message));
  }
}

interface ShipPromptOptions {
  branch?: string;
  message?: string;
  prTitle?: string;
  prBody?: string;
  draft: boolean;
  noPr: boolean;
}

async function promptShipOptions(): Promise<ShipPromptOptions | undefined> {
  const message = await vscode.window.showInputBox({
    title: 'Peridot: Ship Changes',
    prompt: 'Commit message. Leave empty for the Peridot default.',
    ignoreFocusOut: true,
  });
  if (message === undefined) return undefined;
  const branch = await vscode.window.showInputBox({
    title: 'Peridot: Ship Changes',
    prompt: 'Target branch. Leave empty for peridot/ship-<time>.',
    ignoreFocusOut: true,
  });
  if (branch === undefined) return undefined;
  const prTitle = await vscode.window.showInputBox({
    title: 'Peridot: Ship Changes',
    prompt: 'PR title. Leave empty to use the commit message.',
    ignoreFocusOut: true,
  });
  if (prTitle === undefined) return undefined;
  const prBody = await vscode.window.showInputBox({
    title: 'Peridot: Ship Changes',
    prompt: 'PR body. Leave empty for the Peridot default.',
    ignoreFocusOut: true,
  });
  if (prBody === undefined) return undefined;
  const prMode = await vscode.window.showQuickPick(
    [
      { label: 'Draft PR', description: 'Recommended', value: 'draft' },
      { label: 'Ready PR', description: 'Open as ready for review', value: 'ready' },
      { label: 'No PR', description: 'Commit and push only', value: 'none' },
    ],
    {
      title: 'Peridot: Ship Changes',
      placeHolder: 'Choose how Peridot should handle the pull request.',
      ignoreFocusOut: true,
    },
  );
  if (!prMode) return undefined;
  return {
    branch: nonEmpty(branch),
    message: nonEmpty(message),
    prTitle: nonEmpty(prTitle),
    prBody: nonEmpty(prBody),
    draft: prMode.value === 'draft',
    noPr: prMode.value === 'none',
  };
}

function buildShipArgs(options: ShipPromptOptions, dryRun: boolean): string[] {
  const args = ['--output', 'json', 'ship'];
  if (dryRun) args.push('--dry-run');
  if (options.branch) args.push('--branch', options.branch);
  if (options.message) args.push('--message', options.message);
  if (options.prTitle) args.push('--pr-title', options.prTitle);
  if (options.prBody) args.push('--pr-body', options.prBody);
  if (options.draft) args.push('--draft');
  if (options.noPr) args.push('--no-pr');
  return args;
}

interface MergePromptOptions {
  pr?: string;
  method: 'merge' | 'squash' | 'rebase';
  keepBranch: boolean;
}

async function promptMergeOptions(): Promise<MergePromptOptions | undefined> {
  const pr = await vscode.window.showInputBox({
    title: 'Peridot: Merge GitHub PR',
    prompt: 'PR number or URL. Leave empty for the PR linked to the current branch.',
    ignoreFocusOut: true,
  });
  if (pr === undefined) return undefined;
  const method = await vscode.window.showQuickPick(
    [
      { label: 'Merge commit', description: 'Preserve branch commits', value: 'merge' },
      { label: 'Squash', description: 'Collapse into one commit', value: 'squash' },
      { label: 'Rebase', description: 'Replay commits onto base', value: 'rebase' },
    ],
    {
      title: 'Peridot: Merge GitHub PR',
      placeHolder: 'Choose merge strategy.',
      ignoreFocusOut: true,
    },
  );
  if (!method) return undefined;
  const branch = await vscode.window.showQuickPick(
    [
      { label: 'Delete branch', description: 'Recommended after merge', value: 'delete' },
      { label: 'Keep branch', description: 'Leave the remote branch in place', value: 'keep' },
    ],
    {
      title: 'Peridot: Merge GitHub PR',
      placeHolder: 'Choose branch cleanup behavior.',
      ignoreFocusOut: true,
    },
  );
  if (!branch) return undefined;
  return {
    pr: nonEmpty(pr),
    method: method.value as MergePromptOptions['method'],
    keepBranch: branch.value === 'keep',
  };
}

function buildMergeArgs(options: MergePromptOptions): string[] {
  const args = ['pr', 'merge'];
  if (options.pr) args.push(options.pr);
  args.push(`--${options.method}`);
  if (!options.keepBranch) args.push('--delete-branch');
  return args;
}

function mergePreviewText(options: MergePromptOptions): string {
  return [
    'Peridot will merge a GitHub pull request.',
    '',
    `PR: ${options.pr ?? 'current branch PR'}`,
    `Method: ${options.method}`,
    `Branch cleanup: ${options.keepBranch ? 'keep branch' : 'delete branch'}`,
    '',
    'This changes remote repository state.',
  ].join('\n');
}

function shipPreviewText(value: unknown): string {
  const steps = shipSteps(value);
  const lines = steps.slice(0, 6).map((step) => `- ${step.status} ${step.step}${step.detail ? `: ${step.detail}` : ''}`);
  return [
    'Peridot will run the following publish steps:',
    '',
    ...lines,
    '',
    'This will commit, push, and may open a GitHub PR.',
  ].join('\n');
}

function shipResultView(value: unknown, title: string, command: string): CommandResultView {
  const steps = shipSteps(value);
  const prUrl = steps.find((step) => step.step === 'pr' && step.url)?.url;
  return {
    kind: 'ship',
    title,
    message: prUrl ? `Ship complete: ${prUrl}` : title,
    severity: 'info',
    command,
    items: steps.map((step) => ({
      source: step.step,
      label: `${step.status} ${step.step}`,
      detail: step.detail,
    })),
  };
}

interface ShipStepView {
  step: string;
  status: string;
  detail?: string;
  url?: string;
}

function shipSteps(value: unknown): ShipStepView[] {
  const rawSteps = isRecord(value) && Array.isArray(value.steps) ? value.steps : [];
  return rawSteps.filter(isRecord).map((step) => {
    const name = typeof step.step === 'string' ? step.step : 'step';
    const status = typeof step.status === 'string' ? step.status : 'unknown';
    const detail = [
      typeof step.message === 'string' ? step.message : undefined,
      typeof step.branch === 'string' ? step.branch : undefined,
      typeof step.to === 'string' ? step.to : undefined,
      typeof step.title === 'string' ? step.title : undefined,
      typeof step.reason === 'string' ? step.reason : undefined,
      typeof step.url === 'string' ? step.url : undefined,
    ]
      .filter((entry): entry is string => Boolean(entry))
      .join(' · ');
    return {
      step: name,
      status,
      ...(detail ? { detail } : {}),
      ...(typeof step.url === 'string' ? { url: step.url } : {}),
    };
  });
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null;
}
