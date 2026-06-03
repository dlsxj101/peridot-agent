import { diffLines } from 'diff';
import { el } from './util';
import { t, tf } from './i18n';

const COLLAPSE_THRESHOLD = 24; // Lines visible before the diff collapses.

export interface DiffStats {
  added: number;
  removed: number;
}

export function diffStats(before: string | null | undefined, after: string | undefined): DiffStats {
  const beforeText = typeof before === 'string' ? before : '';
  const afterText = typeof after === 'string' ? after : '';
  const parts = diffLines(beforeText, afterText);
  let added = 0;
  let removed = 0;
  for (const part of parts) {
    const lines = part.count ?? part.value.split('\n').length - 1;
    if (part.added) added += lines;
    else if (part.removed) removed += lines;
  }
  return { added, removed };
}

export function renderUnifiedDiff(
  before: string | null | undefined,
  after: string | undefined,
  path?: string,
): HTMLElement {
  const root = el('div', 'diff-pre-wrap');
  const pre = el('pre', 'diff-pre');
  const beforeText = typeof before === 'string' ? before : '';
  const afterText = typeof after === 'string' ? after : '';
  const parts = diffLines(beforeText, afterText);

  if (path) {
    const meta = el('span', 'diff-line meta', `--- ${path}`);
    pre.append(meta, document.createTextNode('\n'));
    pre.append(el('span', 'diff-line meta', `+++ ${path}`), document.createTextNode('\n'));
  }

  let lineCount = 0;
  for (const part of parts) {
    const lines = part.value.split('\n');
    if (lines.length > 0 && lines[lines.length - 1] === '') {
      lines.pop();
    }
    for (const line of lines) {
      lineCount += 1;
      const cls = part.added ? 'diff-line add' : part.removed ? 'diff-line del' : 'diff-line';
      const prefix = part.added ? '+' : part.removed ? '-' : ' ';
      const span = el('span', cls, prefix + line);
      pre.append(span, document.createTextNode('\n'));
    }
  }

  root.append(pre);

  if (lineCount > COLLAPSE_THRESHOLD) {
    pre.style.maxHeight = '120px';
    const toggle = el('button', 'diff-toggle', tf('Expand ({n} lines)', '펼치기 ({n}줄)', { n: lineCount }));
    toggle.type = 'button';
    toggle.addEventListener('click', () => {
      if (pre.style.maxHeight === '120px') {
        pre.style.maxHeight = 'none';
        toggle.textContent = t('Collapse', '접기');
      } else {
        pre.style.maxHeight = '120px';
        toggle.textContent = tf('Expand ({n} lines)', '펼치기 ({n}줄)', { n: lineCount });
      }
    });
    root.append(toggle);
  }

  return root;
}
