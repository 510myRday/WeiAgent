import { useLayoutEffect, useMemo, useRef, useState } from 'react';
import {
  Check,
  Copy,
  Maximize2,
  RefreshCw,
  Terminal,
} from 'lucide-react';

import type { ToolArguments, ToolCallStatus, ToolResultData } from '../types';
import { useI18n } from '../i18n';
import { useClipboard, useToolProcess } from '../hooks';
import { Dialog, IconButton } from './ui';

/**
 * Embedded body of a tool call, always rendered inside ToolDisclosure which
 * owns the header, approval buttons, and process stop actions.
 */
interface ToolCallCardProps {
  agentId: string;
  arguments: ToolArguments;
  argumentsText?: string;
  phase?: 'generating_arguments' | 'executing';
  status: ToolCallStatus;
  result?: ToolResultData;
  onApprove?: (decision: 'approve' | 'reject' | 'always_approve') => Promise<void> | void;
}

function hasText(value?: string | null): boolean {
  return Boolean(value && value.trim().length > 0);
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
  args,
  argumentsText,
  phase,
  t,
}: {
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
  // For non-execute tools, show structured args so the bubble is still useful.
  const formatted = formatToolArguments(args);
  if (!formatted) return null;
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
  arguments: args,
  argumentsText,
  phase,
  status,
  result,
  onApprove,
}: ToolCallCardProps) {
  const { t } = useI18n();
  const processId = result?.process_id;
  const processSnapshot = useToolProcess(processId, agentId);
  const process = processSnapshot.process;

  const processError = processSnapshot.notFound
    ? t('toolCard.processUnavailable')
    : processSnapshot.error || '';

  if (status === 'pending_approval' && onApprove) {
    return (
      <div className="tool-card tool-card--approval tool-card--embedded">
        <ToolPayload args={args} argumentsText={argumentsText} phase={phase} t={t} />
        <dl className="tool-card__approval-context">
          <div><dt>{t('toolCard.cwd')}</dt><dd>{String(args.cwd || t('toolCard.defaultCwd'))}</dd></div>
          <div><dt>{t('toolCard.runtime')}</dt><dd>{String(args.runtime || t('toolCard.runtimeUnknown'))}</dd></div>
          <div><dt>{t('toolCard.permission')}</dt><dd>{t('toolCard.fullAccessPermission')}</dd></div>
          <div><dt>{t('toolCard.background')}</dt><dd>{args.background ? t('toolCard.enabled') : t('toolCard.disabled')}</dd></div>
          {args.justification ? <div><dt>{t('toolCard.justification')}</dt><dd>{String(args.justification)}</dd></div> : null}
        </dl>
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
      <div className={`tool-card ${cardTone} tool-card--embedded`}>
        {missing ? (
          <p className="tool-card__reason" title={processError || title}>
            {t('toolCard.processStatus.notFoundBadge')}
          </p>
        ) : null}
        <ToolPayload args={args} argumentsText={argumentsText} phase={phase} t={t} />
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
      <div className="tool-card tool-card--running tool-card--embedded">
        <ToolPayload args={args} argumentsText={argumentsText} phase={phase} t={t} />
      </div>
    );
  }

  if (status === 'completed' && result) {
    const failed = (result.exit_code ?? 0) !== 0;
    return (
      <div className={`tool-card ${failed ? 'tool-card--error' : 'tool-card--done'} tool-card--embedded`}>
        <ToolPayload args={args} argumentsText={argumentsText} phase={phase} t={t} />
        <TerminalOutputs
          stdout={result.stdout}
          stderr={result.stderr}
          failed={failed}
          t={t}
        />
      </div>
    );
  }

  if (status === 'blocked' || status === 'rejected' || status === 'interrupted') {
    return (
      <div className="tool-card tool-card--blocked tool-card--embedded">
        <ToolPayload args={args} argumentsText={argumentsText} phase={phase} t={t} />
        {result?.reason ? <p className="tool-card__reason">{result.reason}</p> : null}
      </div>
    );
  }

  if (status === 'error') {
    return (
      <div className="tool-card tool-card--error tool-card--embedded">
        <ToolPayload args={args} argumentsText={argumentsText} phase={phase} t={t} />
        {result?.message ? <p className="tool-card__reason">{result.message}</p> : null}
      </div>
    );
  }

  return null;
}
