export interface BranchSnapshotChoice {
  name: string;
  label: string;
}

export function branchSnapshotChoices(snapshots: string[]): BranchSnapshotChoice[] {
  const seen = new Set<string>();
  const choices: BranchSnapshotChoice[] = [];
  for (const snapshot of snapshots) {
    const name = snapshot.trim();
    if (!name || seen.has(name)) continue;
    seen.add(name);
    choices.push({ name, label: name });
  }
  return choices.sort((a, b) => a.label.localeCompare(b.label));
}

export function branchPickerSlashCommand(): string {
  return '/branch';
}

export function branchListSlashCommand(): string {
  return '/branch list';
}

export function branchTreeSlashCommand(): string {
  return '/branch tree';
}

export function branchSaveSlashCommand(name: string): string {
  return `/branch save ${branchNameArg(name)}`;
}

export function branchRestoreSlashCommand(name: string): string {
  return `/branch restore ${branchNameArg(name)}`;
}

export function branchTurnSlashCommand(turnId: number): string {
  return `/branch turn ${positiveIntegerArg(turnId, 'Turn id')}`;
}

export function branchSwitchSlashCommand(index: number): string {
  return `/branch switch ${positiveIntegerArg(index, 'Branch limb index')}`;
}

export function parseBranchTurnInput(value: string): number {
  return parsePositiveIntegerInput(value, 'Enter a positive turn id.');
}

export function parseBranchSwitchInput(value: string): number {
  return parsePositiveIntegerInput(value, 'Enter a positive branch limb index.');
}

function branchNameArg(value: string): string {
  const name = value.trim();
  if (!name) {
    throw new Error('Branch snapshot name is required.');
  }
  if (!/^[A-Za-z0-9_-]+$/.test(name)) {
    throw new Error('Branch snapshot name may only contain ASCII letters, digits, "-", and "_".');
  }
  return name;
}

function positiveIntegerArg(value: number, label: string): number {
  if (!Number.isInteger(value) || value <= 0) {
    throw new Error(`${label} must be a positive integer.`);
  }
  return value;
}

function parsePositiveIntegerInput(value: string, errorMessage: string): number {
  const trimmed = value.trim();
  if (!/^[1-9]\d*$/.test(trimmed)) {
    throw new Error(errorMessage);
  }
  return Number(trimmed);
}
