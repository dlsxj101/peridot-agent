// Minimal webview internationalization. The locale is injected by the host
// (see sidebar.ts html()) as `data-locale` on #app, derived from
// vscode.env.language. We use an inline `t(en, ko)` form rather than a key
// table: the translation lives at the call site, so there are no missing-key
// bugs and wrapping a string is a local edit.

let useKorean = false;

/** Reads the host-injected locale from #app once at startup. Accesses the DOM
 *  through `globalThis` so this module compiles in non-DOM test builds too. */
export function initLocale(): void {
  const doc = (
    globalThis as unknown as {
      document?: { getElementById(id: string): { dataset?: { locale?: string } } | null };
    }
  ).document;
  const locale = doc?.getElementById('app')?.dataset?.locale;
  useKorean = locale === 'ko';
}

/** For tests / explicit control. */
export function setKoreanLocale(enabled: boolean): void {
  useKorean = enabled;
}

/** Returns the Korean string when the locale is Korean, else English. */
export function t(en: string, ko: string): string {
  return useKorean ? ko : en;
}

/** Like `t`, with `{name}` placeholder interpolation. */
export function tf(en: string, ko: string, vars: Record<string, string | number>): string {
  let out = useKorean ? ko : en;
  for (const [key, value] of Object.entries(vars)) {
    out = out.split(`{${key}}`).join(String(value));
  }
  return out;
}
