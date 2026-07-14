import { useEffect, useLayoutEffect, useRef, useState } from 'react';
import {
  AlertTriangle,
  Check,
  CheckCircle,
  Clock3,
  Copy,
  Hand,
  Loader2,
  Maximize2,
  RefreshCw,
  ShieldAlert,
  Square,
  Terminal,
  X,
  XCircle,
} from 'lucide-react';

import { getToolProcess, stopToolProcess, type ToolProcessInfo } from '../api';
import type { ToolCallStatus, ToolResultData } from '../types';
import { useI18n } from '../i18n';
import { Dialog, IconButton } from './ui';

interface ToolCallCardProps {
  agentId: string;
  callId: string;
  name: string;
  arguments: Record<string, any>;
  status: ToolCallStatus;
  result?: ToolResultData;
  permissionMode?: 'request_approval' | 'full_access' | null;
  onApprove?: (decision: 'approve' | 'reject' | 'always_approve') => Promise<void> | void;
}

function hasText(value?: string | null): boolean {
  return Boolean(value && value.trim().length > 0);
}

function PermissionModeBadge({
  permissionMode,
  t,
}: {
  permissionMode?: 'request_approval' | 'full_access' | null;
  t: (path: string, vars?: Record<string, string | number>) => string;
}) {
  if (!permissionMode) return null;
  const isFullAccess = permissionMode === 'full_access';
  const Icon = isFullAccess ? ShieldAlert : Hand;
  const label = t(isFullAccess ? 'toolCard.permissionModeFullAccess' : 'toolCard.permissionModeRequestApproval');
  return (
    <span className={`tool-card__permission-badge ${isFullAccess ? 'tool-card__permission-badge--full-access' : 'tool-card__permission-badge--approval'}`} title={label}>
      <Icon size={12} />
      {label}
    </span>
  );
}

function TerminalContent({
  content,
  tone,
  t,
}: {
  content: string;
  tone: 'command' | 'stdout' | 'stderr';
  t: (path: string, vars?: Record<string, string | number>) => string;
}) {
  const previewRef = useRef<HTMLPreElement>(null);
  const [isOverflowing, setIsOverflowing] = useState(false);
  const [isExpanded, setIsExpanded] = useState(false);
  const [copied, setCopied] = useState(false);
  const isCommand = tone === 'command';
  const title = isCommand
    ? t('toolCard.commandTitle')
    : tone === 'stderr'
      ? t('toolCard.stderrTitle')
      : t('toolCard.stdoutTitle');
  const showAllLabel = t(isCommand ? 'toolCard.showAllCommand' : 'toolCard.showAllOutput');
  const copyLabel = t(isCommand ? 'toolCard.copyCommand' : 'toolCard.copyOutput');

  useLayoutEffect(() => {
    const preview = previewRef.current;
    if (!preview) return;

    const updateOverflow = () => {
      setIsOverflowing(preview.scrollHeight > preview.clientHeight + 1);
    };
    updateOverflow();

    const observer = new ResizeObserver(updateOverflow);
    observer.observe(preview);
    return () => observer.disconnect();
  }, [content]);

  async function handleCopy() {
    try {
      await navigator.clipboard.writeText(content);
      setCopied(true);
      window.setTimeout(() => setCopied(false), 1600);
    } catch {
      setCopied(false);
    }
  }

  return (
    <>
      <div className={`tool-card__output-shell tool-card__output-shell--${tone}`}>
        <pre ref={previewRef} className={`tool-card__output tool-card__output--${tone}`}>{content}</pre>
        {isOverflowing ? (
          <div className="tool-card__output-footer">
            <button type="button" onClick={() => setIsExpanded(true)} aria-label={showAllLabel}>
              <Maximize2 size={14} />
              {showAllLabel}
            </button>
          </div>
        ) : null}
      </div>
      <Dialog
        open={isExpanded}
        title={title}
        description={t(isCommand ? 'toolCard.commandModalHint' : 'toolCard.outputModalHint')}
        icon={<Terminal size={18} />}
        closeLabel={t('toolCard.closeOutput')}
        onClose={() => setIsExpanded(false)}
        size="lg"
        className={`terminal-output-modal terminal-output-modal--${tone}`}
        bodyClassName="terminal-output-modal__body"
        headerActions={(
          <IconButton variant="ghost" type="button" onClick={handleCopy} label={copyLabel}>
            {copied ? <Check size={18} /> : <Copy size={18} />}
          </IconButton>
        )}
      >
        <pre className={`terminal-output-modal__content terminal-output-modal__content--${tone}`}>{content}</pre>
      </Dialog>
    </>
  );
}

function TerminalOutputs({
  stdout,
  stderr,
  failed,
  t,
}: {
  stdout?: string | null;
  stderr?: string | null;
  failed: boolean;
  t: (path: string, vars?: Record<string, string | number>) => string;
}) {
  const hasStdout = hasText(stdout);
  const hasStderr = hasText(stderr);

  if (!hasStdout && !hasStderr) {
    return (
      <div className={`tool-card__output-shell tool-card__output-shell--empty${failed ? ' tool-card__output-shell--empty-failed' : ''}`}>
        <pre className="tool-card__output tool-card__output--empty">
          {t(failed ? 'toolCard.noOutputFailed' : 'toolCard.noOutput')}
        </pre>
      </div>
    );
  }

  return (
    <>
      {hasStdout ? <TerminalContent content={stdout!.trimEnd()} tone="stdout" t={t} /> : null}
      {hasStderr ? (
        <TerminalContent content={stderr!.trimEnd()} tone={failed ? 'stderr' : 'stdout'} t={t} />
      ) : null}
    </>
  );
}

export function ToolCallCard({ agentId, name, arguments: args, status, result, permissionMode, onApprove }: ToolCallCardProps) {
  const { t } = useI18n();
  const command = typeof args.command === 'string' ? args.command : name;
  const processId = result?.process_id;
  const [process, setProcess] = useState<ToolProcessInfo | null>(null);
  const [processError, setProcessError] = useState('');
  const [isStopping, setIsStopping] = useState(false);
  const [approvalDecision, setApprovalDecision] = useState<'approve' | 'reject' | 'always_approve' | null>(null);

  useEffect(() => {
    setApprovalDecision(null);
  }, [args.approval_id, status]);

  useEffect(() => {
    if (!processId) {
      setProcess(null);
      return;
    }
    let cancelled = false;
    let timer: number | undefined;
    const refresh = async () => {
      try {
        const next = await getToolProcess(processId, agentId);
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
  }, [agentId, processId, t]);

  async function handleStop() {
    if (!processId) return;
    setIsStopping(true);
    try {
      setProcess(await stopToolProcess(processId, agentId));
      setProcessError('');
    } catch (error) {
      setProcessError(error instanceof Error ? error.message : t('toolCard.stopFailed'));
    } finally {
      setIsStopping(false);
    }
  }

  async function handleApproval(decision: 'approve' | 'reject' | 'always_approve') {
    if (!onApprove || approvalDecision) return;
    setApprovalDecision(decision);
    try {
      await onApprove(decision);
    } catch {
      setApprovalDecision(null);
    }
  }

  if (status === 'pending_approval' && onApprove) {
    return (
      <div className="tool-card tool-card--approval">
        <div className="tool-card__header">
          <span className="tool-card__status-icon tool-card__status-icon--approval"><Terminal size={19} /></span>
          <div className="tool-card__title">
            <strong>{t('toolCard.approvalTitle')}</strong>
            <small>{t('toolCard.approvalSubtitle')}</small>
          </div>
          <PermissionModeBadge permissionMode={permissionMode} t={t} />
        </div>
        <TerminalContent content={command} tone="command" t={t} />
        <dl className="tool-card__approval-context">
          <div><dt>{t('toolCard.cwd')}</dt><dd>{String(args.cwd || t('toolCard.defaultCwd'))}</dd></div>
          <div><dt>{t('toolCard.runtime')}</dt><dd>{String(args.runtime || t('toolCard.runtimeUnknown'))}</dd></div>
          <div><dt>{t('toolCard.permission')}</dt><dd>{t('toolCard.fullAccessPermission')}</dd></div>
          <div><dt>{t('toolCard.background')}</dt><dd>{args.background ? t('toolCard.enabled') : t('toolCard.disabled')}</dd></div>
          {args.justification ? <div><dt>{t('toolCard.justification')}</dt><dd>{String(args.justification)}</dd></div> : null}
        </dl>
        <div className="tool-card__actions">
          <button type="button" onClick={() => handleApproval('approve')} className="btn-approve" disabled={approvalDecision !== null}><Check size={15} />{approvalDecision === 'approve' ? t('toolCard.approvalSubmitting') : t('toolCard.allow')}</button>
          <button type="button" onClick={() => handleApproval('always_approve')} className="btn-always" disabled={approvalDecision !== null}><Clock3 size={15} />{approvalDecision === 'always_approve' ? t('toolCard.approvalSubmitting') : t('toolCard.alwaysAllow')}</button>
          <button type="button" onClick={() => handleApproval('reject')} className="btn-reject" disabled={approvalDecision !== null}><X size={15} />{approvalDecision === 'reject' ? t('toolCard.approvalSubmitting') : t('toolCard.reject')}</button>
        </div>
      </div>
    );
  }

  if (processId) {
    const effectiveStatus = process?.status ?? 'running';
    const running = effectiveStatus === 'running';
    const failed = effectiveStatus === 'failed' || (process?.exit_code !== undefined && process.exit_code !== 0);
    return (
      <div className={`tool-card ${running ? 'tool-card--running' : effectiveStatus === 'completed' ? 'tool-card--done' : 'tool-card--error'}`}>
        <div className="tool-card__header">
          <span className="tool-card__status-icon">
            {running ? <Loader2 size={18} className="animate-spin" /> : effectiveStatus === 'completed' ? <CheckCircle size={18} /> : <AlertTriangle size={18} />}
          </span>
          <div className="tool-card__title">
            <strong>{running ? t('toolCard.backgroundRunning') : t(`toolCard.processStatus.${effectiveStatus}`)}</strong>
          </div>
          <PermissionModeBadge permissionMode={permissionMode} t={t} />
          {running ? (
            <button type="button" className="tool-process-action" onClick={handleStop} disabled={isStopping}>
              <Square size={14} /> {isStopping ? t('toolCard.stopping') : t('toolCard.stop')}
            </button>
          ) : null}
        </div>
        <TerminalContent content={command} tone="command" t={t} />
        {process ? (
          running && !hasText(process.stdout) && !hasText(process.stderr) ? null : (
            <TerminalOutputs
              stdout={process.stdout}
              stderr={process.stderr}
              failed={failed}
              t={t}
            />
          )
        ) : null}
        {process ? <p className="tool-card__reason">{t('toolCard.duration', { ms: process.duration_ms })}</p> : null}
        {processError ? <p className="tool-card__reason"><RefreshCw size={13} /> {processError}</p> : null}
      </div>
    );
  }

  if (status === 'running') {
    return (
      <div className="tool-card tool-card--running">
        <div className="tool-card__header">
          <span className="tool-card__status-icon"><Loader2 size={18} className="animate-spin" /></span>
          <div className="tool-card__title"><strong>{t('toolCard.executingTitle')}</strong></div>
          <PermissionModeBadge permissionMode={permissionMode} t={t} />
        </div>
        <TerminalContent content={command} tone="command" t={t} />
      </div>
    );
  }

  if (status === 'completed' && result) {
    const failed = (result.exit_code ?? 0) !== 0;
    return (
      <div className={`tool-card ${failed ? 'tool-card--error' : 'tool-card--done'}`}>
        <div className="tool-card__header">
          <span className={`tool-card__status-icon ${failed ? '' : 'tool-card__status-icon--success'}`}>
            {failed ? <AlertTriangle size={18} /> : <CheckCircle size={18} />}
          </span>
          <div className="tool-card__title"><strong>{t('toolCard.completed', { code: result.exit_code ?? 0 })}</strong></div>
          <PermissionModeBadge permissionMode={permissionMode} t={t} />
        </div>
        <TerminalContent content={command} tone="command" t={t} />
        <TerminalOutputs
          stdout={result.stdout}
          stderr={result.stderr}
          failed={failed}
          t={t}
        />
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
