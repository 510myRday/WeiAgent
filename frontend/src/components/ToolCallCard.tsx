import { useEffect, useState } from 'react';
import { Terminal, CheckCircle, XCircle, AlertTriangle, Loader2, Square, RefreshCw, Box, Monitor } from 'lucide-react';

import { getToolProcess, stopToolProcess, type ToolProcessInfo } from '../api';
import type { ToolCallStatus, ToolResultData } from '../types';
import { useI18n } from '../i18n';

interface ToolCallCardProps {
  callId: string;
  name: string;
  arguments: Record<string, any>;
  status: ToolCallStatus;
  result?: ToolResultData;
  onApprove?: (decision: 'approve' | 'reject' | 'always_approve') => void;
}

function normalizeRuntime(runtime?: string | null): {
  key: 'docker' | 'native' | 'fallback' | 'unknown';
  labelKey: string;
  className: string;
} {
  const value = (runtime || '').toLowerCase();
  if (!value) {
    return { key: 'unknown', labelKey: 'toolCard.runtimeUnknown', className: 'tool-runtime-badge--unknown' };
  }
  if (value.includes('fallback')) {
    return { key: 'fallback', labelKey: 'toolCard.runtimeFallback', className: 'tool-runtime-badge--fallback' };
  }
  if (value.startsWith('docker')) {
    return { key: 'docker', labelKey: 'toolCard.runtimeDocker', className: 'tool-runtime-badge--docker' };
  }
  if (value.includes('native')) {
    return { key: 'native', labelKey: 'toolCard.runtimeNative', className: 'tool-runtime-badge--native' };
  }
  return { key: 'unknown', labelKey: 'toolCard.runtimeUnknown', className: 'tool-runtime-badge--unknown' };
}

function RuntimeBadge({ runtime, t }: { runtime?: string | null; t: (path: string, vars?: Record<string, string | number>) => string }) {
  if (!runtime) return null;
  const info = normalizeRuntime(runtime);
  const Icon = info.key === 'docker' ? Box : Monitor;
  return (
    <span className={`tool-runtime-badge ${info.className}`} title={runtime}>
      <Icon size={12} />
      {t(info.labelKey)}
    </span>
  );
}

export function ToolCallCard({ name, arguments: args, status, result, onApprove }: ToolCallCardProps) {
  const { t } = useI18n();
  const command = typeof args.command === 'string' ? args.command : name;
  const processId = result?.process_id;
  const runtime = result?.runtime;
  const [process, setProcess] = useState<ToolProcessInfo | null>(null);
  const [processError, setProcessError] = useState('');
  const [isStopping, setIsStopping] = useState(false);

  useEffect(() => {
    if (!processId) {
      setProcess(null);
      return;
    }
    let cancelled = false;
    let timer: number | undefined;
    const refresh = async () => {
      try {
        const next = await getToolProcess(processId);
        if (cancelled) return;
        setProcess(next);
        setProcessError('');
        if (next.status === 'running') {
          timer = window.setTimeout(refresh, 1000);
        }
      } catch (error) {
        if (!cancelled) setProcessError(error instanceof Error ? error.message : t('toolCard.refreshFailed'));
      }
    };
    void refresh();
    return () => {
      cancelled = true;
      if (timer !== undefined) window.clearTimeout(timer);
    };
  }, [processId, t]);

  async function handleStop() {
    if (!processId) return;
    setIsStopping(true);
    try {
      setProcess(await stopToolProcess(processId));
      setProcessError('');
    } catch (error) {
      setProcessError(error instanceof Error ? error.message : t('toolCard.stopFailed'));
    } finally {
      setIsStopping(false);
    }
  }

  if (status === 'pending_approval' && onApprove) {
    return (
      <div className="tool-card tool-card--approval">
        <div className="tool-card__header">
          <Terminal size={18} />
          <span>{t('toolCard.approvalTitle')}</span>
        </div>
        <pre className="tool-card__command">{command}</pre>
        <div className="tool-card__actions">
          <button type="button" onClick={() => onApprove('approve')} className="btn-approve">{t('toolCard.allow')}</button>
          <button type="button" onClick={() => onApprove('always_approve')} className="btn-always">{t('toolCard.alwaysAllow')}</button>
          <button type="button" onClick={() => onApprove('reject')} className="btn-reject">{t('toolCard.reject')}</button>
        </div>
      </div>
    );
  }

  if (processId) {
    const effectiveStatus = process?.status ?? 'running';
    const running = effectiveStatus === 'running';
    return (
      <div className={`tool-card ${running ? 'tool-card--running' : effectiveStatus === 'completed' ? 'tool-card--done' : 'tool-card--error'}`}>
        <div className="tool-card__header">
          {running ? <Loader2 size={18} className="animate-spin" /> : effectiveStatus === 'completed' ? <CheckCircle size={18} /> : <AlertTriangle size={18} />}
          <span>{running ? t('toolCard.backgroundRunning') : t(`toolCard.processStatus.${effectiveStatus}`)}</span>
          <RuntimeBadge runtime={runtime} t={t} />
          {running ? (
            <button type="button" className="tool-process-action" onClick={handleStop} disabled={isStopping}>
              <Square size={14} /> {isStopping ? t('toolCard.stopping') : t('toolCard.stop')}
            </button>
          ) : null}
        </div>
        <pre className="tool-card__command">{command}</pre>
        {process?.stdout ? <pre className="tool-card__output">{process.stdout}</pre> : null}
        {process?.stderr ? <pre className="tool-card__error">{process.stderr}</pre> : null}
        {process ? <p className="tool-card__reason">{t('toolCard.duration', { ms: process.duration_ms })}</p> : null}
        {processError ? <p className="tool-card__reason"><RefreshCw size={13} /> {processError}</p> : null}
      </div>
    );
  }

  if (status === 'running') {
    return (
      <div className="tool-card tool-card--running">
        <div className="tool-card__header">
          <Loader2 size={18} className="animate-spin" />
          <span>{t('toolCard.executing', { command })}</span>
          <RuntimeBadge runtime={runtime} t={t} />
        </div>
      </div>
    );
  }

  if (status === 'completed' && result) {
    return (
      <div className="tool-card tool-card--done">
        <div className="tool-card__header">
          <CheckCircle size={18} />
          <span>{t('toolCard.completed', { code: result.exit_code ?? 0 })}</span>
          <RuntimeBadge runtime={result.runtime} t={t} />
        </div>
        {result.stdout ? <pre className="tool-card__output">{result.stdout}</pre> : null}
        {result.stderr ? <pre className="tool-card__error">{result.stderr}</pre> : null}
      </div>
    );
  }

  if (status === 'blocked' || status === 'rejected') {
    return (
      <div className="tool-card tool-card--blocked">
        <XCircle size={18} />
        <span>{status === 'blocked' ? t('toolCard.blocked') : t('toolCard.rejected')}</span>
        {result?.reason ? <p className="tool-card__reason">{result.reason}</p> : null}
      </div>
    );
  }

  if (status === 'error') {
    return (
      <div className="tool-card tool-card--error">
        <AlertTriangle size={18} />
        <span>{t('toolCard.executionError')}</span>
        {result?.message ? <p className="tool-card__reason">{result.message}</p> : null}
      </div>
    );
  }

  return null;
}
