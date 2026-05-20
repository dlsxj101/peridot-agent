import { build, context } from 'esbuild';

const watch = process.argv.includes('--watch');
const production = process.argv.includes('--production');

/** @type {import('esbuild').BuildOptions} */
const common = {
  bundle: true,
  sourcemap: !production,
  minify: production,
  logLevel: 'info',
};

/** @type {import('esbuild').BuildOptions} */
const extensionConfig = {
  ...common,
  entryPoints: ['src/extension.ts'],
  outfile: 'dist/extension.js',
  platform: 'node',
  target: 'node18',
  format: 'cjs',
  external: ['vscode'],
};

/** @type {import('esbuild').BuildOptions} */
const webviewConfig = {
  ...common,
  entryPoints: ['webview/index.ts'],
  outfile: 'dist/webview.js',
  platform: 'browser',
  target: 'es2022',
  format: 'iife',
  loader: { '.css': 'css' },
};

async function run() {
  if (watch) {
    const ext = await context(extensionConfig);
    const web = await context(webviewConfig);
    await Promise.all([ext.watch(), web.watch()]);
    console.log('[esbuild] watching extension + webview bundles');
    return;
  }
  await Promise.all([build(extensionConfig), build(webviewConfig)]);
}

run().catch((err) => {
  console.error(err);
  process.exit(1);
});
