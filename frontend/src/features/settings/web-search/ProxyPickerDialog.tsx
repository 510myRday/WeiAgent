import { useMemo, useState } from 'react';
import { CircleAlert, CircleX, ExternalLink, LoaderCircle, Network, Pin, Search } from 'lucide-react';

import type { IpProxy, IpProxyProtocol } from '../../../types';
import { sortIpProxies } from '../IpProxySettings';
import { useI18n } from '../../../i18n';
import { Button, Dialog } from '../../../components/ui';
import type { ProxyLatencySort } from './types';
import { displayProxyUrl, formatProxyLocation, proxyProtocolLabel } from './utils';

interface ProxyPickerDialogProps {
  value: string | null;
  ipProxies: IpProxy[];
  isLoading: boolean;
  onSelect: (url: string | null) => void;
  onClose: () => void;
  onOpenIpProxies?: () => void;
}

export function ProxyPickerDialog({
  value,
  ipProxies,
  isLoading,
  onSelect,
  onClose,
  onOpenIpProxies,
}: ProxyPickerDialogProps) {
  const { t } = useI18n();
  const [proxySearch, setProxySearch] = useState('');
  const [proxyProtocolFilter, setProxyProtocolFilter] = useState<'all' | IpProxyProtocol>('all');
  const [proxyLatencySort, setProxyLatencySort] = useState<ProxyLatencySort>('latency_asc');

  const selectedProxy = ipProxies.find((proxy) => (
    proxy.url === value
    && proxy.enabled
    && proxy.status === 'available'
  )) ?? null;
  const filteredIpProxies = useMemo(() => {
    const query = proxySearch.trim().toLocaleLowerCase();
    const eligibleProxies = ipProxies.filter((proxy) => (
      proxy.enabled
      && proxy.status === 'available'
      && (
        (!query || [proxy.name, proxy.url, proxy.ip_address, proxy.country, proxy.region, proxy.city]
          .some((value) => value?.toLocaleLowerCase().includes(query)))
        && (proxyProtocolFilter === 'all' || proxy.protocol === proxyProtocolFilter)
      )
    ));
    return sortIpProxies(
      eligibleProxies,
      'latency',
      proxyLatencySort === 'latency_asc' ? 'asc' : 'desc',
    );
  }, [ipProxies, proxyLatencySort, proxyProtocolFilter, proxySearch]);

  return (
    <Dialog
      open
      title={t('tools.webSearch.chooseIpProxyTitle')}
      description={t('tools.webSearch.chooseIpProxyDescription')}
      icon={<Network size={19} />}
      closeLabel={t('common.close')}
      onClose={onClose}
      size="lg"
      className="web-search-proxy-picker-dialog"
      bodyClassName="web-search-proxy-picker-dialog__body"
      headerActions={onOpenIpProxies ? (
        <Button type="button" variant="secondary" icon={<ExternalLink size={15} />} onClick={() => {
          onClose();
          onOpenIpProxies();
        }}>
          {t('tools.webSearch.openIpProxySettings')}
        </Button>
      ) : null}
    >
      <div className="web-search-proxy-picker-dialog__toolbar">
        <label className="web-search-proxy-picker-dialog__search">
          <Search size={16} aria-hidden="true" />
          <input
            type="search"
            value={proxySearch}
            onChange={(event) => setProxySearch(event.target.value)}
            placeholder={t('tools.webSearch.proxyPickerSearch')}
            aria-label={t('tools.webSearch.proxyPickerSearch')}
          />
        </label>
        <select
          value={proxyProtocolFilter}
          onChange={(event) => setProxyProtocolFilter(event.target.value as 'all' | IpProxyProtocol)}
          aria-label={t('tools.webSearch.proxyPickerProtocolFilter')}
        >
          <option value="all">{t('ipProxy.allProtocols')}</option>
          <option value="http">HTTP</option>
          <option value="https">HTTPS</option>
          <option value="socks4">SOCKS4</option>
          <option value="socks5">SOCKS5</option>
        </select>
        <select
          value={proxyLatencySort}
          onChange={(event) => setProxyLatencySort(event.target.value as ProxyLatencySort)}
          aria-label={t('tools.webSearch.proxyPickerLatencySort')}
        >
          <option value="latency_asc">{t('tools.webSearch.proxyPickerLatencyAsc')}</option>
          <option value="latency_desc">{t('tools.webSearch.proxyPickerLatencyDesc')}</option>
        </select>
      </div>
      <div className="web-search-proxy-picker-dialog__summary">
        <span>{t('tools.webSearch.proxyPickerCount', { count: filteredIpProxies.length })}</span>
        {selectedProxy ? <span>{t('tools.webSearch.proxyPickerSelected', { name: selectedProxy.name })}</span> : null}
      </div>
      {isLoading ? (
        <div className="web-search-proxy-picker-dialog__empty" role="status">
          <LoaderCircle size={20} className="is-spinning" />
          <span>{t('tools.webSearch.proxyPickerLoading')}</span>
        </div>
      ) : (
        <div className="web-search-proxy-options" role="listbox" aria-label={t('tools.webSearch.chooseIpProxyTitle')}>
          <button
            type="button"
            role="option"
            aria-selected={!value}
            className={`web-search-proxy-option is-clear${!value ? ' is-selected' : ''}`}
            onClick={() => onSelect(null)}
          >
            <CircleX size={18} />
            <span className="web-search-proxy-option__main is-clear">
              <strong>{t('tools.webSearch.proxyPickerClear')}</strong>
              <small>{t('tools.webSearch.proxyPickerClearHint')}</small>
            </span>
          </button>
          {filteredIpProxies.map((proxy) => (
            <button
              key={proxy.id}
              type="button"
              role="option"
              aria-selected={proxy.url === value}
              className={`web-search-proxy-option${proxy.url === value ? ' is-selected' : ''}`}
              onClick={() => onSelect(proxy.url)}
            >
              <span className="web-search-proxy-option__pin">
                {proxy.is_pinned ? <Pin size={17} fill="currentColor" aria-hidden="true" /> : null}
              </span>
              <span className="web-search-proxy-option__main">
                <strong className={proxy.is_pinned ? 'is-pinned' : ''}>
                  <span>{proxy.name}</span>
                </strong>
                <code title={proxy.url}>{displayProxyUrl(proxy.url)}</code>
                <small>{formatProxyLocation(proxy)}</small>
              </span>
              <span className="web-search-proxy-option__meta">
                <span className="web-search-proxy-option__badge is-protocol">
                  {proxyProtocolLabel(proxy.protocol)}
                </span>
                <span>{proxy.latency_ms == null ? '--' : `${proxy.latency_ms} ms`}</span>
              </span>
            </button>
          ))}
          {filteredIpProxies.length === 0 ? (
            <div className="web-search-proxy-picker-dialog__empty">
              <CircleAlert size={20} />
              <span>{t('tools.webSearch.proxyPickerEmpty')}</span>
            </div>
          ) : null}
        </div>
      )}
    </Dialog>
  );
}
