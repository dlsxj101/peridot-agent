// Peridot Agent — VS Code extension entry point.
//
// v0.0.1 surface: two commands ("Peridot: Hello", "Peridot: Check
// Daemon Version") that prove the publish + spawn + JSON-RPC pipeline
// end-to-end. Real chat / diff / approval UI lands in v0.1.0 once this
// foundation is verified in the Marketplace.

import * as vscode from 'vscode';
import { PeridotDaemon } from './daemon';

export function activate(context: vscode.ExtensionContext) {
  // v0.0.1 sanity command — exists purely so a user can verify the
  // extension installed correctly without spawning the daemon.
  context.subscriptions.push(
    vscode.commands.registerCommand('peridot.hello', async () => {
      await vscode.window.showInformationMessage(
        'Hello from Peridot Agent — extension installed correctly.',
      );
    }),
  );

  // Round-trips the daemon to confirm the JSON-RPC pipeline works.
  // Surfaces both the extension and daemon versions to the operator
  // so version mismatches between the .vsix and the bundled binary
  // are visible immediately.
  context.subscriptions.push(
    vscode.commands.registerCommand('peridot.checkVersion', async () => {
      const folder = vscode.workspace.workspaceFolders?.[0]?.uri.fsPath;
      if (!folder) {
        vscode.window.showWarningMessage(
          'Open a workspace folder before checking the Peridot daemon.',
        );
        return;
      }
      try {
        const daemon = await PeridotDaemon.spawn(folder);
        try {
          const result = (await daemon.send('peridot.version')) as { version: string };
          const extensionVersion =
            vscode.extensions.getExtension('dlsxj101.peridot-vscode')?.packageJSON?.version ??
            'unknown';
          await vscode.window.showInformationMessage(
            `Peridot daemon ${result.version} (extension ${extensionVersion}).`,
          );
        } finally {
          await daemon.shutdown();
        }
      } catch (err) {
        const message = err instanceof Error ? err.message : String(err);
        await vscode.window.showErrorMessage(`Peridot daemon spawn failed: ${message}`);
      }
    }),
  );
}

export function deactivate() {
  // No long-lived daemon to tear down in v0.0.1 — each command
  // spawns + shuts down a fresh process. v0.1.0 introduces a
  // persistent daemon and registers it on this lifecycle hook.
}
