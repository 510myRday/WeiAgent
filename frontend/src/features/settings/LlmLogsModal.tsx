import { useState, useEffect, useCallback } from 'react';
import { Copy, FileText, RefreshCw, Trash2 } from 'lucide-react';
import type { LlmLogDetail, LlmLogEntry } from '../../types';
import { listLlmLogs, getLlmLog, deleteLlmLog } from '../../api';
import { useI18n } from '../../i18n';
import { useClipboard } from '../../hooks';
import { Button, Dialog, IconButton, useFeedback } from '../../components/ui';

interface LlmLogsModalProps {
  onClose: () => void;
}

export function LlmLogsModal({ onClose }: LlmLogsModalProps) {
  const { t } = useI18n();
  const { confirm, toast } = useFeedback();
  const { copied, copy } = useClipboard(2000);
  const [logs, setLogs] = useState<LlmLogEntry[]>([]);
  const [selectedLog, setSelectedLog] = useState<LlmLogDetail | null>(null);
  const [isLoading, setIsLoading] = useState(true);
  const [activeTab, setActiveTab] = useState<'request' | 'response'>('request');

  const loadLogs = useCallback(async () => {
    setIsLoading(true);
    try {
      const list = await listLlmLogs();
      setLogs(list);
    } catch (cause) {
      toast.error(cause instanceof Error ? cause.message : t('error.unknown'));
    } finally {
      setIsLoading(false);
    }
  }, [t, toast]);

  useEffect(() => {
    void loadLogs();
  }, [loadLogs]);

  async function handleViewLog(logId: string) {
    try {
      const detail = await getLlmLog(logId);
      setSelectedLog(detail);
      setActiveTab('request');
    } catch (cause) {
      toast.error(cause instanceof Error ? cause.message : t('error.unknown'));
    }
  }

  async function handleDeleteLog(logId: string) {
    const confirmed = await confirm({
      title: t('llmLogs.delete'),
      message: t('llmLogs.deleteConfirm'),
      confirmLabel: t('common.delete'),
      tone: 'danger',
    });
    if (!confirmed) return;
    try {
      await deleteLlmLog(logId);
      setLogs((current) => current.filter((log) => log.id !== logId));
      if (selectedLog?.id === logId) {
        setSelectedLog(null);
      }
      toast.success(t('notification.logDeleted'));
    } catch (cause) {
      toast.error(cause instanceof Error ? cause.message : t('error.unknown'));
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
    void copy(text);
  }

  return (
    <Dialog
      open
      title={t('llmLogs.title')}
      description={t('llmLogs.description')}
      closeLabel={t('llmLogs.close')}
      onClose={onClose}
      size="xl"
      className="llm-logs-modal"
      bodyClassName="llm-logs-modal-body"
      headerActions={(
        <Button variant="ghost" type="button" onClick={loadLogs} icon={<RefreshCw size={16} />}>
          {t('llmLogs.refresh')}
        </Button>
      )}
    >
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
                      className={`llm-log-item-row${selectedLog?.id === log.id ? ' is-active' : ''}`}
                    >
                      <button className="list-item llm-log-item" type="button" onClick={() => handleViewLog(log.id)}>
                        <span className="list-item-leading"><FileText size={16} /></span>
                        <span className="list-item-body">
                          <span className="list-item-title">{log.agent_name}</span>
                          <span className="list-item-subtitle">{log.model_id} · {log.provider_name}</span>
                          <span className="list-item-time">{new Date(log.created_at).toLocaleString()}</span>
                        </span>
                      </button>
                      <span className="list-item-trailing">
                        <IconButton
                          variant="danger"
                          onClick={(e) => { e.stopPropagation(); void handleDeleteLog(log.id); }}
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
                    <div className="inline-error">
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
                <p className="muted llm-logs-select-hint">
                  {logs.length > 0 ? t('llmLogs.selectHint') : ''}
                </p>
              )}
            </div>
          </div>
    </Dialog>
  );
}
