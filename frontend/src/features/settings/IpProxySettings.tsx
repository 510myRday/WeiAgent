import {
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
  type FormEvent,
} from 'react';
import {
  Activity,
  CheckCircle2,
  CircleAlert,
   Gauge,
  Globe2,
  LoaderCircle,
  Pencil,
  Pin,
  Plus,
  RefreshCw,
  ServerCog,
  Trash2,
  Upload,
  WifiOff,
  X,
} from 'lucide-react';

import {
  createIpProxy,
  deleteIpProxy,
  importIpProxies,
  listIpProxies,
  testIpProxy,
  updateIpProxy,
} from '../../api';
import type {
  IpProxy,
  IpProxyImportEntry,
  IpProxyPayload,
  IpProxyProtocol,
  IpProxyStatus,
} from '../../types';
import {
  Button,
  Dialog,
  EmptyState,
  FormField,
  IconButton,
  SearchInput,
  useFeedback,
} from '../../components/ui';
import { useI18n } from '../../i18n';
import { displayProxyUrl, proxyProtocolLabel } from '../../utils/proxy';
import { userErrorMessage } from '../../utils/userError';
import { formatCheckedAt } from './web-search/utils';
import {
  findImportAddressIssues,
  findImportNameConflicts,
  reconcileImportRows,
  sortIpProxies,
  type ImportProxyRow,
  type ProxySortKey,
  type SortDirection,
} from './ipProxyUtils';
import {
  ApiQualityGrid,
  ApiQualityLegend,
  LatencyValue,
  ProxyLocation,
  ProxyMetric,
  ProxyStatusBadge,
  SortHeader,
} from './ip-proxy/ProxyWidgets';

type ProxyDialog = 'create' | 'edit' | 'import' | null;
type ProtocolFilter = 'all' | IpProxyProtocol;
type StatusFilter = 'all' | IpProxyStatus;

const EMPTY_PROXY_FORM: IpProxyPayload = {
  name: '',
  url: '',
  enabled: true,
};

const TEST_CONCURRENCY = 6;

export function IpProxySettings({
  onCountChange,
}: {
  onCountChange: (count: number) => void;
}) {
  const { t, locale } = useI18n();
  const { confirm, toast } = useFeedback();
  const [proxies, setProxies] = useState<IpProxy[]>([]);
  const [isLoading, setIsLoading] = useState(true);
  const [loadError, setLoadError] = useState('');
  const [search, setSearch] = useState('');
  const [protocolFilter, setProtocolFilter] = useState<ProtocolFilter>('all');
  const [statusFilter, setStatusFilter] = useState<StatusFilter>('all');
  const [selectedIds, setSelectedIds] = useState<Set<string>>(new Set());
  const [testingIds, setTestingIds] = useState<Set<string>>(new Set());
  const [pinningIds, setPinningIds] = useState<Set<string>>(new Set());
  const [sortState, setSortState] = useState<{ key: ProxySortKey; direction: SortDirection }>({
    key: 'latency',
    direction: 'asc',
  });
  const [dialog, setDialog] = useState<ProxyDialog>(null);
  const [editingId, setEditingId] = useState('');
  const [proxyForm, setProxyForm] = useState<IpProxyPayload>(EMPTY_PROXY_FORM);
  const [importText, setImportText] = useState('');
  const [importRows, setImportRows] = useState<ImportProxyRow[]>([]);
  const [importEnabled, setImportEnabled] = useState(true);
  const [isSubmitting, setIsSubmitting] = useState(false);
  const [isDeleting, setIsDeleting] = useState(false);
  const selectAllRef = useRef<HTMLInputElement>(null);
  const importAddressRef = useRef<HTMLTextAreaElement>(null);
  const importNameListRef = useRef<HTMLDivElement>(null);

  const refreshProxies = useCallback(async () => {
    setIsLoading(true);
    setLoadError('');
    try {
      setProxies(await listIpProxies());
    } catch (cause) {
      const message = userErrorMessage(cause, { summary: t('ipProxy.loadFailed') });
      setLoadError(message);
      toast.error(message);
    } finally {
      setIsLoading(false);
    }
  }, [t, toast]);

  useEffect(() => {
    void refreshProxies();
  }, [refreshProxies]);

  useEffect(() => {
    onCountChange(proxies.length);
  }, [onCountChange, proxies.length]);

  const filteredProxies = useMemo(() => {
    const query = search.trim().toLocaleLowerCase();
    const filtered = proxies.filter((proxy) => (
      (!query
        || [
          proxy.name,
          proxy.url,
          proxy.ip_address,
          proxy.country,
          proxy.region,
          proxy.city,
        ].some((value) => value?.toLocaleLowerCase().includes(query)))
      && (protocolFilter === 'all' || proxy.protocol === protocolFilter)
      && (statusFilter === 'all' || proxy.status === statusFilter)
    ));
    return sortIpProxies(filtered, sortState.key, sortState.direction);
  }, [protocolFilter, proxies, search, sortState, statusFilter]);

  const enabledCount = proxies.filter((proxy) => proxy.enabled).length;
  const availableProxies = proxies.filter((proxy) => proxy.status === 'available');
  const averageLatency = availableProxies.length > 0
    ? Math.round(availableProxies.reduce((total, proxy) => total + (proxy.latency_ms ?? 0), 0) / availableProxies.length)
    : null;
  const allVisibleSelected = filteredProxies.length > 0
    && filteredProxies.every((proxy) => selectedIds.has(proxy.id));
  const someVisibleSelected = filteredProxies.some((proxy) => selectedIds.has(proxy.id));
  const proxyFormNameConflict = proxies.some((proxy) => (
    proxy.id !== editingId
    && proxy.name.trim().toLocaleLowerCase() === proxyForm.name.trim().toLocaleLowerCase()
  ));
  const importNameConflictKeys = useMemo(
    () => findImportNameConflicts(importRows, proxies.map((proxy) => proxy.name)),
    [importRows, proxies],
  );
  const importAddressIssueKeys = useMemo(
    () => findImportAddressIssues(importRows, proxies.map((proxy) => proxy.url)),
    [importRows, proxies],
  );
  const selectedTestIds = Array.from(selectedIds);
  const testTargetIds = selectedTestIds.length > 0
    ? selectedTestIds
    : proxies.filter((proxy) => proxy.enabled).map((proxy) => proxy.id);

  useEffect(() => {
    if (selectAllRef.current) {
      selectAllRef.current.indeterminate = someVisibleSelected && !allVisibleSelected;
    }
  }, [allVisibleSelected, someVisibleSelected]);

  function mergeUpdatedProxies(updated: IpProxy[]) {
    const byId = new Map(updated.map((proxy) => [proxy.id, proxy]));
    setProxies((current) => current.map((proxy) => byId.get(proxy.id) ?? proxy));
  }

  function openCreateDialog() {
    setProxyForm(EMPTY_PROXY_FORM);
    setEditingId('');
    setDialog('create');
  }

  function openEditDialog(proxy: IpProxy) {
    setProxyForm({ name: proxy.name, url: proxy.url, enabled: proxy.enabled });
    setEditingId(proxy.id);
    setDialog('edit');
  }

  function openImportDialog() {
    setImportText('');
    setImportRows([]);
    setImportEnabled(true);
    setDialog('import');
  }

  function closeDialog() {
    if (isSubmitting) return;
    setDialog(null);
    setEditingId('');
    setImportText('');
    setImportRows([]);
  }

  function handleImportTextChange(value: string) {
    setImportText(value);
    setImportRows((current) => reconcileImportRows(
      value,
      current,
      proxies.map((proxy) => proxy.name),
    ));
  }

  function handleImportNameChange(key: string, name: string) {
    setImportRows((current) => {
      const edited = current.map((row) => (
        row.key === key ? { ...row, name, customName: true } : row
      ));
      return reconcileImportRows(importText, edited, proxies.map((proxy) => proxy.name));
    });
  }

  function syncImportScroll(source: 'addresses' | 'names') {
    if (source === 'addresses' && importAddressRef.current && importNameListRef.current) {
      importNameListRef.current.scrollTop = importAddressRef.current.scrollTop;
    }
    if (source === 'names' && importAddressRef.current && importNameListRef.current) {
      importAddressRef.current.scrollTop = importNameListRef.current.scrollTop;
    }
  }

  async function handleProxySubmit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    const payload = {
      ...proxyForm,
      name: proxyForm.name.trim(),
      url: proxyForm.url.trim(),
    };
    if (!payload.name || !payload.url || proxyFormNameConflict) return;

    setIsSubmitting(true);
    try {
      if (dialog === 'edit' && editingId) {
        const updated = await updateIpProxy(editingId, payload);
        mergeUpdatedProxies([updated]);
        toast.success(t('ipProxy.saved'));
      } else {
        const created = await createIpProxy(payload);
        setProxies((current) => [...current, created]);
        toast.success(t('ipProxy.created'));
      }
      setDialog(null);
      setEditingId('');
    } catch (cause) {
      toast.error(userErrorMessage(cause, { summary: t('ipProxy.saveFailed') }));
    } finally {
      setIsSubmitting(false);
    }
  }

  async function handleImportSubmit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    const entries: IpProxyImportEntry[] = importRows.map((row) => ({
      name: row.name.trim(),
      url: row.address,
    }));
    if (
      entries.length === 0
      || entries.some((entry) => !entry.name)
      || importNameConflictKeys.size > 0
      || importAddressIssueKeys.size > 0
    ) return;

    setIsSubmitting(true);
    try {
      const result = await importIpProxies(entries, importEnabled);
      setProxies((current) => [...current, ...result.proxies]);
      toast.success(t('ipProxy.importSummary', {
        imported: result.imported_count,
      }));
      setDialog(null);
      setImportText('');
      setImportRows([]);
    } catch (cause) {
      toast.error(userErrorMessage(cause, { summary: t('ipProxy.importFailed') }));
    } finally {
      setIsSubmitting(false);
    }
  }

  async function handleEnabledChange(proxy: IpProxy, enabled: boolean) {
    setProxies((current) => current.map((item) => (
      item.id === proxy.id ? { ...item, enabled } : item
    )));
    try {
      mergeUpdatedProxies([await updateIpProxy(proxy.id, { enabled })]);
    } catch (cause) {
      mergeUpdatedProxies([proxy]);
      toast.error(userErrorMessage(cause, { summary: t('ipProxy.toggleFailed') }));
    }
  }

  async function handlePinToggle(proxy: IpProxy) {
    const isPinned = !proxy.is_pinned;
    setPinningIds((current) => new Set(current).add(proxy.id));
    setProxies((current) => current.map((item) => (
      item.id === proxy.id ? { ...item, is_pinned: isPinned } : item
    )));
    try {
      mergeUpdatedProxies([await updateIpProxy(proxy.id, { is_pinned: isPinned })]);
      toast.success(t(isPinned ? 'ipProxy.pinned' : 'ipProxy.unpinned'));
    } catch (cause) {
      mergeUpdatedProxies([proxy]);
      toast.error(userErrorMessage(cause, { summary: t('ipProxy.pinFailed') }));
    } finally {
      setPinningIds((current) => {
        const next = new Set(current);
        next.delete(proxy.id);
        return next;
      });
    }
  }

  async function handleDelete(proxy: IpProxy) {
    const confirmed = await confirm({
      title: t('ipProxy.deleteTitle'),
      message: t('ipProxy.deleteConfirm', { name: proxy.name }),
      confirmLabel: t('common.delete'),
      tone: 'danger',
    });
    if (!confirmed) return;

    setIsDeleting(true);
    try {
      await deleteIpProxy(proxy.id);
      setProxies((current) => current.filter((item) => item.id !== proxy.id));
      setSelectedIds((current) => {
        const next = new Set(current);
        next.delete(proxy.id);
        return next;
      });
      toast.success(t('ipProxy.deleted'));
    } catch (cause) {
      toast.error(userErrorMessage(cause, { summary: t('ipProxy.deleteFailed') }));
    } finally {
      setIsDeleting(false);
    }
  }

  async function handleDeleteSelected() {
    const ids = Array.from(selectedIds);
    if (ids.length === 0) return;
    const confirmed = await confirm({
      title: t('ipProxy.deleteSelectedTitle'),
      message: t('ipProxy.deleteSelectedConfirm', { count: ids.length }),
      confirmLabel: t('common.delete'),
      tone: 'danger',
    });
    if (!confirmed) return;

    setIsDeleting(true);
    try {
      const results = await Promise.allSettled(ids.map((id) => deleteIpProxy(id)));
      const deletedIds = new Set(ids.filter((_, index) => results[index].status === 'fulfilled'));
      setProxies((current) => current.filter((proxy) => !deletedIds.has(proxy.id)));
      setSelectedIds((current) => {
        const next = new Set(current);
        deletedIds.forEach((id) => next.delete(id));
        return next;
      });
      const failedCount = ids.length - deletedIds.size;
      if (failedCount > 0) {
        toast.warning(t('ipProxy.deletePartial', { deleted: deletedIds.size, failed: failedCount }));
      } else {
        toast.success(t('ipProxy.deletedSelected', { count: deletedIds.size }));
      }
    } finally {
      setIsDeleting(false);
    }
  }

  async function runTests(ids: string[]) {
    const uniqueIds = Array.from(new Set(ids));
    if (uniqueIds.length === 0) {
      toast.info(t('ipProxy.noEnabled'));
      return;
    }
    setTestingIds((current) => new Set([...current, ...uniqueIds]));

    let available = 0;
    let failed = 0;
    let firstError: unknown = null;
    const queue = [...uniqueIds];
    const worker = async () => {
      for (;;) {
        const id = queue.shift();
        if (id == null) return;
        try {
          const updated = await testIpProxy(id);
          mergeUpdatedProxies([updated]);
          if (updated.status === 'available') available += 1;
        } catch (cause) {
          failed += 1;
          firstError = firstError ?? cause;
        } finally {
          setTestingIds((current) => {
            const next = new Set(current);
            next.delete(id);
            return next;
          });
        }
      }
    };
    await Promise.all(Array.from(
      { length: Math.min(TEST_CONCURRENCY, uniqueIds.length) },
      () => worker(),
    ));

    const completed = uniqueIds.length - failed;
    if (failed > 0) {
      toast.error(userErrorMessage(firstError, { summary: t('ipProxy.testFailed') }));
    }
    if (completed > 0) {
      toast.success(t('ipProxy.testSummary', { available, count: completed }));
    }
  }

  function toggleSelection(id: string, selected: boolean) {
    setSelectedIds((current) => {
      const next = new Set(current);
      if (selected) next.add(id);
      else next.delete(id);
      return next;
    });
  }

  function toggleVisibleSelection(selected: boolean) {
    setSelectedIds((current) => {
      const next = new Set(current);
      filteredProxies.forEach((proxy) => {
        if (selected) next.add(proxy.id);
        else next.delete(proxy.id);
      });
      return next;
    });
  }

  function handleSort(key: ProxySortKey) {
    setSortState((current) => current.key === key
      ? { key, direction: current.direction === 'asc' ? 'desc' : 'asc' }
      : { key, direction: 'asc' });
  }

  return (
    <div className="ip-proxy-page">
      <section className="ip-proxy-overview" aria-label={t('ipProxy.overview')}>
        <ProxyMetric icon={<Globe2 size={17} />} label={t('ipProxy.total')} value={proxies.length} />
        <ProxyMetric icon={<ServerCog size={17} />} label={t('ipProxy.enabled')} value={enabledCount} tone="info" />
        <ProxyMetric icon={<CheckCircle2 size={17} />} label={t('ipProxy.available')} value={availableProxies.length} tone="success" />
        <ProxyMetric
          icon={<Gauge size={17} />}
          label={t('ipProxy.averageLatency')}
          value={averageLatency == null ? '--' : `${averageLatency} ms`}
          tone="warning"
        />
      </section>

      <section className="form-card ip-proxy-management">
        <div className="ip-proxy-toolbar">
          <div className="ip-proxy-filters">
            <SearchInput value={search} onChange={setSearch} placeholder={t('ipProxy.search')} label={t('ipProxy.search')} />
            <select
              value={protocolFilter}
              onChange={(event) => setProtocolFilter(event.target.value as ProtocolFilter)}
              aria-label={t('ipProxy.protocolFilter')}
            >
              <option value="all">{t('ipProxy.allProtocols')}</option>
              <option value="http">HTTP</option>
              <option value="https">HTTPS</option>
              <option value="socks4">SOCKS4</option>
              <option value="socks5">SOCKS5</option>
            </select>
            <select
              value={statusFilter}
              onChange={(event) => setStatusFilter(event.target.value as StatusFilter)}
              aria-label={t('ipProxy.statusFilter')}
            >
              <option value="all">{t('ipProxy.allStatuses')}</option>
              <option value="available">{t('ipProxy.status.available')}</option>
              <option value="unavailable">{t('ipProxy.status.unavailable')}</option>
              <option value="unchecked">{t('ipProxy.status.unchecked')}</option>
            </select>
          </div>
          <div className="ip-proxy-toolbar-actions">
            <Button
              type="button"
              variant="secondary"
              icon={<Activity size={15} />}
              disabled={testingIds.size > 0 || isDeleting || testTargetIds.length === 0}
              onClick={() => void runTests(testTargetIds)}
            >
              {testingIds.size > 0
                ? t('ipProxy.testing')
                : selectedTestIds.length > 0
                  ? t('ipProxy.testSelectedCount', { count: selectedTestIds.length })
                  : t('ipProxy.testAll')}
            </Button>
            <Button
              type="button"
              variant="secondary"
              icon={<Upload size={15} />}
              disabled={isDeleting}
              onClick={openImportDialog}
            >
              {t('ipProxy.batchImport')}
            </Button>
            <Button type="button" variant="primary" icon={<Plus size={16} />} disabled={isDeleting} onClick={openCreateDialog}>
              {t('ipProxy.add')}
            </Button>
          </div>
        </div>

        {selectedIds.size > 0 ? (
          <div className="ip-proxy-selection-bar">
            <span>{t('ipProxy.selected', { count: selectedIds.size })}</span>
            <div>
              <Button
                type="button"
                variant="ghost"
                icon={<X size={14} />}
                disabled={testingIds.size > 0 || isDeleting}
                onClick={() => setSelectedIds(new Set())}
              >
                {t('ipProxy.clearSelection')}
              </Button>
              <Button
                type="button"
                variant="danger"
                icon={<Trash2 size={14} />}
                disabled={testingIds.size > 0 || isDeleting}
                onClick={() => void handleDeleteSelected()}
              >
                {t('ipProxy.deleteSelected')}
              </Button>
            </div>
          </div>
        ) : null}

        <div className="ip-proxy-table-wrap">
          {isLoading ? (
            <div className="ip-proxy-empty" role="status">
              <LoaderCircle size={24} className="is-spinning" />
              <span>{t('sidebar.loading')}</span>
            </div>
          ) : loadError ? (
            <EmptyState
              icon={<WifiOff size={27} />}
              title={t('ipProxy.loadFailed')}
              description={loadError}
              action={(
                <Button type="button" variant="secondary" icon={<RefreshCw size={15} />} onClick={() => void refreshProxies()}>
                  {t('common.retry')}
                </Button>
              )}
            />
          ) : proxies.length === 0 ? (
            <EmptyState
              icon={<Globe2 size={28} />}
              title={t('ipProxy.emptyTitle')}
              description={t('ipProxy.emptyDescription')}
              action={(
                <Button type="button" variant="primary" icon={<Plus size={15} />} onClick={openCreateDialog}>
                  {t('ipProxy.addFirst')}
                </Button>
              )}
            />
          ) : filteredProxies.length === 0 ? (
            <EmptyState
              icon={<Globe2 size={27} />}
              title={t('ipProxy.noResults')}
              description={t('ipProxy.noResultsDescription')}
            />
          ) : (
            <table className="ip-proxy-table">
              <colgroup>
                <col className="ip-proxy-col-select" />
                <col className="ip-proxy-col-name" />
                <col className="ip-proxy-col-location" />
                <col className="ip-proxy-col-status" />
                <col className="ip-proxy-col-latency" />
                <col className="ip-proxy-col-quality" />
                <col className="ip-proxy-col-enabled" />
                <col className="ip-proxy-col-actions" />
              </colgroup>
              <thead>
                <tr>
                  <th className="ip-proxy-select-cell">
                    <input
                      ref={selectAllRef}
                      type="checkbox"
                      checked={allVisibleSelected}
                      onChange={(event) => toggleVisibleSelection(event.target.checked)}
                      aria-label={t('ipProxy.selectAll')}
                    />
                  </th>
                  <th>{t('ipProxy.proxy')}</th>
                  <th>{t('ipProxy.location')}</th>
                  <th>{t('ipProxy.statusLabel')}</th>
                  <SortHeader label={t('ipProxy.latency')} sortKey="latency" sortState={sortState} onSort={handleSort} />
                  <SortHeader label={t('ipProxy.apiQuality')} sortKey="quality" sortState={sortState} onSort={handleSort} />
                  <SortHeader label={t('ipProxy.enabled')} sortKey="enabled" sortState={sortState} onSort={handleSort} />
                  <th>{t('ipProxy.actions')}</th>
                </tr>
              </thead>
              <tbody>
                {filteredProxies.map((proxy) => {
                  const isTesting = testingIds.has(proxy.id);
                  return (
                    <tr
                      key={proxy.id}
                      className={[
                        selectedIds.has(proxy.id) ? 'is-selected' : '',
                        proxy.is_pinned ? 'is-pinned' : '',
                      ].filter(Boolean).join(' ')}
                    >
                      <td className="ip-proxy-select-cell">
                        <input
                          type="checkbox"
                          checked={selectedIds.has(proxy.id)}
                          onChange={(event) => toggleSelection(proxy.id, event.target.checked)}
                          aria-label={t('ipProxy.selectProxy', { name: proxy.name })}
                        />
                      </td>
                      <td>
                        <div className="ip-proxy-identity">
                          <div className="ip-proxy-name-line">
                            <strong title={proxy.name}>{proxy.name}</strong>
                            <span className={`ip-proxy-protocol is-${proxy.protocol}`}>{proxyProtocolLabel(proxy.protocol)}</span>
                          </div>
                          <code title={proxy.url}>{displayProxyUrl(proxy.url)}</code>
                        </div>
                      </td>
                      <td>
                        <ProxyLocation proxy={proxy} emptyLabel={t('ipProxy.locationPending')} />
                      </td>
                      <td>
                        <div className="ip-proxy-status-cell">
                          <ProxyStatusBadge proxy={proxy} isTesting={isTesting} label={t(`ipProxy.status.${proxy.status}`)} testingLabel={t('ipProxy.testingStatus')} />
                          <span className="ip-proxy-checked-at">
                            {proxy.last_checked_at ? formatCheckedAt(proxy.last_checked_at, locale) : t('ipProxy.neverChecked')}
                          </span>
                        </div>
                      </td>
                      <td><LatencyValue latency={proxy.latency_ms} status={proxy.status} /></td>
                      <td><ApiQualityGrid quality={proxy.api_quality} isTesting={isTesting} /></td>
                      <td className="ip-proxy-enabled-cell">
                        <label className="toggle-switch ip-proxy-enabled-toggle" title={proxy.enabled ? t('ipProxy.enabled') : t('ipProxy.disabled')}>
                          <input
                            type="checkbox"
                            checked={proxy.enabled}
                            disabled={isTesting || isDeleting}
                            onChange={(event) => void handleEnabledChange(proxy, event.target.checked)}
                            aria-label={t('ipProxy.toggleProxy', { name: proxy.name })}
                          />
                          <span className="toggle-slider" />
                        </label>
                      </td>
                      <td>
                        <div className="ip-proxy-row-actions">
                          <IconButton
                            type="button"
                            variant="ghost"
                            label={t('ipProxy.testOne')}
                            disabled={isTesting || isDeleting}
                            onClick={() => void runTests([proxy.id])}
                          >
                            {isTesting ? <LoaderCircle size={15} className="is-spinning" /> : <Activity size={15} />}
                          </IconButton>
                          <IconButton type="button" variant="ghost" label={t('ipProxy.edit')} disabled={isTesting || isDeleting} onClick={() => openEditDialog(proxy)}>
                            <Pencil size={15} />
                          </IconButton>
                          <IconButton
                            type="button"
                            variant="ghost"
                            active={proxy.is_pinned}
                            aria-pressed={proxy.is_pinned}
                            label={t(proxy.is_pinned ? 'ipProxy.unpin' : 'ipProxy.pin')}
                            disabled={isTesting || isDeleting || pinningIds.has(proxy.id)}
                            onClick={() => void handlePinToggle(proxy)}
                          >
                            <Pin size={15} fill={proxy.is_pinned ? 'currentColor' : 'none'} />
                          </IconButton>
                          <IconButton type="button" variant="danger" label={t('ipProxy.delete')} disabled={isTesting || isDeleting || pinningIds.has(proxy.id)} onClick={() => void handleDelete(proxy)}>
                            <Trash2 size={15} />
                          </IconButton>
                        </div>
                      </td>
                    </tr>
                  );
                })}
              </tbody>
            </table>
          )}
        </div>
      </section>

      <ApiQualityLegend />

      <Dialog
        open={dialog === 'create' || dialog === 'edit'}
        title={dialog === 'edit' ? t('ipProxy.editTitle') : t('ipProxy.createTitle')}
        description={t('ipProxy.formDescription')}
        icon={<ServerCog size={20} />}
        closeLabel={t('common.close')}
        onClose={closeDialog}
        size="md"
        footer={(
          <>
            <Button type="button" variant="secondary" onClick={closeDialog} disabled={isSubmitting}>
              {t('common.cancel')}
            </Button>
            <Button
              type="submit"
              form="ip-proxy-form"
              variant="primary"
              icon={isSubmitting ? <LoaderCircle size={15} className="is-spinning" /> : undefined}
              disabled={isSubmitting || !proxyForm.name.trim() || !proxyForm.url.trim() || proxyFormNameConflict}
            >
              {dialog === 'edit' ? t('ipProxy.save') : t('ipProxy.create')}
            </Button>
          </>
        )}
      >
        <form id="ip-proxy-form" className="ip-proxy-dialog-form" onSubmit={handleProxySubmit}>
          <FormField label={t('ipProxy.name')} htmlFor="ip-proxy-name" required>
            <input
              id="ip-proxy-name"
              value={proxyForm.name}
              onChange={(event) => setProxyForm((current) => ({ ...current, name: event.target.value }))}
              placeholder={t('ipProxy.namePlaceholder')}
              autoComplete="off"
              aria-invalid={proxyFormNameConflict}
              required
            />
            {proxyFormNameConflict ? (
              <p className="ip-proxy-field-error" role="alert">
                <CircleAlert size={13} />
                <span>{t('ipProxy.nameConflict')}</span>
              </p>
            ) : null}
          </FormField>
          <FormField
            label={t('ipProxy.address')}
            htmlFor="ip-proxy-url"
            hint={t('ipProxy.addressHint')}
            required
          >
            <input
              id="ip-proxy-url"
              value={proxyForm.url}
              onChange={(event) => setProxyForm((current) => ({ ...current, url: event.target.value }))}
              placeholder="http://127.0.0.1:7890"
              autoComplete="off"
              spellCheck={false}
              required
            />
          </FormField>
          <div className="ip-proxy-form-toggle">
            <div>
              <strong>{t('ipProxy.enableOnCreate')}</strong>
              <span>{t('ipProxy.enableOnCreateHint')}</span>
            </div>
            <label className="toggle-switch">
              <input
                type="checkbox"
                checked={proxyForm.enabled}
                onChange={(event) => setProxyForm((current) => ({ ...current, enabled: event.target.checked }))}
                aria-label={t('ipProxy.enableOnCreate')}
              />
              <span className="toggle-slider" />
            </label>
          </div>
        </form>
      </Dialog>

      <Dialog
        open={dialog === 'import'}
        title={t('ipProxy.importTitle')}
        description={t('ipProxy.importDescription')}
        icon={<Upload size={20} />}
        closeLabel={t('common.close')}
        onClose={closeDialog}
        size="lg"
        className="ip-proxy-import-dialog"
        bodyClassName="ip-proxy-import-dialog__body"
        footer={(
          <>
            <Button type="button" variant="secondary" onClick={closeDialog} disabled={isSubmitting}>
              {t('common.cancel')}
            </Button>
            <Button
              type="submit"
              form="ip-proxy-import-form"
              variant="primary"
              icon={isSubmitting ? <LoaderCircle size={15} className="is-spinning" /> : <Upload size={15} />}
              disabled={
                isSubmitting
                || importRows.length === 0
                || importRows.some((row) => !row.name.trim())
                || importNameConflictKeys.size > 0
                || importAddressIssueKeys.size > 0
              }
            >
              {importRows.length > 0
                ? t('ipProxy.importActionCount', { count: importRows.length })
                : t('ipProxy.importAction')}
            </Button>
          </>
        )}
      >
        <form id="ip-proxy-import-form" className="ip-proxy-dialog-form ip-proxy-import-form" onSubmit={handleImportSubmit}>
          <div className="ip-proxy-import-workspace">
            <FormField
              label={t('ipProxy.importAddresses')}
              htmlFor="ip-proxy-import"
              hint={t('ipProxy.importHint')}
              action={importAddressIssueKeys.size > 0 ? (
                <span className="ip-proxy-import-issue-count">
                  {t('ipProxy.importAddressIssueCount', { count: importAddressIssueKeys.size })}
                </span>
              ) : undefined}
              required
            >
              <textarea
                ref={importAddressRef}
                id="ip-proxy-import"
                value={importText}
                onChange={(event) => handleImportTextChange(event.target.value)}
                onScroll={() => syncImportScroll('addresses')}
                placeholder={'http://127.0.0.1:7890\nsocks4://proxy.example.com:1080'}
                rows={9}
                spellCheck={false}
                aria-invalid={importAddressIssueKeys.size > 0}
                required
              />
            </FormField>

            <div className="form-field ip-proxy-import-names">
              <label>
                <span>{t('ipProxy.importNames')}</span>
                <small className={importNameConflictKeys.size > 0 ? 'is-error' : ''}>
                  {importNameConflictKeys.size > 0
                    ? t('ipProxy.importNameConflictCount', { count: importNameConflictKeys.size })
                    : t('ipProxy.importNamesCount', { count: importRows.length })}
                </small>
              </label>
              <div
                ref={importNameListRef}
                className={`ip-proxy-import-name-list${importRows.length === 0 ? ' is-empty' : ''}`}
                onScroll={() => syncImportScroll('names')}
              >
                {importRows.length === 0 ? (
                  <div className="ip-proxy-import-name-empty">
                    <ServerCog size={20} />
                    <span>{t('ipProxy.importNamesEmpty')}</span>
                  </div>
                ) : importRows.map((row, index) => (
                  <label
                    className={`ip-proxy-import-name-row${importNameConflictKeys.has(row.key) ? ' has-error' : ''}`}
                    key={row.key}
                  >
                    <span>{index + 1}</span>
                    <input
                      value={row.name}
                      onChange={(event) => handleImportNameChange(row.key, event.target.value)}
                      aria-label={t('ipProxy.importNameAria', { index: index + 1 })}
                      aria-invalid={importNameConflictKeys.has(row.key)}
                      autoComplete="off"
                      required
                    />
                    {importNameConflictKeys.has(row.key) ? <CircleAlert size={14} /> : null}
                  </label>
                ))}
              </div>
              <p className="form-field-hint">{t('ipProxy.importNamesHint')}</p>
            </div>
          </div>
          <div className="ip-proxy-form-toggle">
            <div>
              <strong>{t('ipProxy.enableImported')}</strong>
              <span>{t('ipProxy.enableImportedHint')}</span>
            </div>
            <label className="toggle-switch">
              <input
                type="checkbox"
                checked={importEnabled}
                onChange={(event) => setImportEnabled(event.target.checked)}
                aria-label={t('ipProxy.enableImported')}
              />
              <span className="toggle-slider" />
            </label>
          </div>
        </form>
      </Dialog>
    </div>
  );
}

export {
  deriveProxyName,
  findImportAddressIssues,
  findImportNameConflicts,
  parseProxyAddresses,
  reconcileImportRows,
  sortIpProxies,
} from './ipProxyUtils';
export type { ImportProxyRow, ProxySortKey } from './ipProxyUtils';


