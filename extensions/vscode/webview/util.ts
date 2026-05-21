export function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null;
}

export function stringField(record: Record<string, unknown>, key: string): string {
  const value = record[key];
  return typeof value === 'string' ? value : json(value);
}

export function numberField(record: Record<string, unknown>, key: string): number {
  const value = record[key];
  return typeof value === 'number' ? value : 0;
}

export function json(value: unknown): string {
  try {
    const serialized = JSON.stringify(value);
    return serialized === undefined ? String(value) : serialized;
  } catch {
    return String(value);
  }
}

export function el<K extends keyof HTMLElementTagNameMap>(
  tag: K,
  className?: string,
  text?: string,
): HTMLElementTagNameMap[K] {
  const node = document.createElement(tag);
  if (className) node.className = className;
  if (text !== undefined) node.textContent = text;
  return node;
}

export function formatTokens(n: number | undefined): string {
  if (typeof n !== 'number' || !Number.isFinite(n)) return '0';
  if (n < 1000) return String(n);
  if (n < 1_000_000) return `${(n / 1000).toFixed(1)}k`;
  return `${(n / 1_000_000).toFixed(2)}M`;
}

export function formatUsd(n: number | undefined): string {
  if (typeof n !== 'number' || !Number.isFinite(n)) return '$0.00';
  if (n < 0.01) return `$${n.toFixed(4)}`;
  return `$${n.toFixed(2)}`;
}

const HIGHLIGHT_LIMIT = 16_000;
const HTML_ESCAPE: Record<string, string> = {
  '&': '&amp;',
  '<': '&lt;',
  '>': '&gt;',
  '"': '&quot;',
  "'": '&#39;',
};

export function escapeHtml(text: string): string {
  return text.replace(/[&<>"']/g, (ch) => HTML_ESCAPE[ch] ?? ch);
}

/**
 * Lite JSON-ish syntax highlighter. Walks the input once and emits
 * `<span class="tok-…">…</span>` markup for the obvious tokens — keys,
 * string literals, numbers, booleans/null, punctuation. Anything that
 * doesn't fall into those buckets renders unstyled. Falls back to plain
 * HTML-escape when the input is huge so we never block the webview
 * thread on a megabyte tool dump.
 */
export function highlightLite(raw: string): string {
  if (typeof raw !== 'string') return '';
  if (raw.length > HIGHLIGHT_LIMIT) return escapeHtml(raw);
  let out = '';
  let i = 0;
  const len = raw.length;
  while (i < len) {
    const ch = raw[i];
    // String literal (handle escapes).
    if (ch === '"' || ch === "'") {
      const quote = ch;
      let j = i + 1;
      while (j < len) {
        if (raw[j] === '\\' && j + 1 < len) {
          j += 2;
          continue;
        }
        if (raw[j] === quote) {
          j += 1;
          break;
        }
        j += 1;
      }
      const lit = raw.slice(i, j);
      // Is the next non-whitespace char a colon? Then this string is a key.
      let k = j;
      while (k < len && (raw[k] === ' ' || raw[k] === '\t')) k += 1;
      const cls = raw[k] === ':' ? 'tok-key' : 'tok-string';
      out += `<span class="${cls}">${escapeHtml(lit)}</span>`;
      i = j;
      continue;
    }
    // Number literal (positive or negative, decimal, exponent).
    if ((ch >= '0' && ch <= '9') || (ch === '-' && raw[i + 1] >= '0' && raw[i + 1] <= '9')) {
      let j = i + 1;
      while (j < len && /[0-9eE+.\-]/.test(raw[j])) j += 1;
      out += `<span class="tok-number">${escapeHtml(raw.slice(i, j))}</span>`;
      i = j;
      continue;
    }
    // Keywords (true / false / null).
    if (ch === 't' || ch === 'f' || ch === 'n') {
      const rest = raw.slice(i, i + 5);
      if (rest.startsWith('true') && !/[A-Za-z0-9_]/.test(raw[i + 4] ?? '')) {
        out += `<span class="tok-bool">true</span>`;
        i += 4;
        continue;
      }
      if (rest.startsWith('false') && !/[A-Za-z0-9_]/.test(raw[i + 5] ?? '')) {
        out += `<span class="tok-bool">false</span>`;
        i += 5;
        continue;
      }
      if (rest.startsWith('null') && !/[A-Za-z0-9_]/.test(raw[i + 4] ?? '')) {
        out += `<span class="tok-bool">null</span>`;
        i += 4;
        continue;
      }
    }
    // Punctuation.
    if (',[]{}:'.includes(ch)) {
      out += `<span class="tok-punct">${escapeHtml(ch)}</span>`;
      i += 1;
      continue;
    }
    out += escapeHtml(ch);
    i += 1;
  }
  return out;
}
