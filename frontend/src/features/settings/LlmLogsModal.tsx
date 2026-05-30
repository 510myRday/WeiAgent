import { useState, useEffect, useCallback } from 'react';
import { Copy, FileText, RefreshCw, Trash2, X } from 'lucide-react';
import type { LlmLogDetail, LlmLogEntry } from '../../types';
import { listLlmLogs, getLlmLog, deleteLlmLog } from '../../api';
import { useI18n } from '../../i18n';
import { AlertBanner, Button, IconButton } from '../../components/ui';

interface LlmLogsModalProps {
  onClose: () => void;
}

export function LlmLogsModal({ onClose }: LlmLogsModalProps) {
  const { t } = useI18n();
  const [logs, setLogs] = useState<LlmLogEntry[]>([]);
  const [selectedLog, setSelectedLog] = useState<LlmLogDetail | null>(null);
  const [isLoading, setIsLoading] = useState(true);
  const [error, setError] = useState('');
  const [activeTab, setActiveTab] = useState<'request' | 'response'>('request');
  const [copied, setCopied] = useState(false);

  const loadLogs = useCallback(async () => {
    setIsLoading(true);
    setError('');
    try {
      const list = await listLlmLogs();
      setLogs(list);
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : 'Failed to load logs');
    } finally {
      setIsLoading(false);
    }
  }, []);

  useEffect(() => {
    void loadLogs();
  }, [loadLogs]);

  async function handleViewLog(logId: string) {
    setError('');
    try {
      const detail = await getLlmLog(logId);
      setSelectedLog(detail);
      setActiveTab('request');
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : 'Failed to load log detail');
    }
  }

  async function handleDeleteLog(logId: string) {
    setError('');
    try {
      await deleteLlmLog(logId);
      setLogs((current) => current.filter((log) => log.id !== logId));
      if (selectedLog?.id === logId) {
        setSelectedLog(null);
      }
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : 'Failed to delete log');
    }
  }

  function formatRaw(raw: string | undefined): string {
    if (!raw) return '';
    try {
      return JSON.stringify(JSON.parse(raw), null, 2);
    } catch {
      return raw;
    }
  }

  function handleCopy() {
    const text = activeTab === 'request'
      ? formatRaw(selectedLog?.raw_request)
      : (selectedLog?.raw_response ? formatRaw(selectedLog.raw_response) : selectedLog?.error || '');
    if (!text) return;
    navigator.clipboard.writeText(text).then(() => {
      setCopied(true);
      setTimeout(() => setCopied(false), 2000);
    }).catch(() => {});
  }

  return (
    <div className="modal-overlay llm-logs-overlay" onClick={onClose}>
      <div className="modal-container llm-logs-modal llm-logs-modal-large" onClick={(e) => e.stopPropagation()}>
        <div className="modal-header llm-logs-modal-header">
          <div className="modal-header-content">
            <h2>{t('llmLogs.title')}</h2>
            <p className="muted modal-description">{t('llmLogs.description')}</p>
          </div>
          <div className="modal-header-actions">
            <Button variant="ghost" type="button" onClick={loadLogs} icon={<RefreshCw size={16} />}>
              {t('llmLogs.refresh')}
            </Button>
            <IconButton onClick={onClose} label={t('llmLogs.close')}>
              <X size={20} />
            </IconButton>
          </div>
        </div>

        {error ? <AlertBanner message={error} onDismiss={() => setError('')} dismissLabel={t('error.dismiss')} /> : null}

        <div className="modal-body">
          <div className="llm-logs-layout">
            <aside className="llm-logs-list">
              {isLoading ? (
                <div className="llm-logs-empty-state">
                  <p className="muted">{t('sidebar.loading')}</p>
                </div>
              ) : logs.length === 0 ? (
                <div className="llm-logs-empty-state">
                  <FileText size={32} className="empty-icon" />
                  <p className="muted">{t('llmLogs.empty')}</p>
                  <p className="hint-small">{t('settings.llmLogHint')}</p>
                </div>
              ) : (
                <div className="list-stack flush">
                  {logs.map((log) => (
                    <div
                      key={log.id}
                      className={`list-item llm-log-item${selectedLog?.id === log.id ? ' is-active' : ''}`}
                      onClick={() => handleViewLog(log.id)}
                    >
                      <span className="list-item-leading"><FileText size={16} /></span>
                      <div className="list-item-body">
                        <span className="list-item-title">{log.agent_name}</span>
                        <span className="list-item-subtitle">{log.model_id} · {log.provider_name}</span>
                        <span className="list-item-time">{new Date(log.created_at).toLocaleString()}</span>
                      </div>
                      <span className="list-item-trailing">
                        <IconButton
                          variant="danger"
                          onClick={(e) => { e.stopPropagation(); handleDeleteLog(log.id); }}
                          label={t('llmLogs.delete')}
                        >
                          <Trash2 size={14} />
                        </IconButton>
                      </span>
                    </div>
                  ))}
                </div>
              )}
            </aside>

            <div className="llm-logs-detail">
              {selectedLog ? (
                <>
                  <div className="llm-logs-meta">
                    <div className="meta-row">
                      <span className="meta-label">{t('llmLogs.agent')}</span>
                      <span>{selectedLog.agent_name}</span>
                    </div>
                    <div className="meta-row">
                      <span className="meta-label">{t('llmLogs.model')}</span>
                      <span>{selectedLog.model_id}</span>
                    </div>
                    <div className="meta-row">
                      <span className="meta-label">{t('llmLogs.provider')}</span>
                      <span>{selectedLog.provider_name}</span>
                    </div>
                    <div className="meta-row">
                      <span className="meta-label">{t('llmLogs.time')}</span>
                      <span>{new Date(selectedLog.created_at).toLocaleString()}</span>
                    </div>
                    {selectedLog.raw_status !== undefined ? (
                      <div className="meta-row">
                        <span className="meta-label">{t('llmLogs.status')}</span>
                        <span className={selectedLog.raw_status >= 400 ? 'text-error' : 'text-success'}>{selectedLog.raw_status}</span>
                      </div>
                    ) : null}
                  </div>

                  {selectedLog.error ? (
                    <div className="alert-banner error">
                      <strong>{t('llmLogs.error')}:</strong> {selectedLog.error}
                    </div>
                  ) : null}

                  <div className="llm-logs-tabs">
                    <button
                      className={`tab-btn ${activeTab === 'request' ? 'active' : ''}`}
                      onClick={() => setActiveTab('request')}
                    >
                      {t('llmLogs.request')}
                    </button>
                    <button
                      className={`tab-btn ${activeTab === 'response' ? 'active' : ''}`}
                      onClick={() => setActiveTab('response')}
                    >
                      {t('llmLogs.response')}
                    </button>
                    <div className="tab-spacer" />
                    <Button variant="ghost" type="button" onClick={handleCopy} icon={<Copy size={14} />}>
                      {copied ? t('chat.copied') : t('chat.copy')}
                    </Button>
                  </div>

                  <pre className="llm-logs-content">
                    {activeTab === 'request'
                      ? formatRaw(selectedLog.raw_request)
                      : (selectedLog.raw_response
                          ? formatRaw(selectedLog.raw_response)
                          : (selectedLog.error || '(empty)'))}
                  </pre>
                </>
              ) : (
                <p className="muted" style={{ alignSelf: 'center', margin: 'auto' }}>
                  {logs.length > 0 ? t('agentConfig.selectAgent') : ''}
                </p>
              )}
            </div>
          </div>
        </div>
      </div>
    </div>
  );
}
