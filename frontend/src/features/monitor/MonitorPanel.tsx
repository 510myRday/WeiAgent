import { useCallback, useEffect, useMemo, useState } from 'react';
import {
  ArrowDownToLine,
  ArrowLeft,
  ArrowUpFromLine,
  ChartColumn,
  CheckCircle2,
  CircleX,
  Database,
  ChevronDown,
  ChevronLeft,
  ChevronRight,
  Copy,
  FileText,
  Hash,
  RefreshCw,
  ShieldCheck,
  Sparkles,
} from 'lucide-react';
import type {
  Agent,
  AppSettings,
  AvatarIcon,
  LlmLogAgentStat,
  LlmLogDetail,
  LlmLogEntry,
  LlmLogModelStat,
  LlmLogSummary,
  ModelConfig,
  ModelProvider,
} from '../../types';
import {
  getLlmLog,
  listLlmLogs,
} from '../../api';
import { useI18n } from '../../i18n';
import { useClipboard } from '../../hooks';
import {
  Button,
  Dialog,
  IconButton,
  ListItem,
  PaneHeader,
  useFeedback,
} from '../../components/ui';
import { PanelShell } from '../../components/layout';
import { AvatarBadge } from '../../components/AvatarPicker';
import { formatLlmResponse, formatRawJson } from '../settings/llmLogFormatting';
import { agentDisplayName } from '../../components/AgentListItem';

interface MonitorPanelProps {
  agents: Agent[];
  providers: ModelProvider[];
  settings: AppSettings;
  onSettingsSave: (settings: AppSettings) => Promise<AppSettings>;
  onOpenMain: () => void;
  onBackToList: () => void;
}

type Scope =
  | { kind: 'overview' }
  | { kind: 'agent'; agentKey: string; agentId: string; agentName: string }
  | { kind: 'model'; modelKey: string; providerName: string; modelId: string };

type AgentOption = {
  agentKey: string;
  agentId: string;
  agentName: string;
  requestCount: number;
  latestAt: string;
  avatarIcon: AvatarIcon;
  avatarColor: string;
  agent?: Agent;
};

type ModelOption = {
  modelKey: string;
  providerName: string;
  modelId: string;
  requestCount: number;
  latestAt: string;
};

type Translate = (path: string, params?: Record<string, string | number>) => string;

const PAGE_SIZE_OPTIONS = [10, 20, 50] as const;
type PageSize = (typeof PAGE_SIZE_OPTIONS)[number];

type DateRangeKey = 'today' | '7d' | '30d';
type StatusFilter = 'all' | 'success' | 'error';

const DATE_RANGE_OPTIONS: DateRangeKey[] = ['today', '7d', '30d'];

const EMPTY_SUMMARY: LlmLogSummary = {
  request_count: 0,
  success_count: 0,
  error_count: 0,
  input_tokens: 0,
  output_tokens: 0,
  cache_read_tokens: 0,
  cache_write_tokens: 0,
  cost_usd: 0,
};

function startOfLocalDay(date: Date) {
  const next = new Date(date);
  next.setHours(0, 0, 0, 0);
  return next;
}

function addLocalDays(date: Date, days: number) {
  const next = new Date(date);
  next.setDate(next.getDate() + days);
  return next;
}

function rangeBounds(range: DateRangeKey): { since: string; until: string } {
  const todayStart = startOfLocalDay(new Date());
  const until = addLocalDays(todayStart, 1);
  if (range === 'today') {
    return { since: todayStart.toISOString(), until: until.toISOString() };
  }
  if (range === '7d') {
    return { since: addLocalDays(todayStart, -6).toISOString(), until: until.toISOString() };
  }
  return { since: addLocalDays(todayStart, -29).toISOString(), until: until.toISOString() };
}

function formatCompactNumber(value: number) {
  return value.toLocaleString();
}

function formatTokenCompact(value: number) {
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

function formatSuccessRate(success: number, total: number) {
  if (total <= 0) return '—';
  const rate = (success / total) * 100;
  return `${rate.toFixed(1)}%`;
}

function TokenCompact({
  value,
  className,
}: {
  value: number;
  className?: string;
}) {
  return (
    <span className={className} title={formatCompactNumber(value)}>
      {formatTokenCompact(value)}
    </span>
  );
}


function buildPageItems(current: number, totalPages: number): Array<number | 'ellipsis'> {
  if (totalPages <= 7) {
    return Array.from({ length: totalPages }, (_, index) => index + 1);
  }
  const pages = new Set<number>([1, totalPages, current]);
  for (let page = current - 1; page <= current + 1; page += 1) {
    if (page >= 1 && page <= totalPages) pages.add(page);
  }
  if (current <= 3) {
    pages.add(2);
    pages.add(3);
    pages.add(4);
  }
  if (current >= totalPages - 2) {
    pages.add(totalPages - 1);
    pages.add(totalPages - 2);
    pages.add(totalPages - 3);
  }
  const sorted = [...pages].sort((a, b) => a - b);
  const items: Array<number | 'ellipsis'> = [];
  let previous = 0;
  for (const page of sorted) {
    if (previous && page - previous > 1) items.push('ellipsis');
    items.push(page);
    previous = page;
  }
  return items;
}


function agentScopeKey(log: Pick<LlmLogEntry, 'agent_id' | 'agent_name'>) {
  if (log.agent_id) return `id:${log.agent_id}`;
  return `name:${log.agent_name || 'unknown'}`;
}

function resolveAgent(
  log: Pick<LlmLogEntry, 'agent_id' | 'agent_name'>,
  agentById: Map<string, Agent>,
  agentByName: Map<string, Agent>,
): Agent | undefined {
  if (log.agent_id) {
    const byId = agentById.get(log.agent_id);
    if (byId) return byId;
  }
  if (log.agent_name) return agentByName.get(log.agent_name);
  return undefined;
}

function resolveAgentName(
  log: Pick<LlmLogEntry, 'agent_id' | 'agent_name'>,
  agentById: Map<string, Agent>,
  agentByName: Map<string, Agent>,
  t: Translate,
): string {
  const agent = resolveAgent(log, agentById, agentByName);
  if (agent) return agentDisplayName(agent, t);
  return log.agent_name || t('llmLogs.unknownAgent');
}

function findModelConfig(
  providers: ModelProvider[],
  providerName: string,
  modelId: string,
): ModelConfig | undefined {
  const provider = providers.find((item) => item.name === providerName)
    ?? providers.find((item) => item.model_configs.some((config) => config.model_id === modelId)
      || item.models.includes(modelId));
  if (!provider) return undefined;
  return provider.model_configs.find((config) => config.model_id === modelId);
}

/** Unit prices are USD / 1M tokens. Missing prices count as 0. */
function calculateCostUsd(
  log: Pick<LlmLogEntry, 'provider_name' | 'model_id' | 'input_tokens' | 'output_tokens' | 'cache_write_tokens' | 'cache_read_tokens' | 'cost_usd'>,
  providers: ModelProvider[],
): number {
  const config = findModelConfig(providers, log.provider_name, log.model_id);
  if (!config) {
    return log.cost_usd ?? 0;
  }
  const input = log.input_tokens ?? 0;
  const output = log.output_tokens ?? 0;
  const cacheWrite = log.cache_write_tokens ?? 0;
  const cacheRead = log.cache_read_tokens ?? 0;
  return (
    input * (config.input_price ?? 0)
    + output * (config.output_price ?? 0)
    + cacheWrite * (config.cache_write_price ?? 0)
    + cacheRead * (config.cache_read_price ?? 0)
  ) / 1_000_000;
}

function formatTokenCount(value: number | undefined, t: Translate) {
  if (value == null) return t('llmLogs.unknown');
  return value.toLocaleString();
}

function formatCostValue(value: number | undefined, t: Translate) {
  if (value == null) return t('llmLogs.unknown');
  if (value === 0) return t('llmLogs.usd', { value: '0' });
  // Round to at most 5 decimal places (table rows + summary).
  const fixed = value.toFixed(5).replace(/\.?0+$/, '');
  return t('llmLogs.usd', { value: fixed || '0' });
}

function formatSharePercent(part: number, total: number, t: Translate) {
  const percent = total <= 0 ? '0.0%' : `${((part / total) * 100).toFixed(1)}%`;
  return t('llmLogs.metricShareOfTotal', { percent });
}

function formatSecondsValue(ms: number | undefined, t: Translate) {
  if (ms == null) return t('llmLogs.unknown');
  const seconds = ms / 1000;
  if (seconds < 10) return `${seconds.toFixed(2)}s`;
  if (seconds < 60) return `${seconds.toFixed(1)}s`;
  return `${seconds.toFixed(0)}s`;
}

/** Latency color tiers from green (fast) to deep red (very slow). */
type LatencyTone =
  | 'under5'
  | 's5_10'
  | 's10_15'
  | 's15_20'
  | 's20_25'
  | 's25_30'
  | 's30_60'
  | 'm1_5'
  | 'm5_10'
  | 'over10'
  | 'unknown';

function latencyTone(ms: number | undefined): LatencyTone {
  if (ms == null) return 'unknown';
  if (ms < 5_000) return 'under5';
  if (ms < 10_000) return 's5_10';
  if (ms < 15_000) return 's10_15';
  if (ms < 20_000) return 's15_20';
  if (ms < 25_000) return 's20_25';
  if (ms < 30_000) return 's25_30';
  if (ms < 60_000) return 's30_60';
  if (ms < 5 * 60_000) return 'm1_5';
  if (ms < 10 * 60_000) return 'm5_10';
  return 'over10';
}

function LatencyBadge({
  ms,
  t,
}: {
  ms: number | undefined;
  t: Translate;
}) {
  const tone = latencyTone(ms);
  return (
    <span className={`monitor-latency-badge is-${tone}`}>
      {formatSecondsValue(ms, t)}
    </span>
  );
}

function formatDateParts(value: string) {
  const date = new Date(value);
  return {
    date: date.toLocaleDateString(),
    time: date.toLocaleTimeString(),
  };
}

function SplitValue({
  left,
  right,
  leftMono = false,
  rightMono = true,
}: {
  left: string;
  right: string;
  leftMono?: boolean;
  rightMono?: boolean;
}) {
  return (
    <span className="monitor-split-cell">
      <span className={leftMono ? 'mono' : undefined}>{left}</span>
      <span className="monitor-split-sep" aria-hidden="true">|</span>
      <span className={rightMono ? 'mono' : undefined}>{right}</span>
    </span>
  );
}


function isFailedRequest(status?: number, error?: string | null) {
  if (status != null && status >= 400) return true;
  if (error && error.trim()) return true;
  return false;
}

function StatusBadge({
  status,
  unknownLabel,
}: {
  status?: number;
  unknownLabel: string;
}) {
  const tone = status == null
    ? 'unknown'
    : status >= 400
      ? 'error'
      : status >= 300
        ? 'warning'
        : status >= 200
          ? 'success'
          : 'warning';
  return (
    <span className={`monitor-status is-${tone}`}>
      <span className="monitor-status-dot" aria-hidden="true" />
      <span>{status ?? unknownLabel}</span>
    </span>
  );
}

export function MonitorPanel({
  agents,
  providers,
  settings,
  onSettingsSave,
  onOpenMain,
  onBackToList,
}: MonitorPanelProps) {
  const { t } = useI18n();
  const { toast } = useFeedback();
  const { copied, copy } = useClipboard(2000);

  const [logs, setLogs] = useState<LlmLogEntry[]>([]);
  const [agentStats, setAgentStats] = useState<LlmLogAgentStat[]>([]);
  const [modelStats, setModelStats] = useState<LlmLogModelStat[]>([]);
  const [total, setTotal] = useState(0);
  const [summary, setSummary] = useState<LlmLogSummary>(EMPTY_SUMMARY);
  const [statusSummary, setStatusSummary] = useState<LlmLogSummary>(EMPTY_SUMMARY);
  const [scope, setScope] = useState<Scope>({ kind: 'overview' });
  const [statusFilter, setStatusFilter] = useState<StatusFilter>('all');
  const [dateRange, setDateRange] = useState<DateRangeKey>('today');
  const [isLoading, setIsLoading] = useState(true);
  const [page, setPage] = useState(1);
  const [pageSize, setPageSize] = useState<PageSize>(10);
  const [selectedLog, setSelectedLog] = useState<LlmLogDetail | null>(null);
  const [activeTab, setActiveTab] = useState<'request' | 'response'>('request');
  const [isDetailLoading, setIsDetailLoading] = useState(false);
  const [pendingDetailNavigation, setPendingDetailNavigation] = useState<{ direction: -1 | 1; page: number } | null>(null);
  const [isSavingToggle, setIsSavingToggle] = useState(false);

  const agentById = useMemo(() => {
    const map = new Map<string, Agent>();
    for (const agent of agents) map.set(agent.id, agent);
    return map;
  }, [agents]);

  const agentByName = useMemo(() => {
    const map = new Map<string, Agent>();
    for (const agent of agents) map.set(agent.name, agent);
    return map;
  }, [agents]);

  const loadData = useCallback(async () => {
    setIsLoading(true);
    try {
      const offset = (page - 1) * pageSize;
      const { since, until } = rangeBounds(dateRange);
      const filter = {
        agentId: scope.kind === 'agent' ? (scope.agentId || undefined) : undefined,
        agentName: scope.kind === 'agent' ? scope.agentName : undefined,
        providerName: scope.kind === 'model' ? scope.providerName : undefined,
        modelId: scope.kind === 'model' ? scope.modelId : undefined,
        since,
        until,
        status: statusFilter === 'all' ? undefined : statusFilter,
      };
      const pageData = await listLlmLogs({
        offset,
        limit: pageSize,
        ...filter,
      });
      setLogs(pageData.items);
      setTotal(pageData.total);
      setAgentStats(pageData.agent_stats ?? []);
      setModelStats(pageData.model_stats ?? []);

      // DB cost_usd is often 0 (API rarely returns usage.cost). Match table pricing:
      // estimate from model unit prices for all successful rows in range.
      const baseSummary = pageData.summary ?? EMPTY_SUMMARY;
      const unfilteredPage = statusFilter === 'all'
        ? pageData
        : await listLlmLogs({ offset: 0, limit: 1, ...filter, status: undefined });
      setStatusSummary(unfilteredPage.summary ?? baseSummary);
      let costRows = pageData.items;
      if (pageData.total > pageData.items.length) {
        const costPage = await listLlmLogs({
          offset: 0,
          limit: Math.min(Math.max(pageData.total, 1), 1000),
          ...filter,
        });
        costRows = costPage.items;
      }
      const estimatedCost = costRows.reduce((sum, log) => {
        if (isFailedRequest(log.raw_status, log.error)) return sum;
        return sum + calculateCostUsd(log, providers);
      }, 0);
      setSummary({ ...baseSummary, cost_usd: estimatedCost });

      const maxPage = Math.max(1, Math.ceil(pageData.total / pageSize) || 1);
      if (page > maxPage) setPage(maxPage);
    } catch (cause) {
      toast.error(cause instanceof Error ? cause.message : t('error.unknown'));
    } finally {
      setIsLoading(false);
    }
  }, [dateRange, page, pageSize, providers, scope, statusFilter, t, toast]);

  useEffect(() => {
    void loadData();
  }, [loadData]);

  useEffect(() => {
    if (!pendingDetailNavigation || !selectedLog || isLoading || page !== pendingDetailNavigation.page) return;
    const targetIndex = pendingDetailNavigation.direction === -1 ? logs.length - 1 : 0;
    const target = logs[targetIndex];
    if (!target) return;
    setPendingDetailNavigation(null);
    void handleViewPayload(target.id, Boolean(target.has_payload));
  }, [handleViewPayload, isLoading, logs, page, pendingDetailNavigation, selectedLog]);

  const agentOptions = useMemo(() => {
    return agentStats
      .map((stat) => {
        const agent = (stat.agent_id ? agentById.get(stat.agent_id) : undefined)
          ?? agentByName.get(stat.agent_name);
        const agentName = agent
          ? agentDisplayName(agent, t)
          : (stat.agent_name || t('llmLogs.unknownAgent'));
        return {
          agentKey: stat.agent_key,
          agentId: agent?.id || stat.agent_id || '',
          agentName,
          requestCount: stat.request_count,
          latestAt: stat.latest_at,
          avatarIcon: agent?.avatar_icon ?? 'bot',
          avatarColor: agent?.avatar_color ?? '#64748b',
          agent,
        } satisfies AgentOption;
      })
      .sort((a, b) => b.latestAt.localeCompare(a.latestAt));
  }, [agentById, agentByName, agentStats, t]);

  const modelOptions = useMemo(() => {
    return modelStats
      .map((stat) => ({
        modelKey: stat.model_key,
        providerName: stat.provider_name,
        modelId: stat.model_id,
        requestCount: stat.request_count,
        latestAt: stat.latest_at,
      } satisfies ModelOption))
      .sort((a, b) => b.latestAt.localeCompare(a.latestAt));
  }, [modelStats]);

  const totalPages = Math.max(1, Math.ceil(total / pageSize) || 1);
  const rangeStart = total === 0 ? 0 : (page - 1) * pageSize + 1;
  const rangeEnd = total === 0 ? 0 : Math.min(page * pageSize, total);
  const pageItems = useMemo(() => buildPageItems(page, totalPages), [page, totalPages]);

  const formattedResponse = useMemo(() => (
    selectedLog?.raw_response ? formatLlmResponse(selectedLog.raw_response) : null
  ), [selectedLog?.raw_response]);

  async function handleToggleLogging(enabled: boolean) {
    setIsSavingToggle(true);
    try {
      await onSettingsSave({ ...settings, llm_log_enabled: enabled });
    } catch (cause) {
      toast.error(cause instanceof Error ? cause.message : t('error.unknown'));
    } finally {
      setIsSavingToggle(false);
    }
  }

  async function handleViewPayload(logId: string, hasPayload: boolean) {
    if (!hasPayload) return;
    setIsDetailLoading(true);
    try {
      const detail = await getLlmLog(logId);
      if (!detail.raw_request && !detail.raw_response && !detail.error) {
        toast.error(t('llmLogs.payloadUnavailable'));
        return;
      }
      setSelectedLog(detail);
      setActiveTab('request');
    } catch (cause) {
      toast.error(cause instanceof Error ? cause.message : t('error.unknown'));
    } finally {
      setIsDetailLoading(false);
    }
  }

  const selectedLogIndex = selectedLog
    ? logs.findIndex((log) => log.id === selectedLog.id)
    : -1;
  const canGoPrevious = selectedLogIndex > 0 || (selectedLogIndex === 0 && page > 1);
  const canGoNext = selectedLogIndex >= 0 && (
    selectedLogIndex < logs.length - 1 || page < totalPages
  );

  function handleNavigateDetail(direction: -1 | 1) {
    if (selectedLogIndex < 0) return;
    const target = logs[selectedLogIndex + direction];
    if (target) {
      void handleViewPayload(target.id, Boolean(target.has_payload));
      return;
    }
    const targetPage = page + direction;
    if (targetPage < 1 || targetPage > totalPages) return;
    setIsLoading(true);
    setPendingDetailNavigation({ direction, page: targetPage });
    setPage(targetPage);
  }

  function handleCopy() {
    const text = activeTab === 'request'
      ? formatRawJson(selectedLog?.raw_request)
      : (selectedLog?.raw_response
        ? formatRawJson(selectedLog.raw_response)
        : selectedLog?.error || '');
    if (!text) return;
    void copy(text);
  }

  function selectOverview() {
    setScope({ kind: 'overview' });
    setStatusFilter('all');
    setPage(1);
    onOpenMain();
  }

  function selectStatus(nextStatus: Exclude<StatusFilter, 'all'>) {
    setStatusFilter(nextStatus);
    setPage(1);
    onOpenMain();
  }

  function selectAgent(option: AgentOption) {
    setScope({
      kind: 'agent',
      agentKey: option.agentKey,
      agentId: option.agentId,
      agentName: option.agentName,
    });
    setPage(1);
    onOpenMain();
  }

  function selectModel(option: ModelOption) {
    setScope({
      kind: 'model',
      modelKey: option.modelKey,
      providerName: option.providerName,
      modelId: option.modelId,
    });
    setPage(1);
    onOpenMain();
  }

  const baseTitle = scope.kind === 'overview'
    ? t('llmLogs.allRequests')
    : scope.kind === 'agent'
      ? scope.agentName
      : <span className="mono">{scope.modelId}</span>;

  const statusTitle = statusFilter === 'success'
    ? t('llmLogs.successRequests')
    : statusFilter === 'error'
      ? t('llmLogs.errorRequests')
      : null;

  const mainTitle = (
    <span className="monitor-page-title">
      <span>{baseTitle}</span>
      {statusTitle ? (
        <span className={`monitor-status-scope-badge is-${statusFilter}`}>
          {statusFilter === 'success' ? <CheckCircle2 size={15} /> : <CircleX size={15} />}
          {statusTitle}
        </span>
      ) : null}
    </span>
  );

  const mainDescription = scope.kind === 'overview'
    ? t('llmLogs.overviewDesc')
    : scope.kind === 'agent'
      ? t('llmLogs.agentRequests')
      : (
        <span className="monitor-scope-model-meta">
          <span>{t('llmLogs.modelRequests')}</span>
          <span aria-hidden="true">·</span>
          <span>
            {t('llmLogs.provider')}：<strong>{scope.providerName}</strong>
          </span>
        </span>
      );

  return (
    <>
      <PanelShell
        className="monitor-panel"
        sidebar={(
          <div className="context-pane monitor-list-pane" aria-label={t('llmLogs.navTitle')}>
        <PaneHeader
          className="mobile-pane-header"
          title={t('llmLogs.navTitle')}
        />
        <div className="list-stack monitor-scope-list">
          <ListItem
            type="button"
            active={scope.kind === 'overview'}
            title={t('llmLogs.overview')}
            subtitle={t('llmLogs.requestCount', {
              count: agentStats.reduce((sum, item) => sum + item.request_count, 0),
            })}
            leading={<ChartColumn size={18} />}
            onClick={selectOverview}
          />

          <div className="monitor-section-label">{t('llmLogs.byStatus')}</div>
          <ListItem
            className="monitor-status-scope-item is-success"
            type="button"
            active={statusFilter === 'success'}
            title={t('llmLogs.successRequests')}
            subtitle={t('llmLogs.requestCount', { count: statusSummary.success_count })}
            leading={<CheckCircle2 size={18} />}
            onClick={() => selectStatus('success')}
          />
          <ListItem
            className="monitor-status-scope-item is-error"
            type="button"
            active={statusFilter === 'error'}
            title={t('llmLogs.errorRequests')}
            subtitle={t('llmLogs.requestCount', { count: statusSummary.error_count })}
            leading={<CircleX size={18} />}
            onClick={() => selectStatus('error')}
          />

          <div className="monitor-section-label">{t('llmLogs.byAgent')}</div>
          {agentOptions.length === 0 ? (
            <p className="muted centered monitor-empty-hint">{t('llmLogs.agentEmpty')}</p>
          ) : (
            agentOptions.map((option) => (
              <ListItem
                key={option.agentKey}
                type="button"
                active={scope.kind === 'agent' && scope.agentKey === option.agentKey}
                title={(
                  <span className="agent-list-title">
                    <span>{option.agentName}</span>
                    {option.agent?.is_builtin ? (
                      <span className="mini-status-badge">
                        <ShieldCheck size={12} />
                        {t('agentConfig.builtinBadge')}
                      </span>
                    ) : null}
                  </span>
                )}
                subtitle={t('llmLogs.requestCount', { count: option.requestCount })}
                leading={(
                  <AvatarBadge
                    icon={option.avatarIcon}
                    color={option.avatarColor}
                    size={16}
                  />
                )}
                onClick={() => selectAgent(option)}
              />
            ))
          )}

          <div className="monitor-section-label">{t('llmLogs.byModel')}</div>
          {modelOptions.length === 0 ? (
            <p className="muted centered monitor-empty-hint">{t('llmLogs.modelEmpty')}</p>
          ) : (
            <div className="monitor-model-scope-list" role="list">
              {modelOptions.map((option) => {
                const active = scope.kind === 'model' && scope.modelKey === option.modelKey;
                return (
                  <button
                    key={option.modelKey}
                    type="button"
                    role="listitem"
                    className={`monitor-model-scope-card${active ? ' is-active' : ''}`}
                    title={`${option.providerName} · ${option.modelId}`}
                    aria-pressed={active}
                    onClick={() => selectModel(option)}
                  >
                    <span className="monitor-model-scope-card-top">
                      <span className="monitor-model-scope-provider">{option.providerName}</span>
                      <span className="monitor-model-scope-count">
                        {t('llmLogs.requestCount', { count: option.requestCount })}
                      </span>
                    </span>
                    <span className="monitor-model-scope-model mono">{option.modelId}</span>
                  </button>
                );
              })}
            </div>
          )}
        </div>
          </div>
        )}
        main={(
          <div className="main-pane config-main monitor-main" aria-label={t('llmLogs.navTitle')}>
        <PaneHeader
          className="config-page-header"
          leading={(
            <Button
              className="main-back-button"
              variant="ghost"
              type="button"
              onClick={onBackToList}
              icon={<ArrowLeft size={17} />}
            >
              {t('common.back')}
            </Button>
          )}
          eyebrow={t('llmLogs.navTitle')}
          title={mainTitle}
          description={mainDescription}
          actions={(
            <span className="model-note-badge monitor-header-hint" title={t('llmLogs.dashboardHint')}>
              {t('llmLogs.dashboardHint')}
            </span>
          )}
        />

        <section className="monitor-dashboard form-card" aria-label={t('llmLogs.metrics')}>
          <div className="monitor-metric-grid">
            <article className="monitor-metric-card is-controls">
              <div className="monitor-metric-head">
                <span className="monitor-metric-label">{t('llmLogs.rangeLabel')}</span>
              </div>
              <div className="monitor-range-group" role="group" aria-label={t('llmLogs.rangeLabel')}>
                {DATE_RANGE_OPTIONS.map((key) => (
                  <button
                    key={key}
                    type="button"
                    className={`monitor-range-btn${dateRange === key ? ' is-active' : ''}`}
                    onClick={() => {
                      setDateRange(key);
                      setPage(1);
                    }}
                  >
                    {t(`llmLogs.range${key === 'today' ? 'Today' : key === '7d' ? '7d' : '30d'}`)}
                  </button>
                ))}
              </div>
              <div className="monitor-controls-footer">
                <label
                  className={`monitor-payload-switch${settings.llm_log_enabled ? ' is-on' : ''}`}
                  title={t('llmLogs.enableLoggingHint')}
                >
                  <input
                    type="checkbox"
                    checked={settings.llm_log_enabled}
                    disabled={isSavingToggle}
                    onChange={(event) => void handleToggleLogging(event.target.checked)}
                  />
                  <span className="monitor-payload-track" aria-hidden="true">
                    <span className="monitor-payload-thumb" />
                  </span>
                  <span className="monitor-payload-copy">
                    <span className="monitor-payload-title">{t('llmLogs.enableLogging')}</span>
                    <span className="monitor-payload-state">
                      {settings.llm_log_enabled ? t('llmLogs.savePayloadOn') : t('llmLogs.savePayloadOff')}
                    </span>
                  </span>
                </label>
                <Button
                  className="monitor-refresh-btn"
                  variant="secondary"
                  type="button"
                  onClick={() => void loadData()}
                  icon={<RefreshCw size={14} />}
                >
                  {t('llmLogs.refresh')}
                </Button>
              </div>
            </article>

            <article className="monitor-metric-card is-requests">
              <div className="monitor-metric-head">
                <span className="monitor-metric-icon" aria-hidden="true"><Hash size={14} /></span>
                <span className="monitor-metric-label">{t('llmLogs.metricRequests')}</span>
              </div>
              <strong className="monitor-metric-value mono">{formatCompactNumber(summary.request_count)}</strong>
              <span className="monitor-metric-sub">
                <span className="monitor-metric-sub-strong is-success">
                  {t('llmLogs.metricSuccess')} {formatCompactNumber(summary.success_count)}
                </span>
                <span className="monitor-metric-dot" aria-hidden="true">·</span>
                <span className="monitor-metric-sub-strong is-error">
                  {t('llmLogs.metricErrors')} {formatCompactNumber(summary.error_count)}
                </span>
              </span>
              <span className="monitor-metric-sub monitor-metric-rate">
                {t('llmLogs.metricSuccessRate')} {formatSuccessRate(summary.success_count, summary.request_count)}
              </span>
            </article>

            <article className="monitor-metric-card is-tokens">
              <div className="monitor-metric-head">
                <span className="monitor-metric-icon" aria-hidden="true"><Sparkles size={14} /></span>
                <span className="monitor-metric-label">{t('llmLogs.metricTotalTokens')}</span>
              </div>
              <strong className="monitor-metric-value mono">
                <TokenCompact
                  value={
                    summary.input_tokens
                    + summary.output_tokens
                    + summary.cache_read_tokens
                    + summary.cache_write_tokens
                  }
                />
              </strong>
              <span className="monitor-metric-sub">
                {t('llmLogs.metricCost')} {formatCostValue(summary.cost_usd, t)}
              </span>
              <span className="monitor-metric-sub monitor-metric-rate">
                {t('llmLogs.total', { count: summary.success_count })}
              </span>
            </article>

            <article className="monitor-metric-card is-io">
              <div className="monitor-metric-head">
                <span className="monitor-metric-icon" aria-hidden="true"><ArrowDownToLine size={14} /></span>
                <span className="monitor-metric-label">{t('llmLogs.metricTokensIO')}</span>
              </div>
              <div className="monitor-metric-split">
                <div className="monitor-metric-split-item">
                  <span className="monitor-metric-split-label">
                    <ArrowDownToLine size={12} aria-hidden="true" />
                    {t('llmLogs.metricInputTokens')}
                  </span>
                  <strong className="monitor-metric-split-value mono">
                    <TokenCompact value={summary.input_tokens} />
                  </strong>
                  <span className="monitor-metric-share">
                    {formatSharePercent(
                      summary.input_tokens,
                      summary.input_tokens
                        + summary.output_tokens
                        + summary.cache_read_tokens
                        + summary.cache_write_tokens,
                      t,
                    )}
                  </span>
                </div>
                <div className="monitor-metric-split-item">
                  <span className="monitor-metric-split-label">
                    <ArrowUpFromLine size={12} aria-hidden="true" />
                    {t('llmLogs.metricOutputTokens')}
                  </span>
                  <strong className="monitor-metric-split-value mono">
                    <TokenCompact value={summary.output_tokens} />
                  </strong>
                  <span className="monitor-metric-share">
                    {formatSharePercent(
                      summary.output_tokens,
                      summary.input_tokens
                        + summary.output_tokens
                        + summary.cache_read_tokens
                        + summary.cache_write_tokens,
                      t,
                    )}
                  </span>
                </div>
              </div>
            </article>

            <article className="monitor-metric-card is-cache">
              <div className="monitor-metric-head">
                <span className="monitor-metric-icon" aria-hidden="true"><Database size={14} /></span>
                <span className="monitor-metric-label">{t('llmLogs.metricCacheIO')}</span>
              </div>
              <div className="monitor-metric-split">
                <div className="monitor-metric-split-item">
                  <span className="monitor-metric-split-label">
                    <ArrowDownToLine size={12} aria-hidden="true" />
                    {t('llmLogs.metricCacheReadTokens')}
                  </span>
                  <strong className="monitor-metric-split-value mono">
                    <TokenCompact value={summary.cache_read_tokens} />
                  </strong>
                  <span className="monitor-metric-share">
                    {formatSharePercent(
                      summary.cache_read_tokens,
                      summary.input_tokens
                        + summary.output_tokens
                        + summary.cache_read_tokens
                        + summary.cache_write_tokens,
                      t,
                    )}
                  </span>
                </div>
                <div className="monitor-metric-split-item">
                  <span className="monitor-metric-split-label">
                    <ArrowUpFromLine size={12} aria-hidden="true" />
                    {t('llmLogs.metricCacheWriteTokens')}
                  </span>
                  <strong className="monitor-metric-split-value mono">
                    <TokenCompact value={summary.cache_write_tokens} />
                  </strong>
                  <span className="monitor-metric-share">
                    {formatSharePercent(
                      summary.cache_write_tokens,
                      summary.input_tokens
                        + summary.output_tokens
                        + summary.cache_read_tokens
                        + summary.cache_write_tokens,
                      t,
                    )}
                  </span>
                </div>
              </div>
            </article>
          </div>
        </section>

        <div className="monitor-table-wrap form-card">
          {isLoading ? (
            <div className="monitor-empty">
              <p className="muted">{t('sidebar.loading')}</p>
            </div>
          ) : logs.length === 0 ? (
            <div className="monitor-empty">
              <ChartColumn size={28} className="empty-icon" />
              <p className="muted">{t('llmLogs.empty')}</p>
            </div>
          ) : (
            <table className="monitor-request-table">
              <thead>
                <tr>
                  <th>{t('llmLogs.time')}</th>
                  <th>{t('llmLogs.agent')}</th>
                  <th>{t('llmLogs.model')}</th>
                  <th>{t('llmLogs.status')}</th>
                  <th>{t('llmLogs.firstToken')}</th>
                  <th>{t('llmLogs.duration')}</th>
                  <th>{t('llmLogs.tokensIO')}</th>
                  <th>{t('llmLogs.cacheIO')}</th>
                  <th>{t('llmLogs.cost')}</th>
                  <th>{t('llmLogs.logColumn')}</th>
                </tr>
              </thead>
              <tbody>
                {logs.map((log) => {
                  const agent = resolveAgent(log, agentById, agentByName);
                  const agentName = resolveAgentName(log, agentById, agentByName, t);
                  const cost = calculateCostUsd(log, providers);
                  const { date, time } = formatDateParts(log.created_at);
                  const hasPayload = Boolean(log.has_payload);
                  const disabledHint = settings.llm_log_enabled
                    ? t('llmLogs.noPayloadHistoricHint')
                    : t('llmLogs.noPayloadHint');
                  return (
                    <tr key={log.id}>
                      <td>
                        <span className="monitor-time-cell mono">
                          <span className="monitor-time-date">{date}</span>
                          <span className="monitor-time-clock">{time}</span>
                        </span>
                      </td>
                      <td>
                        <span className="monitor-agent-cell">
                          <AvatarBadge
                            icon={agent?.avatar_icon ?? 'bot'}
                            color={agent?.avatar_color ?? '#64748b'}
                            size={14}
                            className="row-avatar monitor-agent-avatar"
                          />
                          <span>{agentName}</span>
                        </span>
                      </td>
                      <td>
                        <span className="monitor-model-cell">
                          <span className="monitor-model-provider">{log.provider_name}</span>
                          <span className="monitor-model-id mono">{log.model_id}</span>
                        </span>
                      </td>
                      <td>
                        <StatusBadge status={log.raw_status} unknownLabel={t('llmLogs.unknown')} />
                      </td>
                      <td><LatencyBadge ms={log.first_token_ms} t={t} /></td>
                      <td><LatencyBadge ms={log.duration_ms} t={t} /></td>
                      <td>
                        {isFailedRequest(log.raw_status, log.error) ? (
                          <span className="monitor-empty-metric">{t('llmLogs.unknown')}</span>
                        ) : (
                          <SplitValue
                            left={formatTokenCount(log.input_tokens, t)}
                            right={formatTokenCount(log.output_tokens, t)}
                            leftMono
                            rightMono
                          />
                        )}
                      </td>
                      <td>
                        {isFailedRequest(log.raw_status, log.error) ? (
                          <span className="monitor-empty-metric">{t('llmLogs.unknown')}</span>
                        ) : (
                          <SplitValue
                            left={formatTokenCount(log.cache_read_tokens, t)}
                            right={formatTokenCount(log.cache_write_tokens, t)}
                            leftMono
                            rightMono
                          />
                        )}
                      </td>
                      <td>
                        {isFailedRequest(log.raw_status, log.error) ? (
                          <span className="monitor-empty-metric">{t('llmLogs.unknown')}</span>
                        ) : (
                          <span className="monitor-cost-badge">{formatCostValue(cost, t)}</span>
                        )}
                      </td>
                      <td className="monitor-actions">
                        <span
                          className={hasPayload ? 'monitor-log-action' : 'monitor-log-action is-disabled'}
                          title={hasPayload ? undefined : disabledHint}
                        >
                          <IconButton
                            variant="ghost"
                            onClick={() => void handleViewPayload(log.id, hasPayload)}
                            label={hasPayload ? t('llmLogs.viewLog') : t('llmLogs.noPayload')}
                            title={hasPayload ? t('llmLogs.viewLog') : disabledHint}
                            disabled={!hasPayload}
                            className={hasPayload ? undefined : 'is-disabled-log'}
                          >
                            <FileText size={14} />
                          </IconButton>
                        </span>
                      </td>
                    </tr>
                  );
                })}
              </tbody>
            </table>
          )}
        </div>

        <div className="monitor-pagination" aria-label={t('llmLogs.pagination')}>
          <span className="monitor-pagination-range">
            {t('llmLogs.pageRange', { start: rangeStart, end: rangeEnd, total })}
          </span>
          <div className="monitor-pagination-controls">
            <button
              type="button"
              className="monitor-page-nav"
              disabled={page <= 1 || isLoading}
              onClick={() => setPage((current) => Math.max(1, current - 1))}
              aria-label={t('llmLogs.prevPage')}
            >
              <ChevronLeft size={16} />
            </button>
            {pageItems.map((item, index) => (
              item === 'ellipsis' ? (
                <span key={`e-${index}`} className="monitor-page-ellipsis">...</span>
              ) : (
                <button
                  key={item}
                  type="button"
                  className={`monitor-page-btn${item === page ? ' is-active' : ''}`}
                  disabled={isLoading}
                  onClick={() => setPage(item)}
                >
                  {item}
                </button>
              )
            ))}
            <button
              type="button"
              className="monitor-page-nav"
              disabled={page >= totalPages || isLoading}
              onClick={() => setPage((current) => Math.min(totalPages, current + 1))}
              aria-label={t('llmLogs.nextPage')}
            >
              <ChevronRight size={16} />
            </button>
            <label className="monitor-page-size">
              <span>{t('llmLogs.pageSize')}</span>
              <span className="monitor-page-size-control">
                <select
                  value={pageSize}
                  disabled={isLoading}
                  onChange={(event) => {
                    setPageSize(Number(event.target.value) as PageSize);
                    setPage(1);
                  }}
                >
                  {PAGE_SIZE_OPTIONS.map((size) => (
                    <option key={size} value={size}>{size}</option>
                  ))}
                </select>
                <ChevronDown size={14} className="monitor-page-size-chevron" aria-hidden="true" />
              </span>
            </label>
          </div>
        </div>
          </div>
        )}
      />

      {selectedLog ? (
        <Dialog
          open
          title={t('llmLogs.detailTitle')}
          description={`${resolveAgentName(selectedLog, agentById, agentByName, t)} · ${selectedLog.provider_name} · ${selectedLog.model_id}`}
          closeLabel={t('llmLogs.close')}
          onClose={() => setSelectedLog(null)}
          size="xl"
          className="llm-logs-modal monitor-detail-modal"
          bodyClassName="llm-logs-modal-body monitor-detail-modal-body"
        >
          <div className="monitor-detail">
            <div className="monitor-detail-metrics" aria-label={t('llmLogs.metrics')}>
              <div className="monitor-detail-metric">
                <span className="monitor-detail-metric-label">{t('llmLogs.time')}</span>
                <span className="monitor-detail-metric-value">
                  {new Date(selectedLog.created_at).toLocaleString()}
                </span>
              </div>
              <div className="monitor-detail-metric">
                <span className="monitor-detail-metric-label">{t('llmLogs.status')}</span>
                <span className="monitor-detail-metric-value">
                  <StatusBadge status={selectedLog.raw_status} unknownLabel={t('llmLogs.unknown')} />
                </span>
              </div>
              <div className="monitor-detail-metric">
                <span className="monitor-detail-metric-label">{t('llmLogs.firstToken')}</span>
                <span className="monitor-detail-metric-value">
                  <LatencyBadge ms={selectedLog.first_token_ms} t={t} />
                </span>
              </div>
              <div className="monitor-detail-metric">
                <span className="monitor-detail-metric-label">{t('llmLogs.duration')}</span>
                <span className="monitor-detail-metric-value">
                  <LatencyBadge ms={selectedLog.duration_ms} t={t} />
                </span>
              </div>
              <div className="monitor-detail-metric">
                <span className="monitor-detail-metric-label">{t('llmLogs.inputTokens')}</span>
                <span className="monitor-detail-metric-value mono">
                  {formatTokenCount(selectedLog.input_tokens, t)}
                </span>
              </div>
              <div className="monitor-detail-metric">
                <span className="monitor-detail-metric-label">{t('llmLogs.outputTokens')}</span>
                <span className="monitor-detail-metric-value mono">
                  {formatTokenCount(selectedLog.output_tokens, t)}
                </span>
              </div>
              <div className="monitor-detail-metric">
                <span className="monitor-detail-metric-label">{t('llmLogs.cacheReadTokens')}</span>
                <span className="monitor-detail-metric-value mono">
                  {formatTokenCount(selectedLog.cache_read_tokens, t)}
                </span>
              </div>
              <div className="monitor-detail-metric">
                <span className="monitor-detail-metric-label">{t('llmLogs.cacheWriteTokens')}</span>
                <span className="monitor-detail-metric-value mono">
                  {formatTokenCount(selectedLog.cache_write_tokens, t)}
                </span>
              </div>
              <div className="monitor-detail-metric is-accent">
                <span className="monitor-detail-metric-label">{t('llmLogs.cost')}</span>
                <span className="monitor-detail-metric-value">
                  <span className="monitor-cost-badge">
                    {formatCostValue(calculateCostUsd(selectedLog, providers), t)}
                  </span>
                </span>
              </div>
            </div>

            {selectedLog.error ? (
              <div className="monitor-detail-error">
                <strong>{t('llmLogs.error')}</strong>
                <span>{selectedLog.error}</span>
              </div>
            ) : null}

            <div className="monitor-detail-panel">
              <div className="llm-logs-tabs monitor-detail-tabs">
                <button
                  className={`tab-btn ${activeTab === 'request' ? 'active' : ''}`}
                  onClick={() => setActiveTab('request')}
                  type="button"
                >
                  {t('llmLogs.request')}
                </button>
                <button
                  className={`tab-btn ${activeTab === 'response' ? 'active' : ''}`}
                  onClick={() => setActiveTab('response')}
                  type="button"
                >
                  {t('llmLogs.response')}
                </button>
                <div className="tab-spacer" />
                <div className="monitor-detail-navigation" role="group" aria-label={t('llmLogs.detailNavigation')}>
                  <IconButton
                    variant="ghost"
                    type="button"
                    className="monitor-detail-nav-button"
                    onClick={() => handleNavigateDetail(-1)}
                    disabled={!canGoPrevious || isDetailLoading}
                    label={t('llmLogs.previousRequest')}
                  >
                    <ChevronLeft size={16} />
                  </IconButton>
                  <IconButton
                    variant="ghost"
                    type="button"
                    className="monitor-detail-nav-button"
                    onClick={() => handleNavigateDetail(1)}
                    disabled={!canGoNext || isDetailLoading}
                    label={t('llmLogs.nextRequest')}
                  >
                    <ChevronRight size={16} />
                  </IconButton>
                </div>
              </div>

              <div className="monitor-detail-content">
                <IconButton
                  variant="ghost"
                  type="button"
                  className="monitor-detail-copy-button"
                  onClick={handleCopy}
                  disabled={isDetailLoading}
                  label={copied ? t('chat.copied') : t('chat.copy')}
                  title={copied ? t('chat.copied') : t('chat.copy')}
                >
                  <Copy size={15} />
                </IconButton>
                {activeTab === 'request' ? (
                  <pre className="llm-logs-content monitor-detail-code">
                    {formatRawJson(selectedLog.raw_request)}
                  </pre>
                ) : formattedResponse?.isStreaming ? (
                  <div className="llm-logs-response-content monitor-detail-response">
                    <section className="llm-logs-content-section">
                      <h3 className="llm-logs-content-label">{t('llmLogs.aggregatedResponse')}</h3>
                      <pre className="llm-logs-content llm-logs-content-block monitor-detail-code">
                        {formattedResponse.formatted}
                      </pre>
                    </section>
                    <section className="llm-logs-content-section">
                      <h3 className="llm-logs-content-label">{t('llmLogs.streamChunks')}</h3>
                      <pre className="llm-logs-content llm-logs-content-block monitor-detail-code">
                        {formattedResponse.raw}
                      </pre>
                    </section>
                  </div>
                ) : (
                  <pre className="llm-logs-content monitor-detail-code">
                    {formattedResponse?.formatted || selectedLog.error || '(empty)'}
                  </pre>
                )}
              </div>
            </div>
          </div>
        </Dialog>
      ) : null}
    </>
  );
}
