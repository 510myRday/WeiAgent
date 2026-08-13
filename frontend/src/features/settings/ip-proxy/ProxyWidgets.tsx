import type { ReactNode } from 'react';
import {
  ArrowDown,
  ArrowDownUp,
  ArrowUp,
  CheckCircle2,
  CircleDashed,
  LoaderCircle,
  WifiOff,
} from 'lucide-react';

import type {
  IpProxy,
  IpProxyQuality,
  IpProxyQualityProbe,
  IpProxyStatus,
} from '../../../types';
import { useI18n } from '../../../i18n';
import type { ProxySortKey, SortDirection } from '../ipProxyUtils';

export function SortHeader({
  label,
  sortKey,
  sortState,
  onSort,
}: {
  label: string;
  sortKey: ProxySortKey;
  sortState: { key: ProxySortKey; direction: SortDirection };
  onSort: (key: ProxySortKey) => void;
}) {
  const active = sortState.key === sortKey;
  return (
    <th aria-sort={active ? (sortState.direction === 'asc' ? 'ascending' : 'descending') : 'none'}>
      <button type="button" className={`ip-proxy-sort-button${active ? ' is-active' : ''}`} onClick={() => onSort(sortKey)}>
        <span>{label}</span>
        {active ? (sortState.direction === 'asc' ? <ArrowUp size={13} /> : <ArrowDown size={13} />) : <ArrowDownUp size={13} />}
      </button>
    </th>
  );
}

export function ProxyMetric({
  icon,
  label,
  value,
  tone = 'default',
}: {
  icon: ReactNode;
  label: string;
  value: string | number;
  tone?: 'default' | 'info' | 'success' | 'warning';
}) {
  return (
    <div className={`ip-proxy-metric is-${tone}`}>
      <span className="ip-proxy-metric-icon">{icon}</span>
      <span className="ip-proxy-metric-copy">
        <span>{label}</span>
        <strong>{value}</strong>
      </span>
    </div>
  );
}

export function ProxyLocation({ proxy, emptyLabel }: { proxy: IpProxy; emptyLabel: string }) {
  const seen = new Set<string>();
  const parts = [proxy.country, proxy.region, proxy.city].filter((value): value is string => {
    const normalized = value?.trim().toLocaleLowerCase();
    if (!normalized || seen.has(normalized)) return false;
    seen.add(normalized);
    return true;
  });
  const location = parts.join(' · ') || proxy.country_code || emptyLabel;
  return (
    <span className={`ip-proxy-location${parts.length === 0 && !proxy.country_code ? ' is-empty' : ''}`}>
      <span title={location}>{location}</span>
      {proxy.ip_address ? <code>{proxy.ip_address}</code> : null}
    </span>
  );
}

export function ProxyStatusBadge({
  proxy,
  isTesting,
  label,
  testingLabel,
}: {
  proxy: IpProxy;
  isTesting: boolean;
  label: string;
  testingLabel: string;
}) {
  const icon = isTesting
    ? <LoaderCircle size={13} className="is-spinning" />
    : proxy.status === 'available'
      ? <CheckCircle2 size={13} />
      : proxy.status === 'unavailable'
        ? <WifiOff size={13} />
        : <CircleDashed size={13} />;
  return (
    <span
      className={`ip-proxy-status is-${isTesting ? 'testing' : proxy.status}`}
      title={proxy.last_error ?? undefined}
    >
      {icon}
      <span>{isTesting ? testingLabel : label}</span>
    </span>
  );
}

export function LatencyValue({ latency, status }: { latency: number | null; status: IpProxyStatus }) {
  if (status !== 'available' || latency == null) return <span className="ip-proxy-latency is-empty">--</span>;
  const tone = latency <= 500 ? 'fast' : latency <= 1500 ? 'medium' : 'slow';
  return <span className={`ip-proxy-latency is-${tone}`}>{latency} ms</span>;
}

type QualityApiKey = keyof IpProxyQuality;

const QUALITY_API_ITEMS: Array<{ key: QualityApiKey; short: string; label: string }> = [
  { key: 'google', short: 'G', label: 'Google' },
  { key: 'openai', short: 'O', label: 'OpenAI' },
  { key: 'anthropic', short: 'A', label: 'Anthropic' },
  { key: 'grok', short: 'X', label: 'Grok' },
];

export function ApiQualityGrid({
  quality,
  isTesting,
}: {
  quality: IpProxyQuality;
  isTesting: boolean;
}) {
  return (
    <div className="ip-proxy-quality-grid" aria-label="API quality">
      {QUALITY_API_ITEMS.map((item) => {
        const probe: IpProxyQualityProbe = quality?.[item.key] ?? {
          status: 'unchecked',
          latency_ms: null,
          error: null,
        };
        const state = isTesting ? 'testing' : probe.status;
        const value = isTesting ? '...' : probe.status === 'available' && probe.latency_ms != null
          ? `${probe.latency_ms} ms`
          : probe.status === 'error' ? 'ERROR' : '--';
        return (
          <span
            key={item.key}
            className={`ip-proxy-quality-cell is-${item.key} is-${state}`}
            title={probe.error ?? item.label}
          >
            <strong>{item.short}</strong>
            <span>{value}</span>
          </span>
        );
      })}
    </div>
  );
}

export function ApiQualityLegend() {
  const { t } = useI18n();
  return (
    <div className="ip-proxy-quality-legend" aria-label={t('ipProxy.apiQuality')}>
      <span className="ip-proxy-quality-legend-label">{t('ipProxy.qualityLegendLabel')}</span>
      {QUALITY_API_ITEMS.map((item) => (
        <span key={item.key} className={`ip-proxy-quality-legend-item is-${item.key}`}>
          <i aria-hidden="true" />
          {item.label}
        </span>
      ))}
      <span className="ip-proxy-quality-legend-item is-error">
        <i aria-hidden="true" />
        ERROR
      </span>
    </div>
  );
}



