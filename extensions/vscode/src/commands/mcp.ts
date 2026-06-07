// `/mcp` command handlers for the extension host.
//
// List / add / test / remove the workspace's configured MCP servers by driving
// the matching `/mcp …` daemon slash command and rendering the result into the
// sidebar transcript. Split out of `extension.ts`; the public handlers take the
// shared output channel + sidebar and reuse the host's now-exported
// runSlashCommand / refreshStatus helpers. The pickMcpServer helper stays
// private to this module.

import * as vscode from 'vscode';

import { refreshStatus, runSlashCommand } from '../extension';
import {
  mcpAddSlashCommand,
  mcpRemoveSlashCommand,
  mcpServerChoices,
  mcpTestSlashCommand,
  type McpTransport,
} from '../mcpCommand';
import type { PeridotSidebarProvider } from '../sidebar';
import type { McpServerSummary } from '../types';

export async function showMcpServers(
  output: vscode.OutputChannel,
  sidebar: PeridotSidebarProvider,
): Promise<void> {
  const folder = vscode.workspace.workspaceFolders?.[0]?.uri.fsPath;
  if (!folder) {
    const message = 'Open a workspace folder before showing MCP servers.';
    sidebar.setWorkspaceProblem(message);
    await vscode.window.showWarningMessage(message);
    return;
  }
  await vscode.commands.executeCommand('peridot.chatView.focus');
  try {
    const result = await runSlashCommand('/mcp list', output, sidebar, sidebar.currentRunOptions());
    sidebar.appendCommandResult(result);
  } catch (err) {
    const message = err instanceof Error ? err.message : String(err);
    output.appendLine(`[peridot] mcp list failed: ${message}`);
    sidebar.appendError(message);
    await vscode.window.showErrorMessage(vscode.l10n.t('Peridot MCP server list failed: {0}', message));
  }
}

export async function addMcpServer(
  output: vscode.OutputChannel,
  sidebar: PeridotSidebarProvider,
): Promise<void> {
  const folder = vscode.workspace.workspaceFolders?.[0]?.uri.fsPath;
  if (!folder) {
    const message = 'Open a workspace folder before adding MCP servers.';
    sidebar.setWorkspaceProblem(message);
    await vscode.window.showWarningMessage(message);
    return;
  }
  const existingNames = new Set(sidebar.currentMcpServers().map((server) => server.name));
  const name = await vscode.window.showInputBox({
    title: 'Peridot: Add MCP Server',
    prompt: 'Enter a unique MCP server name.',
    placeHolder: 'filesystem',
    ignoreFocusOut: true,
    validateInput: (value) => {
      const trimmed = value.trim();
      if (!trimmed) return 'MCP server name is required.';
      if (/\s/.test(trimmed)) return 'MCP server name cannot contain whitespace.';
      if (existingNames.has(trimmed)) return 'An MCP server with this name already exists.';
      return undefined;
    },
  });
  if (!name) return;
  const transport = await vscode.window.showQuickPick(
    [
      {
        label: 'stdio',
        description: 'Run a local command that speaks MCP over stdio',
        transport: 'stdio' as McpTransport,
      },
      {
        label: 'http',
        description: 'Connect to an HTTP/SSE MCP endpoint',
        transport: 'http' as McpTransport,
      },
    ],
    {
      title: 'Peridot: Add MCP Server',
      placeHolder: 'Choose the MCP transport',
      ignoreFocusOut: true,
    },
  );
  if (!transport) return;
  const target = await vscode.window.showInputBox({
    title: 'Peridot: Add MCP Server',
    prompt:
      transport.transport === 'stdio'
        ? 'Enter the command and args to start the MCP server.'
        : 'Enter the MCP server URL.',
    placeHolder:
      transport.transport === 'stdio'
        ? 'npx -y @modelcontextprotocol/server-filesystem .'
        : 'https://example.com/mcp',
    ignoreFocusOut: true,
    validateInput: (value) => {
      const trimmed = value.trim();
      if (!trimmed) return 'MCP server target is required.';
      if (/[\r\n]/.test(trimmed)) return 'MCP server target must be a single line.';
      if (transport.transport === 'http' && !/^https?:\/\//i.test(trimmed)) {
        return 'HTTP MCP server URL must start with http:// or https://.';
      }
      return undefined;
    },
  });
  if (!target) return;
  let command: string;
  try {
    command = mcpAddSlashCommand(name, transport.transport, target);
  } catch (err) {
    const message = err instanceof Error ? err.message : String(err);
    await vscode.window.showErrorMessage(vscode.l10n.t('Peridot MCP server add failed: {0}', message));
    return;
  }
  await vscode.commands.executeCommand('peridot.chatView.focus');
  try {
    const result = await runSlashCommand(command, output, sidebar, sidebar.currentRunOptions());
    sidebar.appendCommandResult(result);
    await refreshStatus(output, sidebar, { force: true });
  } catch (err) {
    const message = err instanceof Error ? err.message : String(err);
    output.appendLine(`[peridot] mcp add failed: ${message}`);
    sidebar.appendError(message);
    await vscode.window.showErrorMessage(vscode.l10n.t('Peridot MCP server add failed: {0}', message));
  }
}

export async function testMcpServer(
  output: vscode.OutputChannel,
  sidebar: PeridotSidebarProvider,
): Promise<void> {
  const server = await pickMcpServer(output, sidebar, {
    title: 'Peridot: Test MCP Server',
    placeHolder: 'Choose a configured MCP server to test',
  });
  if (!server) return;
  let command: string;
  try {
    command = mcpTestSlashCommand(server.name);
  } catch (err) {
    const message = err instanceof Error ? err.message : String(err);
    await vscode.window.showErrorMessage(vscode.l10n.t('Peridot MCP server test failed: {0}', message));
    return;
  }
  await vscode.commands.executeCommand('peridot.chatView.focus');
  try {
    const result = await vscode.window.withProgress(
      {
        location: vscode.ProgressLocation.Notification,
        title: `Peridot: testing MCP server ${server.name}`,
        cancellable: false,
      },
      async () => runSlashCommand(command, output, sidebar, sidebar.currentRunOptions()),
    );
    sidebar.appendCommandResult(result);
  } catch (err) {
    const message = err instanceof Error ? err.message : String(err);
    output.appendLine(`[peridot] mcp test failed: ${message}`);
    sidebar.appendError(message);
    await vscode.window.showErrorMessage(vscode.l10n.t('Peridot MCP server test failed: {0}', message));
  }
}

export async function removeMcpServer(
  output: vscode.OutputChannel,
  sidebar: PeridotSidebarProvider,
): Promise<void> {
  const server = await pickMcpServer(output, sidebar, {
    title: 'Peridot: Remove MCP Server',
    placeHolder: 'Choose a configured MCP server to remove',
  });
  if (!server) return;
  const confirmation = await vscode.window.showWarningMessage(
    vscode.l10n.t('Remove MCP server "{0}" from this workspace config?', server.name),
    { modal: true },
    'Remove',
  );
  if (confirmation !== 'Remove') return;
  let command: string;
  try {
    command = mcpRemoveSlashCommand(server.name);
  } catch (err) {
    const message = err instanceof Error ? err.message : String(err);
    await vscode.window.showErrorMessage(vscode.l10n.t('Peridot MCP server removal failed: {0}', message));
    return;
  }
  await vscode.commands.executeCommand('peridot.chatView.focus');
  try {
    const result = await vscode.window.withProgress(
      {
        location: vscode.ProgressLocation.Notification,
        title: `Peridot: removing MCP server ${server.name}`,
        cancellable: false,
      },
      async () => runSlashCommand(command, output, sidebar, sidebar.currentRunOptions()),
    );
    sidebar.appendCommandResult(result);
    await refreshStatus(output, sidebar, { force: true });
  } catch (err) {
    const message = err instanceof Error ? err.message : String(err);
    output.appendLine(`[peridot] mcp remove failed: ${message}`);
    sidebar.appendError(message);
    await vscode.window.showErrorMessage(vscode.l10n.t('Peridot MCP server removal failed: {0}', message));
  }
}

async function pickMcpServer(
  output: vscode.OutputChannel,
  sidebar: PeridotSidebarProvider,
  options: { title: string; placeHolder: string },
): Promise<McpServerSummary | undefined> {
  const folder = vscode.workspace.workspaceFolders?.[0]?.uri.fsPath;
  if (!folder) {
    const message = 'Open a workspace folder before selecting MCP servers.';
    sidebar.setWorkspaceProblem(message);
    await vscode.window.showWarningMessage(message);
    return undefined;
  }
  let choices = mcpServerChoices(sidebar.currentMcpServers() ?? []);
  if (choices.length === 0) {
    await refreshStatus(output, sidebar, { force: true });
    choices = mcpServerChoices(sidebar.currentMcpServers() ?? []);
  }
  if (choices.length === 0) {
    await vscode.window.showWarningMessage(vscode.l10n.t('No MCP servers are configured for this workspace.'));
    return undefined;
  }
  if (choices.length === 1) return { name: choices[0].name };
  return vscode.window.showQuickPick(
    choices.map((choice) => ({
      label: choice.label,
      description: choice.description,
      name: choice.name,
    })),
    {
      title: options.title,
      placeHolder: options.placeHolder,
      ignoreFocusOut: true,
    },
  );
}
