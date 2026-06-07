// ESLint flat config for the Peridot VS Code extension.
//
// Lints the TypeScript sources in `src/` (extension host) and `webview/`
// (webview bundle) on top of `tsc`'s type checking — catching unused symbols,
// fall-through, and other correctness smells that the compiler alone misses.
// Uses the (fast, non-type-checked) typescript-eslint recommended set so a
// `npm run lint` stays quick; build outputs and deps are ignored.

import js from '@eslint/js';
import globals from 'globals';
import tseslint from 'typescript-eslint';

export default tseslint.config(
  {
    // Build artifacts, deps, and generated bundles — never linted.
    ignores: ['dist/**', 'out/**', 'out-test/**', 'node_modules/**', 'resources/**'],
  },
  js.configs.recommended,
  ...tseslint.configs.recommended,
  {
    // Node-run build / test scripts (plain ESM, not bundled).
    files: ['**/*.mjs'],
    languageOptions: { globals: globals.node },
  },
  {
    files: ['src/**/*.ts', 'webview/**/*.ts', 'test/**/*.ts', 'scripts/**/*.mjs'],
    rules: {
      // The codebase intentionally uses `any` at a few provider/JSON
      // boundaries; flag as a warning rather than blocking the build.
      '@typescript-eslint/no-explicit-any': 'warn',
      // Allow intentionally-unused args/vars when prefixed with `_`.
      '@typescript-eslint/no-unused-vars': [
        'error',
        { argsIgnorePattern: '^_', varsIgnorePattern: '^_', caughtErrorsIgnorePattern: '^_' },
      ],
    },
  },
);
