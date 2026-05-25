import * as vscode from 'vscode';
import { PeridotDaemon } from './daemon';

/**
 * Wire format mirroring `peridot_tui::SettingItem` over JSON-RPC.
 *
 * Kept in lock-step with `crates/peridot-tui/src/settings_screen.rs`:
 * adding a new `SettingValue` variant on the Rust side requires extending
 * the union here (and the renderer below) before the new field shows up
 * in the webview. `SettingValue` is `#[serde(tag = "kind", content =
 * "data")]` so the JSON shape is `{ kind: "Bool", data: true }`.
 */
export interface SettingItem {
  id: string;
  group: string;
  label: string;
  help?: string | null;
  value: SettingValue;
  /**
   * Surface tags from the Rust side. The webview hides items whose
   * `surfaces` doesn't include `"vscode"`. Optional for forward
   * compatibility with older daemons that didn't ship this field.
   */
  surfaces?: string[];
}

export type SettingValue =
  | { kind: 'Bool'; data: boolean }
  | { kind: 'Choice'; data: { options: string[]; selected: number } }
  | { kind: 'U32'; data: { value: number; min: number; max: number; step: number } }
  | { kind: 'F64'; data: { value: number; min: number; max: number; step: number } }
  | { kind: 'Usize'; data: { value: number; min: number; max: number; step: number } };

interface SettingsListResult {
  config_path: string;
  items: SettingItem[];
}

interface SettingsSaveResult {
  saved: boolean;
  config_path: string;
}

/**
 * Resolves a daemon for the current workspace, asking the caller-supplied
 * factory. Returning `null` (rather than throwing) keeps the openSettings
 * command idempotent — multiple invocations during a daemon-down window
 * just show a single error toast.
 */
export type DaemonResolver = () => Promise<PeridotDaemon | null>;

/**
 * Owns a single webview panel that surfaces the same curated settings the
 * TUI's `peridot setting` screen does, but as form controls in VS Code's
 * editor area. There is at most one panel per VS Code window; opening
 * the command while a panel exists reveals the existing one rather than
 * spawning a duplicate.
 */
export class SettingsPanelManager {
  private panel: vscode.WebviewPanel | undefined;

  constructor(
    private readonly extensionUri: vscode.Uri,
    private readonly output: vscode.OutputChannel,
    private readonly resolveDaemon: DaemonResolver,
  ) {}

  public async open(): Promise<void> {
    if (this.panel) {
      this.panel.reveal(vscode.ViewColumn.Active);
      return;
    }
    const panel = vscode.window.createWebviewPanel(
      'peridot.settings',
      'Peridot Settings',
      vscode.ViewColumn.Active,
      {
        enableScripts: true,
        retainContextWhenHidden: true,
        localResourceRoots: [vscode.Uri.joinPath(this.extensionUri, 'dist')],
      },
    );
    panel.iconPath = vscode.Uri.joinPath(this.extensionUri, 'resources', 'peridot-icon.png');
    panel.webview.html = this.html(panel.webview);
    panel.onDidDispose(() => {
      this.panel = undefined;
    });
    panel.webview.onDidReceiveMessage((message) => {
      void this.onMessage(panel, message);
    });
    this.panel = panel;
    await this.loadInto(panel);
  }

  private async loadInto(panel: vscode.WebviewPanel): Promise<void> {
    const daemon = await this.resolveDaemon();
    if (!daemon) {
      panel.webview.postMessage({
        type: 'load-error',
        error: 'No Peridot daemon is running for this workspace. Start a task first to launch one.',
      });
      return;
    }
    try {
      const result = (await daemon.send('settings.list')) as SettingsListResult;
      panel.webview.postMessage({
        type: 'load',
        configPath: result.config_path,
        items: result.items,
      });
    } catch (err) {
      const message = err instanceof Error ? err.message : String(err);
      this.output.appendLine(`[peridot] settings.list failed: ${message}`);
      panel.webview.postMessage({ type: 'load-error', error: message });
    }
  }

  private async onMessage(panel: vscode.WebviewPanel, message: unknown): Promise<void> {
    if (typeof message !== 'object' || message === null) {
      return;
    }
    const msg = message as { type?: string };
    if (msg.type === 'save') {
      await this.handleSave(panel, message as { items?: SettingItem[] });
      return;
    }
    if (msg.type === 'reload') {
      await this.loadInto(panel);
      return;
    }
  }

  private async handleSave(
    panel: vscode.WebviewPanel,
    message: { items?: SettingItem[] },
  ): Promise<void> {
    const items = message.items;
    if (!Array.isArray(items)) {
      panel.webview.postMessage({ type: 'save-error', error: 'invalid items payload' });
      return;
    }
    const daemon = await this.resolveDaemon();
    if (!daemon) {
      panel.webview.postMessage({
        type: 'save-error',
        error: 'No Peridot daemon is running for this workspace.',
      });
      return;
    }
    try {
      const result = (await daemon.send('settings.save', { items })) as SettingsSaveResult;
      panel.webview.postMessage({
        type: 'save-ok',
        configPath: result.config_path,
      });
    } catch (err) {
      const message = err instanceof Error ? err.message : String(err);
      this.output.appendLine(`[peridot] settings.save failed: ${message}`);
      panel.webview.postMessage({ type: 'save-error', error: message });
    }
  }

  private html(webview: vscode.Webview): string {
    const nonce = randomNonce();
    const scriptUri = webview.asWebviewUri(
      vscode.Uri.joinPath(this.extensionUri, 'dist', 'settings.js'),
    );
    const styleUri = webview.asWebviewUri(
      vscode.Uri.joinPath(this.extensionUri, 'dist', 'settings.css'),
    );
    return /* html */ `<!DOCTYPE html>
<html lang="en">
<head>
  <meta charset="UTF-8">
  <meta
    http-equiv="Content-Security-Policy"
    content="default-src 'none'; style-src ${webview.cspSource} 'unsafe-inline'; script-src 'nonce-${nonce}';"
  >
  <meta name="viewport" content="width=device-width, initial-scale=1.0">
  <title>Peridot Settings</title>
  <link href="${styleUri}" rel="stylesheet" />
</head>
<body>
  <div id="app">
    <p class="loading">Loading settings…</p>
  </div>
  <script nonce="${nonce}" src="${scriptUri}"></script>
</body>
</html>`;
  }
}

function randomNonce(): string {
  const chars =
    'ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789';
  let result = '';
  for (let i = 0; i < 32; i++) {
    result += chars.charAt(Math.floor(Math.random() * chars.length));
  }
  return result;
}
