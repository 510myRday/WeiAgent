import { useEffect, useLayoutEffect, useMemo, useRef, useState, type ReactNode } from 'react';
import {
  AlertTriangle,
  Check,
  CheckCircle,
  Clock3,
  Copy,
  Loader2,
  Maximize2,
  RefreshCw,
  Square,
  Terminal,
  Wrench,
  X,
  XCircle,
} from 'lucide-react';

import { stopToolProcess } from '../api';
import type { ToolArguments, ToolCallStatus, ToolResultData } from '../types';
import { useI18n } from '../i18n';
import { updateToolProcessCache, useClipboard, useToolProcess } from '../hooks';
import { Dialog, IconButton } from './ui';

interface ToolCallCardProps {
  agentId: string;
  callId: string;
  name: string;
  arguments: ToolArguments;
  argumentsText?: string;
  phase?: 'generating_arguments' | 'executing';
  status: ToolCallStatus;
  result?: ToolResultData;
  /** Hide the status/title header when nested under a disclosure summary. */
  hideHeader?: boolean;
  /** Render the running-process action in the parent disclosure summary instead. */
  hideProcessActions?: boolean;
  onApprove?: (decision: 'approve' | 'reject' | 'always_approve') => Promise<void> | void;
}

function hasText(value?: string | null): boolean {
  return Boolean(value && value.trim().length > 0);
}

function isTerminalTool(name: string): boolean {
  return (
    name === 'execute_terminal' ||
    name === 'terminal_session'
  );
}

function formatToolArguments(args: ToolArguments): string | null {
  const display: Record<string, unknown> = {};
  for (const [key, value] of Object.entries(args)) {
    if (
      key === 'approval_id' ||
      key === 'agent_id' ||
      key === 'runtime' ||
      key === 'permission' ||
      value === undefined ||
      value === null ||
      value === ''
    ) {
      continue;
    }
    display[key] = value;
  }
  if (Object.keys(display).length === 0) return null;
  try {
    return JSON.stringify(display, null, 2);
  } catch {
    return String(display);
  }
}

export function formatToolOutput(content: string): string {
  const trimmed = content.trim();
  if (!trimmed) return content;

  const formatted = prettyJsonValue(trimmed);
  if (formatted) return formatted;

  const lines = trimmed.split(/\r?\n/);
  if (lines.length <= 1) return content;
  const formattedLines = lines.map((line) => {
    if (!line.trim()) return '';
    return prettyJsonValue(line.trim());
  });
  if (formattedLines.some((line) => line === null)) return content;
  return formattedLines.join('\n');
}

function prettyJsonValue(value: string): string | null {
  try {
    let parsed: unknown = JSON.parse(value);
    if (
      typeof parsed === 'string'
      && (parsed.trim().startsWith('{') || parsed.trim().startsWith('['))
    ) {
      parsed = JSON.parse(parsed);
    }
    if (parsed === null || typeof parsed !== 'object') return null;
    return JSON.stringify(parsed, null, 2);
  } catch {
    return null;
  }
}

function ToolNameBadge({ name }: { name: string }) {
  return (
    <span className="tool-card__tool-badge" title={name}>
      <Wrench size={12} aria-hidden />
      <code>{name}</code>
    </span>
  );
}

function HeaderMeta({
  name,
  children,
}: {
  name: string;
  children?: ReactNode;
}) {
  return (
    <div className="tool-card__meta">
      <ToolNameBadge name={name} />
      {children}
    </div>
  );
}

function TerminalContent({
  content,
  tone,
  t,
}: {
  content: string;
  tone: 'command' | 'stdout' | 'stderr' | 'args';
  t: (path: string, vars?: Record<string, string | number>) => string;
}) {
  const previewRef = useRef<HTMLPreElement>(null);
  const [isOverflowing, setIsOverflowing] = useState(false);
  const [isExpanded, setIsExpanded] = useState(false);
  const { copied, copy } = useClipboard();
  const isCommand = tone === 'command';
  const isArgs = tone === 'args';
  const displayContent = useMemo(
    () => (isCommand ? content : formatToolOutput(content)),
    [content, isCommand],
  );
  const title = isCommand
    ? t('toolCard.commandTitle')
    : isArgs
      ? t('toolCard.argumentsTitle')
      : tone === 'stderr'
        ? t('toolCard.stderrTitle')
        : t('toolCard.stdoutTitle');
  const showAllLabel = t(isCommand || isArgs ? 'toolCard.showAllCommand' : 'toolCard.showAllOutput');
  const copyLabel = t(isCommand || isArgs ? 'toolCard.copyCommand' : 'toolCard.copyOutput');

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
  }, [displayContent]);

  const shellTone = tone === 'args' ? 'command' : tone;

  return (
    <>
      <div className={`tool-card__output-shell tool-card__output-shell--${shellTone}`}>
        <pre ref={previewRef} className={`tool-card__output tool-card__output--${shellTone}`}>{displayContent}</pre>
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
        description={t(isCommand || isArgs ? 'toolCard.commandModalHint' : 'toolCard.outputModalHint')}
        icon={<Terminal size={18} />}
        closeLabel={t('toolCard.closeOutput')}
        onClose={() => setIsExpanded(false)}
        size="lg"
        className={`terminal-output-modal terminal-output-modal--${shellTone}`}
        bodyClassName="terminal-output-modal__body"
        headerActions={(
          <IconButton variant="ghost" type="button" onClick={() => void copy(displayContent)} label={copyLabel}>
            {copied ? <Check size={18} /> : <Copy size={18} />}
          </IconButton>
        )}
      >
        <pre className={`terminal-output-modal__content terminal-output-modal__content--${shellTone}`}>{displayContent}</pre>
      </Dialog>
    </>
  );
}

function ToolPayload({
  name,
  args,
  argumentsText,
  phase,
  t,
}: {
  name: string;
  args: ToolArguments;
  argumentsText?: string;
  phase?: 'generating_arguments' | 'executing';
  t: (path: string, vars?: Record<string, string | number>) => string;
}) {
  if (phase === 'generating_arguments' && hasText(argumentsText)) {
    return <TerminalContent content={argumentsText!} tone="args" t={t} />;
  }
  if (typeof args.command === 'string' && args.command.trim()) {
    return <TerminalContent content={args.command} tone="command" t={t} />;
  }
  const formatted = formatToolArguments(args);
  if (!formatted) return null;
  // For non-execute tools, show structured args so the bubble is still useful.
  void name;
  return <TerminalContent content={formatted} tone="args" t={t} />;
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

export function ToolCallCard({
  agentId,
  name,
  arguments: args,
  argumentsText,
  phase,
  status,
  result,
  hideHeader = false,
  hideProcessActions = false,
  onApprove,
}: ToolCallCardProps) {
  const { t } = useI18n();
  const processId = result?.process_id;
  const processSnapshot = useToolProcess(processId, agentId);
  const process = processSnapshot.process;
  const [processActionError, setProcessActionError] = useState('');
  const [isStopping, setIsStopping] = useState(false);
  const [approvalDecision, setApprovalDecision] = useState<'approve' | 'reject' | 'always_approve' | null>(null);

  useEffect(() => {
    setApprovalDecision(null);
  }, [args.approval_id, status]);

  useEffect(() => {
    setProcessActionError('');
  }, [processId]);

  const processError = processActionError
    || (processSnapshot.notFound
      ? t('toolCard.processUnavailable')
      : processSnapshot.error || '');

  async function handleStop() {
    if (!processId) return;
    setIsStopping(true);
    try {
      const stopped = await stopToolProcess(processId, agentId);
      updateToolProcessCache(stopped);
      setProcessActionError('');
    } catch (error) {
      const message = error instanceof Error ? error.message : '';
      if (/not found/i.test(message)) {
        setProcessActionError(t('toolCard.processUnavailable'));
      } else {
        setProcessActionError(message || t('toolCard.stopFailed'));
      }
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
      <div className={`tool-card tool-card--approval${hideHeader ? ' tool-card--embedded' : ''}`}>
        {hideHeader ? null : (
          <div className="tool-card__header">
            <span className="tool-card__status-icon tool-card__status-icon--approval"><Terminal size={19} /></span>
            <div className="tool-card__title">
              <strong>{t('toolCard.approvalTitle')}</strong>
              <small>{t('toolCard.approvalSubtitle')}</small>
            </div>
            <HeaderMeta name={name} />
          </div>
        )}
        <ToolPayload name={name} args={args} argumentsText={argumentsText} phase={phase} t={t} />
        <dl className="tool-card__approval-context">
          <div><dt>{t('toolCard.cwd')}</dt><dd>{String(args.cwd || t('toolCard.defaultCwd'))}</dd></div>
          <div><dt>{t('toolCard.runtime')}</dt><dd>{String(args.runtime || t('toolCard.runtimeUnknown'))}</dd></div>
          <div><dt>{t('toolCard.permission')}</dt><dd>{t('toolCard.fullAccessPermission')}</dd></div>
          <div><dt>{t('toolCard.background')}</dt><dd>{args.background ? t('toolCard.enabled') : t('toolCard.disabled')}</dd></div>
          {args.justification ? <div><dt>{t('toolCard.justification')}</dt><dd>{String(args.justification)}</dd></div> : null}
        </dl>
        {hideHeader ? null : (
          <div className="tool-card__actions">
            <button type="button" onClick={() => handleApproval('approve')} className="btn-approve" disabled={approvalDecision !== null}><Check size={15} />{approvalDecision === 'approve' ? t('toolCard.approvalSubmitting') : t('toolCard.allow')}</button>
            <button type="button" onClick={() => handleApproval('always_approve')} className="btn-always" disabled={approvalDecision !== null}><Clock3 size={15} />{approvalDecision === 'always_approve' ? t('toolCard.approvalSubmitting') : t('toolCard.alwaysAllow')}</button>
            <button type="button" onClick={() => handleApproval('reject')} className="btn-reject" disabled={approvalDecision !== null}><X size={15} />{approvalDecision === 'reject' ? t('toolCard.approvalSubmitting') : t('toolCard.reject')}</button>
          </div>
        )}
      </div>
    );
  }

  if (processId) {
    // Only treat as running after the process manager confirms it. After restart the
    // process_id remains in chat history but the in-memory process is gone.
    const missing = !process && Boolean(processError);
    const loading = processSnapshot.loading || (!process && !processError);
    const effectiveStatus = process?.status ?? (missing ? 'not_found' : 'running');
    const running = process?.status === 'running';
    const failed = effectiveStatus === 'failed' || (process?.exit_code !== undefined && process.exit_code !== 0);
    const title = missing
      ? t('toolCard.processStatus.notFound')
      : loading
        ? t('toolCard.processStatus.checking')
        : running
        ? t('toolCard.backgroundRunning')
        : t(`toolCard.processStatus.${effectiveStatus}`);
    const cardTone = running || loading
      ? 'tool-card--running'
      : effectiveStatus === 'completed'
        ? 'tool-card--done'
        : 'tool-card--error';
    return (
      <div className={`tool-card ${cardTone}${hideHeader ? ' tool-card--embedded' : ''}`}>
        {hideHeader ? (
          running && !hideProcessActions ? (
            <div className="tool-card__embedded-actions">
              <button type="button" className="tool-process-action" onClick={handleStop} disabled={isStopping}>
                <Square size={14} /> {isStopping ? t('toolCard.stopping') : t('toolCard.stop')}
              </button>
            </div>
          ) : missing ? (
            <p className="tool-card__reason" title={processError || title}>
              {t('toolCard.processStatus.notFoundBadge')}
            </p>
          ) : null
        ) : (
          <div className="tool-card__header">
            <span className="tool-card__status-icon">
              {running || loading
                ? <Loader2 size={18} className="animate-spin" />
                : effectiveStatus === 'completed'
                  ? <CheckCircle size={18} />
                  : <AlertTriangle size={18} />}
            </span>
            <div className="tool-card__title">
              <strong>{title}</strong>
            </div>
            <HeaderMeta name={name}>
              {running ? (
                <button type="button" className="tool-process-action" onClick={handleStop} disabled={isStopping}>
                  <Square size={14} /> {isStopping ? t('toolCard.stopping') : t('toolCard.stop')}
                </button>
              ) : missing ? (
                <span className="tool-card__status-badge tool-card__status-badge--missing" title={processError || title}>
                  {t('toolCard.processStatus.notFoundBadge')}
                </span>
              ) : null}
            </HeaderMeta>
          </div>
        )}
        <ToolPayload name={name} args={args} argumentsText={argumentsText} phase={phase} t={t} />
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
      <div className={`tool-card tool-card--running${hideHeader ? ' tool-card--embedded' : ''}`}>
        {hideHeader ? null : (
          <div className="tool-card__header">
            <span className="tool-card__status-icon"><Loader2 size={18} className="animate-spin" /></span>
            <div className="tool-card__title">
              <strong>{isTerminalTool(name) ? t('toolCard.executingTitle') : t('toolCard.executingGeneric')}</strong>
            </div>
            <HeaderMeta name={name} />
          </div>
        )}
        <ToolPayload name={name} args={args} argumentsText={argumentsText} phase={phase} t={t} />
      </div>
    );
  }

  if (status === 'completed' && result) {
    const failed = (result.exit_code ?? 0) !== 0;
    const title = result.exit_code !== undefined && result.exit_code !== null
      ? t('toolCard.completed', { code: result.exit_code })
      : t('toolCard.completedGeneric');
    return (
      <div className={`tool-card ${failed ? 'tool-card--error' : 'tool-card--done'}${hideHeader ? ' tool-card--embedded' : ''}`}>
        {hideHeader ? null : (
          <div className="tool-card__header">
            <span className={`tool-card__status-icon ${failed ? '' : 'tool-card__status-icon--success'}`}>
              {failed ? <AlertTriangle size={18} /> : <CheckCircle size={18} />}
            </span>
            <div className="tool-card__title"><strong>{title}</strong></div>
            <HeaderMeta name={name} />
          </div>
        )}
        <ToolPayload name={name} args={args} argumentsText={argumentsText} phase={phase} t={t} />
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
      <div className={`tool-card tool-card--blocked${hideHeader ? ' tool-card--embedded' : ''}`}>
        {hideHeader ? null : (
          <div className="tool-card__header">
            <span className="tool-card__status-icon"><XCircle size={18} /></span>
            <div className="tool-card__title">
              <strong>{status === 'blocked' ? t('toolCard.blocked') : t('toolCard.rejected')}</strong>
            </div>
            <HeaderMeta name={name} />
          </div>
        )}
        <ToolPayload name={name} args={args} argumentsText={argumentsText} phase={phase} t={t} />
        {result?.reason ? <p className="tool-card__reason">{result.reason}</p> : null}
      </div>
    );
  }

  if (status === 'error') {
    return (
      <div className={`tool-card tool-card--error${hideHeader ? ' tool-card--embedded' : ''}`}>
        {hideHeader ? null : (
          <div className="tool-card__header">
            <span className="tool-card__status-icon"><AlertTriangle size={18} /></span>
            <div className="tool-card__title"><strong>{t('toolCard.executionError')}</strong></div>
            <HeaderMeta name={name} />
          </div>
        )}
        <ToolPayload name={name} args={args} argumentsText={argumentsText} phase={phase} t={t} />
        {result?.message ? <p className="tool-card__reason">{result.message}</p> : null}
      </div>
    );
  }

  return null;
}

