import { memo } from 'react';
import {
  ArrowDown,
  ArrowUp,
  CheckCircle2,
  ChevronDown,
  CircleAlert,
  Clock3,
  KeyRound,
  LoaderCircle,
  Pause,
  Plus,
  Play,
  ScanSearch,
  Settings2,
  Trash2,
} from 'lucide-react';

import type {
  WebSearchKeyCheckResult,
  WebSearchSourceRole,
  WebSearchSourceConfig,
} from '../../../api';
import type { IpProxy } from '../../../types';
import { useI18n } from '../../../i18n';
import { Button, IconButton } from '../../../components/ui';
import { SOURCE_ROLE_GROUPS } from './types';
import type { KeyCheckProgress, ProxyPickerField, SourceSection } from './types';
import {
  displayProxyUrl,
  formatCheckedAt,
  keyStatusIcon,
  keyStatusLabel,
  keyStatusRank,
  providerLabel,
  sourceCanBeEnabled,
  summarizeKeyStatuses,
} from './utils';

interface SourceCardProps {
  source: WebSearchSourceConfig;
  index: number;
  total: number;
  isExpanded: boolean;
  expandedSections: Partial<Record<SourceSection, boolean>> | undefined;
  selectedKeyIndices: number[];
  sourceStatuses: Record<number, WebSearchKeyCheckResult>;
  checkProgress: KeyCheckProgress | undefined;
  hasPausedChecks: boolean;
  isChecking: boolean;
  isDeleting: boolean;
  isImporting: boolean;
  ipProxies: IpProxy[];
  onToggleExpanded: (sourceId: string) => void;
  onToggleSection: (sourceId: string, section: SourceSection) => void;
  onUpdateSource: (index: number, patch: Partial<WebSearchSourceConfig>) => void;
  onMoveSource: (index: number, direction: -1 | 1) => void;
  onToggleRole: (index: number, role: WebSearchSourceRole, checked: boolean) => void;
  onSelectAllKeys: (sourceId: string, count: number) => void;
  onToggleKeySelection: (sourceId: string, keyIndex: number, selected: boolean) => void;
  onCheckKeys: (sourceId: string, indices: number[]) => void;
  onPauseCheck: (sourceId: string) => void;
  onResumeCheck: (sourceId: string) => void;
  onDeleteSelectedKeys: (sourceId: string) => void;
  onClearKeys: (sourceId: string) => void;
  onRemoveKey: (sourceId: string, keyIndex: number) => void;
  onAddKey: (sourceId: string) => void;
  onOpenProxyPicker: (sourceIndex: number, field: ProxyPickerField) => void;
}

export const SourceCard = memo(function SourceCard({
  source,
  index,
  total,
  isExpanded,
  expandedSections,
  selectedKeyIndices,
  sourceStatuses,
  checkProgress,
  hasPausedChecks,
  isChecking,
  isDeleting,
  isImporting,
  ipProxies,
  onToggleExpanded,
  onToggleSection,
  onUpdateSource,
  onMoveSource,
  onToggleRole,
  onSelectAllKeys,
  onToggleKeySelection,
  onCheckKeys,
  onPauseCheck,
  onResumeCheck,
  onDeleteSelectedKeys,
  onClearKeys,
  onRemoveKey,
  onAddKey,
  onOpenProxyPicker,
}: SourceCardProps) {
  const { t } = useI18n();
  const keyCount = source.api_keys.length;
  const selected = selectedKeyIndices
    .filter((keyIndex) => keyIndex >= 0 && keyIndex < keyCount);
  const canEnableSource = sourceCanBeEnabled(source);
  const isPreferencesExpanded = Boolean(expandedSections?.preferences);
  const isKeysExpanded = Boolean(expandedSections?.keys);
  const isConnectionExpanded = Boolean(expandedSections?.connection);
  const allSelected = keyCount > 0 && selected.length === keyCount;
  const statusSummary = summarizeKeyStatuses(keyCount, sourceStatuses);
  const currentCheckProgress = checkProgress;
  const checkIndices = selected.length > 0
    ? selected
    : source.api_keys.map((_, keyIndex) => keyIndex);
  const emptyLabel = t(source.provider === 'grok' ? 'tools.webSearch.noSso' : 'tools.webSearch.noKeys');
  const sourceBodyId = `web-search-source-body-${source.id}`;
  const preferencesBodyId = `web-search-preferences-body-${source.id}`;
  const keysBodyId = `web-search-keys-body-${source.id}`;
  const connectionBodyId = `web-search-connection-body-${source.id}`;

  function renderProxySelector(field: ProxyPickerField, labelKey: string, descriptionKey: string) {
    const value = source[field];
    const currentProxy = ipProxies.find((proxy) => proxy.url === value) ?? null;
    const label = t(labelKey);
    return (
      <div className="web-search-extra-field">
        <div className="web-search-extra-field__label">
          <span>{label}</span>
          <p className="setting-desc">{t(descriptionKey)}</p>
        </div>
        <button
          type="button"
          className={`web-search-proxy-picker${currentProxy ? '' : ' is-empty'}`}
          onClick={() => onOpenProxyPicker(index, field)}
          aria-label={label}
        >
          <span className="web-search-proxy-picker__copy">
            {currentProxy ? (
              <>
                <strong>{currentProxy.name}</strong>
                <code>{displayProxyUrl(currentProxy.url)}</code>
              </>
            ) : value ? (
              <>
                <strong className="is-warning"><CircleAlert size={14} />{t('tools.webSearch.proxyNotInList')}</strong>
                <code>{displayProxyUrl(value)}</code>
              </>
            ) : (
              <span>{t('tools.webSearch.selectIpProxy')}</span>
            )}
          </span>
          <ChevronDown size={17} aria-hidden="true" />
        </button>
      </div>
    );
  }

  return (
    <div
      className={`web-search-source ${source.enabled ? 'is-enabled' : 'is-disabled'} ${isExpanded ? 'is-expanded' : 'is-collapsed'}`}
    >
      <div className="web-search-source__header">
        <button
          type="button"
          className="web-search-source__summary"
          aria-expanded={isExpanded}
          aria-controls={sourceBodyId}
          onClick={() => onToggleExpanded(source.id)}
        >
          <span
            className="web-search-source__priority"
            title={t('tools.webSearch.sourcePriority', { index: String(index + 1) })}
          >
            {index + 1}
          </span>
          <span className="web-search-source__meta">
            <span className="web-search-source__title-line">
              <strong>{providerLabel(source.provider, t)}</strong>
            </span>
            <span className="web-search-source__health" aria-label={t('tools.webSearch.statusSummary')}>
              <span className="is-total">
                <KeyRound size={13} />
                {t('tools.webSearch.totalKeyCount', { count: keyCount })}
              </span>
              <span className="is-healthy">
                <CheckCircle2 size={13} />
                {t('tools.webSearch.healthyCount', { count: statusSummary.healthy })}
              </span>
              <span className="is-issue">
                <CircleAlert size={13} />
                {t('tools.webSearch.issueCount', { count: statusSummary.issues })}
              </span>
              <span className="is-unchecked">
                <Clock3 size={13} />
                {t('tools.webSearch.uncheckedCount', { count: statusSummary.unchecked })}
              </span>
            </span>
          </span>
          <ChevronDown size={18} className="web-search-source__chevron" />
        </button>
        <div className="web-search-source__actions">
          <label
            className={`toggle-switch web-search-source__toggle ${canEnableSource ? '' : 'is-disabled'}`}
            title={canEnableSource
              ? t(source.enabled ? 'tools.enabled' : 'tools.disabled')
              : t('tools.webSearch.keyRequiredToEnable')}
          >
            <input
              type="checkbox"
              checked={source.enabled}
              disabled={!canEnableSource}
              onChange={(e) => onUpdateSource(index, { enabled: e.target.checked })}
            />
            <span className="toggle-slider" />
            <span className="web-search-source__toggle-text">
              {t(source.enabled ? 'tools.enabled' : 'tools.disabled')}
            </span>
          </label>
          <div
            className="web-search-source__reorder"
            role="group"
            aria-label={t('tools.webSearch.sourcePriority', { index: String(index + 1) })}
          >
            <IconButton
              type="button"
              variant="ghost"
              className="web-search-source__reorder-btn"
              label={t('tools.webSearch.moveUp')}
              onClick={() => onMoveSource(index, -1)}
              disabled={index === 0}
            >
              <ArrowUp size={15} />
            </IconButton>
            <IconButton
              type="button"
              variant="ghost"
              className="web-search-source__reorder-btn"
              label={t('tools.webSearch.moveDown')}
              onClick={() => onMoveSource(index, 1)}
              disabled={index === total - 1}
            >
              <ArrowDown size={15} />
            </IconButton>
          </div>
        </div>
      </div>

      {isExpanded ? (
        <div className="web-search-source__body" id={sourceBodyId}>
          <section className={`web-search-subsection web-search-source-profile ${isPreferencesExpanded ? 'is-open' : ''}`}>
            <div className="web-search-subsection__header">
              <button
                type="button"
                className="web-search-subsection__toggle"
                aria-expanded={isPreferencesExpanded}
                aria-controls={preferencesBodyId}
                onClick={() => onToggleSection(source.id, 'preferences')}
              >
                <span className="web-search-section-icon"><Settings2 size={15} /></span>
                <span className="web-search-subsection__copy">
                  <strong>{t('tools.webSearch.sourceProfile')}</strong>
                  <small>{t('tools.webSearch.sourceProfileDesc')}</small>
                </span>
                <ChevronDown size={16} className="web-search-subsection__chevron" />
              </button>
            </div>
            {isPreferencesExpanded ? (
              <div
                className="web-search-subsection__content web-search-source-profile__content"
                id={preferencesBodyId}
              >
                <div className="web-search-source-profile__groups">
                  {SOURCE_ROLE_GROUPS.map((group) => (
                    <fieldset className="web-search-source-profile__group" key={group.key}>
                      <legend>{t(`tools.webSearch.sourcePreferenceGroup.${group.key}`)}</legend>
                      <div className="web-search-source-profile__options">
                        {group.roles.map((role) => (
                          <label key={role} title={t(`tools.webSearch.sourceRoleDesc.${role}`)}>
                            <input
                              type="checkbox"
                              checked={source.roles.includes(role)}
                              onChange={(event) => onToggleRole(index, role, event.target.checked)}
                            />
                            <span>{t(`tools.webSearch.sourceRole.${role}`)}</span>
                          </label>
                        ))}
                        {group.key === 'intent' ? (
                          <>
                            <label title={t('tools.webSearch.deepEnabledDesc')}>
                              <input
                                type="checkbox"
                                checked={source.deep_enabled}
                                onChange={(event) => onUpdateSource(index, { deep_enabled: event.target.checked })}
                              />
                              <span>{t('tools.webSearch.deepEnabled')}</span>
                            </label>
                            <label title={t('tools.webSearch.extractEnabledDesc')}>
                              <input
                                type="checkbox"
                                checked={source.extract_enabled}
                                onChange={(event) => onUpdateSource(index, { extract_enabled: event.target.checked })}
                              />
                              <span>{t('tools.webSearch.extractEnabled')}</span>
                            </label>
                          </>
                        ) : null}
                      </div>
                    </fieldset>
                  ))}
                </div>
              </div>
            ) : null}
          </section>

          <section className={`web-search-subsection web-search-keys ${isKeysExpanded ? 'is-open' : ''}`}>
            <div className={`web-search-subsection__header web-search-keys__heading ${selected.length > 0 ? 'has-selection' : ''}`}>
              <button
                type="button"
                className="web-search-subsection__toggle"
                aria-expanded={isKeysExpanded}
                aria-controls={keysBodyId}
                onClick={() => onToggleSection(source.id, 'keys')}
              >
                <span className="web-search-section-icon"><KeyRound size={15} /></span>
                <span className="web-search-subsection__copy">
                  <strong>{t('tools.webSearch.keysLabel')}</strong>
                  <small>{t('tools.webSearch.keysSectionDesc')}</small>
                </span>
                <ChevronDown size={16} className="web-search-subsection__chevron" />
              </button>
              <div className="web-search-keys__toolbar">
                {isKeysExpanded && keyCount > 0 ? (
                  <label className="web-search-keys__select-all">
                    <input
                      type="checkbox"
                      checked={allSelected}
                      ref={(node) => {
                        if (node) node.indeterminate = selected.length > 0 && !allSelected;
                      }}
                      onChange={() => onSelectAllKeys(source.id, allSelected ? 0 : keyCount)}
                    />
                    <span>{allSelected ? t('tools.webSearch.clearSelection') : t('tools.webSearch.selectAll')}</span>
                  </label>
                ) : null}
                {isKeysExpanded && keyCount > 0 ? (
                  <>
                    <Button
                      type="button"
                      variant="secondary"
                      className="web-search-keys__check-btn"
                      icon={isChecking
                        ? (currentCheckProgress?.pauseRequested
                          ? <LoaderCircle size={14} className="is-spinning" />
                          : <Pause size={14} />)
                        : hasPausedChecks ? <Play size={14} /> : <ScanSearch size={15} />}
                      title={t('tools.webSearch.keyCheckHint')}
                      onClick={() => {
                        if (isChecking) {
                          onPauseCheck(source.id);
                        } else if (hasPausedChecks) {
                          onResumeCheck(source.id);
                        } else {
                          onCheckKeys(source.id, checkIndices);
                        }
                      }}
                      disabled={isDeleting || Boolean(currentCheckProgress?.pauseRequested)}
                    >
                      {isChecking
                        ? (currentCheckProgress?.pauseRequested
                          ? t('tools.webSearch.checkPausing')
                          : t('tools.webSearch.pauseCheck'))
                        : hasPausedChecks
                          ? t('tools.webSearch.resumeCheck')
                          : selected.length > 0
                            ? t('tools.webSearch.checkSelected', { count: selected.length })
                            : t('tools.webSearch.checkAll')}
                    </Button>
                    {currentCheckProgress ? (
                      <span className={`web-search-key-progress ${currentCheckProgress.paused ? 'is-paused' : ''}`} role="status">
                        {currentCheckProgress.paused
                          ? t('tools.webSearch.checkProgressPaused', {
                            completed: currentCheckProgress.completed,
                            total: currentCheckProgress.total,
                            healthy: currentCheckProgress.healthy,
                            issues: currentCheckProgress.issues,
                            remaining: currentCheckProgress.total - currentCheckProgress.completed,
                          })
                          : t('tools.webSearch.checkProgress', {
                            completed: currentCheckProgress.completed,
                            total: currentCheckProgress.total,
                            healthy: currentCheckProgress.healthy,
                            issues: currentCheckProgress.issues,
                            remaining: currentCheckProgress.total - currentCheckProgress.completed,
                          })}
                      </span>
                    ) : null}
                  </>
                ) : null}
                {isKeysExpanded && selected.length > 0 ? (
                  <Button
                    type="button"
                    variant="danger"
                    className="web-search-keys__delete-selected"
                    icon={isDeleting ? <LoaderCircle size={14} className="is-spinning" /> : <Trash2 size={14} />}
                    onClick={() => onDeleteSelectedKeys(source.id)}
                    disabled={isDeleting || isChecking}
                  >
                    {t('tools.webSearch.deleteSelected')}
                  </Button>
                ) : null}
                {isKeysExpanded && keyCount > 0 ? (
                  <Button
                    type="button"
                    variant="danger"
                    className="web-search-keys__clear"
                    icon={<Trash2 size={14} />}
                    onClick={() => onClearKeys(source.id)}
                    disabled={isDeleting || isChecking}
                  >
                    {t('tools.webSearch.clearKeys')}
                  </Button>
                ) : null}
                <IconButton
                  type="button"
                  variant="default"
                  className="web-search-keys__add"
                  label={t('tools.webSearch.addKey')}
                  onClick={() => onAddKey(source.id)}
                  disabled={isDeleting || isChecking || isImporting}
                >
                  {isImporting
                    ? <LoaderCircle size={17} className="is-spinning" />
                    : <Plus size={18} />}
                </IconButton>
              </div>
            </div>

            {isKeysExpanded ? (
              <div className="web-search-subsection__content web-search-keys__content" id={keysBodyId}>
                {keyCount === 0 ? (
                  <div className="web-search-keys__empty">
                    <KeyRound size={18} />
                    <div>
                      <strong>{emptyLabel}</strong>
                      <span>{t('tools.webSearch.emptyKeysHint')}</span>
                    </div>
                  </div>
                ) : (
                  <ul className="web-search-key-list">
                    {source.api_keys
                      .map((key, keyIndex) => ({ key, keyIndex, status: sourceStatuses[keyIndex] }))
                      .sort((left, right) => (
                        keyStatusRank(left.status?.status) - keyStatusRank(right.status?.status)
                        || left.keyIndex - right.keyIndex
                      ))
                      .map(({ key, keyIndex, status }) => (
                        <li
                          key={`${source.id}-${keyIndex}`}
                          className={`web-search-key-card ${selected.includes(keyIndex) ? 'is-selected' : ''}`}
                        >
                          <input
                            type="checkbox"
                            className="web-search-key-card__select"
                            aria-label={t('tools.webSearch.selectKey', { index: keyIndex + 1 })}
                            checked={selected.includes(keyIndex)}
                            onChange={(event) => onToggleKeySelection(source.id, keyIndex, event.target.checked)}
                          />
                          <div className="web-search-key-card__content">
                            <div className="web-search-key-card__topline">
                              <code title={key}>{key}</code>
                              {status ? (
                                <span
                                  className={`web-search-key-status is-${status.status}`}
                                  title={status.detail}
                                >
                                  {keyStatusIcon(status.status)}
                                  <span>{keyStatusLabel(status.status, t)}</span>
                                </span>
                              ) : (
                                <span className="web-search-key-status is-unchecked">
                                  <Clock3 size={14} />
                                  <span>{t('tools.webSearch.unchecked')}</span>
                                </span>
                              )}
                            </div>
                            {status ? (
                              <small className="web-search-key-card__status-detail" title={status.detail}>
                                <time dateTime={status.checked_at}>{formatCheckedAt(status.checked_at)}</time>
                                <span aria-hidden="true">·</span>
                                <span>{status.detail}</span>
                              </small>
                            ) : null}
                          </div>
                          <div className="web-search-key-card__actions">
                            <IconButton
                              type="button"
                              variant="ghost"
                              className="web-search-key-card__check"
                              label={t('tools.webSearch.checkOne')}
                              title={t('tools.webSearch.keyCheckHint')}
                              onClick={() => onCheckKeys(source.id, [keyIndex])}
                              disabled={isChecking || isDeleting}
                            >
                              {isChecking && currentCheckProgress?.activeKeyIndex === keyIndex
                                ? <LoaderCircle size={14} className="is-spinning" />
                                : <ScanSearch size={15} />}
                            </IconButton>
                            <IconButton
                              type="button"
                              variant="ghost"
                              className="web-search-key-card__remove"
                              label={t('tools.webSearch.removeKey')}
                              onClick={() => onRemoveKey(source.id, keyIndex)}
                              disabled={isDeleting || isChecking}
                            >
                              {isDeleting ? <LoaderCircle size={14} className="is-spinning" /> : <Trash2 size={14} />}
                            </IconButton>
                          </div>
                        </li>
                      ))}
                  </ul>
                )}
              </div>
            ) : null}
          </section>

          <section className={`web-search-subsection web-search-grok-extras ${isConnectionExpanded ? 'is-open' : ''}`}>
              <div className="web-search-subsection__header">
                <button
                  type="button"
                  className="web-search-subsection__toggle"
                  aria-expanded={isConnectionExpanded}
                  aria-controls={connectionBodyId}
                  onClick={() => onToggleSection(source.id, 'connection')}
                >
                  <span className="web-search-section-icon"><Settings2 size={15} /></span>
                  <span className="web-search-subsection__copy">
                    <strong>{t('tools.webSearch.advancedSettings')}</strong>
                    <small>{t('tools.webSearch.advancedSettingsDesc')}</small>
                  </span>
                  <ChevronDown size={16} className="web-search-subsection__chevron" />
                </button>
              </div>
              {isConnectionExpanded ? (
                <div
                  className="web-search-subsection__content web-search-grok-extras__content"
                  id={connectionBodyId}
                >
                  {renderProxySelector('proxy_url', 'tools.webSearch.proxyUrl', 'tools.webSearch.proxyUrlDesc')}
                  {source.provider === 'grok' ? (
                    <>
                  <div className="web-search-extra-field">
                    <div className="web-search-extra-field__label">
                      <label htmlFor={`grok-flaresolverr-${source.id}`}>{t('tools.webSearch.flaresolverrUrl')}</label>
                      <p className="setting-desc">{t('tools.webSearch.flaresolverrUrlDesc')}</p>
                    </div>
                    <input
                      id={`grok-flaresolverr-${source.id}`}
                      type="text"
                      className="pattern-input web-search-extra-input"
                      value={source.flaresolverr_url ?? ''}
                      placeholder={t('tools.webSearch.flaresolverrUrlPlaceholder')}
                      onChange={(e) => onUpdateSource(index, {
                        flaresolverr_url: e.target.value.trim() ? e.target.value.trim() : null,
                      })}
                    />
                  </div>
                  {renderProxySelector('flaresolverr_proxy_url', 'tools.webSearch.flaresolverrProxyUrl', 'tools.webSearch.flaresolverrProxyUrlDesc')}
                    </>
                  ) : null}
                </div>
              ) : null}
          </section>
        </div>
      ) : null}
    </div>
  );
});
