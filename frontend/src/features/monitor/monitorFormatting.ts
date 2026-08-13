type Translate = (path: string, params?: Record<string, string | number>) => string;

export function formatCompactNumber(value: number) {
  return value.toLocaleString();
}

export function formatTokenCompact(value: number) {
  const abs = Math.abs(value);
  if (abs >= 1_000_000_000) {
    const scaled = value / 1_000_000_000;
    return `${scaled >= 100 ? scaled.toFixed(0) : scaled.toFixed(1).replace(/\.0$/, '')}B`;
  }
  if (abs >= 1_000_000) {
    const scaled = value / 1_000_000;
    return `${scaled >= 100 ? scaled.toFixed(0) : scaled.toFixed(1).replace(/\.0$/, '')}M`;
  }
  if (abs >= 1_000) {
    const scaled = value / 1_000;
    return `${scaled >= 100 ? scaled.toFixed(0) : scaled.toFixed(1).replace(/\.0$/, '')}K`;
  }
  return value.toLocaleString();
}

export function formatSuccessRate(success: number, total: number) {
  if (total <= 0) return '—';
  const rate = (success / total) * 100;
  return `${rate.toFixed(1)}%`;
}

export function formatTokenCount(value: number | undefined, t: Translate) {
  if (value == null) return t('llmLogs.unknown');
  return value.toLocaleString();
}

export function formatCostValue(value: number | undefined, t: Translate) {
  if (value == null) return t('llmLogs.unknown');
  if (value === 0) return t('llmLogs.usd', { value: '0' });
  // Round to at most 5 decimal places (table rows + summary).
  const fixed = value.toFixed(5).replace(/\.?0+$/, '');
  return t('llmLogs.usd', { value: fixed || '0' });
}

export function formatSharePercent(part: number, total: number, t: Translate) {
  const percent = total <= 0 ? '0.0%' : `${((part / total) * 100).toFixed(1)}%`;
  return t('llmLogs.metricShareOfTotal', { percent });
}

export function formatSecondsValue(ms: number | undefined, t: Translate) {
  if (ms == null) return t('llmLogs.unknown');
  const seconds = ms / 1000;
  if (seconds < 10) return `${seconds.toFixed(2)}s`;
  if (seconds < 60) return `${seconds.toFixed(1)}s`;
  return `${seconds.toFixed(0)}s`;
}

export function formatDateParts(value: string) {
  const date = new Date(value);
  return {
    date: date.toLocaleDateString(),
    time: date.toLocaleTimeString(),
  };
}
