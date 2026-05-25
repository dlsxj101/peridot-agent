// Settings webview entry. Receives a SettingItem[] from the host on `load`,
// renders one row per item grouped by `group`, sends an updated array
// back on save. The schema mirrors `peridot_tui::SettingItem`'s
// serde-tagged JSON shape.

import './settings.css';

interface SettingItem {
  id: string;
  group: string;
  label: string;
  help?: string | null;
  value: SettingValue;
  /**
   * Which UI surfaces should render this row. Mirrors the Rust-side
   * `SettingItem.surfaces`. The webview only renders items where
   * `surfaces` includes `"vscode"`, hiding TUI-only knobs like the
   * deer mascot or the per-header token counter that wouldn't do
   * anything from inside VS Code. Missing field (older daemons) =
   * render everywhere for safety.
   */
  surfaces?: string[];
}

type SettingValue =
  | { kind: 'Bool'; data: boolean }
  | { kind: 'Choice'; data: { options: string[]; selected: number } }
  | { kind: 'U32'; data: { value: number; min: number; max: number; step: number } }
  | { kind: 'F64'; data: { value: number; min: number; max: number; step: number } }
  | { kind: 'Usize'; data: { value: number; min: number; max: number; step: number } };

interface VsCodeApi {
  postMessage(message: unknown): void;
}

declare function acquireVsCodeApi(): VsCodeApi;

const vscode = acquireVsCodeApi();

// Local working copy. Mutated as the user edits; sent back wholesale on
// save. Host owns the source of truth — we re-request on every panel
// open / reload to avoid drift.
let workingItems: SettingItem[] = [];
let configPath = '';

window.addEventListener('message', (event) => {
  const msg = event.data as
    | { type: 'load'; configPath: string; items: SettingItem[] }
    | { type: 'load-error'; error: string }
    | { type: 'save-ok'; configPath: string }
    | { type: 'save-error'; error: string };
  if (!msg || typeof msg !== 'object') {
    return;
  }
  switch (msg.type) {
    case 'load':
      workingItems = msg.items;
      configPath = msg.configPath;
      render();
      break;
    case 'load-error':
      renderError(msg.error);
      break;
    case 'save-ok':
      configPath = msg.configPath;
      showFlash(`Saved to ${msg.configPath}`, 'ok');
      break;
    case 'save-error':
      showFlash(`Save failed: ${msg.error}`, 'err');
      break;
  }
});

function render(): void {
  const app = document.getElementById('app');
  if (!app) {
    return;
  }
  // Filter by surfaces *before* grouping so an entire group becomes
  // empty (and thus omitted) when none of its items target the
  // VS Code surface — e.g. the TUI-only `tui.*` knobs.
  const visible = workingItems.filter(isVisibleHere);
  const groups = groupBy(visible, (item) => item.group);
  const groupOrder = Array.from(groups.keys());

  app.innerHTML = '';
  app.appendChild(buildHeader());

  for (const group of groupOrder) {
    const section = document.createElement('section');
    section.className = 'group';
    const heading = document.createElement('h2');
    heading.textContent = group;
    section.appendChild(heading);
    for (const item of groups.get(group) ?? []) {
      section.appendChild(buildRow(item));
    }
    app.appendChild(section);
  }

  app.appendChild(buildFooter());
}

/**
 * Item visibility predicate. Items without a `surfaces` field (older
 * daemons) are shown — fail-open beats hiding a real setting because
 * of a schema gap. Items with an explicit `surfaces` list must
 * include `"vscode"` to render here.
 */
function isVisibleHere(item: SettingItem): boolean {
  if (!Array.isArray(item.surfaces) || item.surfaces.length === 0) {
    return true;
  }
  return item.surfaces.includes('vscode');
}

function renderError(message: string): void {
  const app = document.getElementById('app');
  if (!app) {
    return;
  }
  app.innerHTML = '';
  const wrap = document.createElement('div');
  wrap.className = 'error';
  wrap.textContent = `Couldn't load settings: ${message}`;
  app.appendChild(wrap);
}

function buildHeader(): HTMLElement {
  const wrap = document.createElement('header');
  wrap.className = 'panel-header';
  const title = document.createElement('h1');
  title.textContent = 'Peridot Settings';
  const sub = document.createElement('p');
  sub.className = 'subtitle';
  sub.textContent = configPath
    ? `Editing ${configPath} — changes apply to new sessions started after Save.`
    : 'Changes apply to new sessions started after Save.';
  wrap.appendChild(title);
  wrap.appendChild(sub);
  return wrap;
}

function buildFooter(): HTMLElement {
  const wrap = document.createElement('footer');
  wrap.className = 'panel-footer';
  const save = document.createElement('button');
  save.className = 'primary';
  save.textContent = 'Save';
  save.addEventListener('click', () => {
    vscode.postMessage({ type: 'save', items: workingItems });
  });
  const reload = document.createElement('button');
  reload.className = 'secondary';
  reload.textContent = 'Reload from disk';
  reload.addEventListener('click', () => {
    vscode.postMessage({ type: 'reload' });
  });
  const flash = document.createElement('span');
  flash.id = 'flash';
  flash.className = 'flash';
  wrap.appendChild(save);
  wrap.appendChild(reload);
  wrap.appendChild(flash);
  return wrap;
}

function buildRow(item: SettingItem): HTMLElement {
  const row = document.createElement('div');
  row.className = 'row';
  row.dataset.id = item.id;

  const labelCell = document.createElement('div');
  labelCell.className = 'label-cell';
  const label = document.createElement('label');
  label.textContent = item.label;
  label.htmlFor = `field-${item.id}`;
  labelCell.appendChild(label);
  if (item.help) {
    const help = document.createElement('p');
    help.className = 'help';
    help.textContent = item.help;
    labelCell.appendChild(help);
  }

  const controlCell = document.createElement('div');
  controlCell.className = 'control-cell';
  controlCell.appendChild(buildControl(item));

  row.appendChild(labelCell);
  row.appendChild(controlCell);
  return row;
}

function buildControl(item: SettingItem): HTMLElement {
  const fieldId = `field-${item.id}`;
  switch (item.value.kind) {
    case 'Bool': {
      const wrap = document.createElement('label');
      wrap.className = 'toggle';
      const input = document.createElement('input');
      input.type = 'checkbox';
      input.id = fieldId;
      input.checked = item.value.data;
      input.addEventListener('change', () => {
        item.value = { kind: 'Bool', data: input.checked };
      });
      const slider = document.createElement('span');
      slider.className = 'slider';
      wrap.appendChild(input);
      wrap.appendChild(slider);
      return wrap;
    }
    case 'Choice': {
      const select = document.createElement('select');
      select.id = fieldId;
      const { options, selected } = item.value.data;
      options.forEach((opt, idx) => {
        const node = document.createElement('option');
        node.value = String(idx);
        node.textContent = opt;
        if (idx === selected) {
          node.selected = true;
        }
        select.appendChild(node);
      });
      select.addEventListener('change', () => {
        if (item.value.kind === 'Choice') {
          item.value = {
            kind: 'Choice',
            data: { options, selected: Number(select.value) },
          };
        }
      });
      return select;
    }
    case 'U32':
    case 'F64':
    case 'Usize':
      return buildNumberControl(item, fieldId);
  }
}

function buildNumberControl(item: SettingItem, fieldId: string): HTMLElement {
  if (item.value.kind === 'Bool' || item.value.kind === 'Choice') {
    // Exhaustiveness guard: only number variants reach here.
    return document.createElement('span');
  }
  const data = item.value.data;
  const input = document.createElement('input');
  input.type = 'number';
  input.id = fieldId;
  input.min = String(data.min);
  input.max = String(data.max);
  input.step = String(data.step);
  input.value = String(data.value);
  input.addEventListener('input', () => {
    const raw = Number(input.value);
    if (Number.isNaN(raw)) {
      return;
    }
    const clamped = Math.min(data.max, Math.max(data.min, raw));
    // Preserve the discriminator on the original variant.
    if (item.value.kind === 'U32') {
      item.value = { kind: 'U32', data: { ...data, value: clamped } };
    } else if (item.value.kind === 'F64') {
      item.value = { kind: 'F64', data: { ...data, value: clamped } };
    } else if (item.value.kind === 'Usize') {
      item.value = { kind: 'Usize', data: { ...data, value: clamped } };
    }
  });
  return input;
}

function showFlash(message: string, kind: 'ok' | 'err'): void {
  const flash = document.getElementById('flash');
  if (!flash) {
    return;
  }
  flash.textContent = message;
  flash.dataset.kind = kind;
  flash.classList.add('visible');
  window.setTimeout(() => {
    flash.classList.remove('visible');
  }, 3500);
}

function groupBy<T, K>(items: T[], keyer: (item: T) => K): Map<K, T[]> {
  const groups = new Map<K, T[]>();
  for (const item of items) {
    const key = keyer(item);
    const bucket = groups.get(key);
    if (bucket) {
      bucket.push(item);
    } else {
      groups.set(key, [item]);
    }
  }
  return groups;
}
