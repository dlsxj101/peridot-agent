export interface RiskChipView {
  className: string;
  label: string;
  title: string;
}

const KNOWN_RISK_CLASSES = new Set([
  'read_only',
  'local_write',
  'build_or_test',
  'external_network',
  'destructive',
  'secret_adjacent',
]);

export function riskChipView(riskClass: string | undefined): RiskChipView | undefined {
  const normalized = riskClass?.trim();
  if (!normalized) return undefined;
  const tone = KNOWN_RISK_CLASSES.has(normalized) ? normalized : 'unknown';
  return {
    className: `risk-chip risk-chip-${tone}`,
    label: riskChipLabel(normalized),
    title: `Risk class: ${normalized.replace(/_/g, ' ')}`,
  };
}

function riskChipLabel(riskClass: string): string {
  switch (riskClass) {
    case 'read_only':
      return 'R';
    case 'local_write':
      return 'W';
    case 'build_or_test':
      return 'B';
    case 'external_network':
      return 'N';
    case 'destructive':
      return '!';
    case 'secret_adjacent':
      return 'S';
    default:
      return '?';
  }
}
