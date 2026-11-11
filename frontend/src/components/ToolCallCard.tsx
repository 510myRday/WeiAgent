import { useLayoutEffect, useMemo, useRef, useState, type ReactNode } from 'react';
import {
  AlertTriangle,
  ArrowDownToLine,
  Check,
  Copy,
  Maximize2,
  RefreshCw,
  Terminal,
} from 'lucide-react';

import type { ToolProcessInfo } from '../toolTypes';
import type { ImageAttachment, ToolArguments, ToolCallStatus, ToolResultData } from '../types';
import { useI18n } from '../i18n';
import { useClipboard } from '../hooks';
import { AttachmentImage } from '../features/chat/AttachmentImage';
import { Dialog, IconButton } from './ui';

/**
 * Embedded body of a tool call, always rendered inside ToolDisclosure which
 * owns the header, approval buttons, and process stop actions.
 */
export type ToolProcessSnapshot = {
  process: ToolProcessInfo | null;
  error: string;
  loading: boolean;
  notFound: boolean;
};

interface ToolCallCardProps {
  agentId: string;
  toolName: string;
  arguments: ToolArguments;
  argumentsText?: string;
  phase?: 'generating_arguments' | 'executing';
  status: ToolCallStatus;
  result?: ToolResultData;
  processSnapshot?: ToolProcessSnapshot;
  onApprove?: (decision: 'approve' | 'reject' | 'always_approve') => Promise<void> | void;
}

function ToolCardSection({ className = '', children }: { className?: string; children: ReactNode }) {
  return <div className={`tool-card tool-card--embedded ${className}`.trim()}>{children}</div>;
}

function hasText(value?: string | null): boolean {
  return Boolean(value && value.trim().length > 0);
}

function formatStructuredContent(value: unknown): string | null {
  if (value === undefined || value === null) return null;
  try {
    return typeof value === 'string' ? value : JSON.stringify(value, null, 2);
  } catch {
    return String(value);
  }
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

export function formatTerminalCommand(command: string, cwd?: string | null): string {
  const path = cwd?.trim();
  return path ? `${path}> ${command}` : command;
}

export function formatToolCommand(toolName: string, command: string, cwd?: string | null): string {
  return toolName === 'execute_terminal' ? formatTerminalCommand(command, cwd) : command;
}

export function hasToolPayload(
  args: ToolArguments,
  argumentsText?: string,
  phase?: 'generating_arguments' | 'executing',
): boolean {
  if (phase === 'generating_arguments' && hasText(argumentsText)) return true;
  if (typeof args.command === 'string' && args.command.trim()) return true;
  return Boolean(formatToolArguments(args));
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
        <TerminalHeading title={title} tone={shellTone} />
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
  toolName,
  cwd,
  t,
}: {
  args: ToolArguments;
  argumentsText?: string;
  phase?: 'generating_arguments' | 'executing';
  toolName: string;
  cwd?: string | null;
  t: (path: string, vars?: Record<string, string | number>) => string;
}) {
  if (phase === 'generating_arguments' && hasText(argumentsText)) {
    return <TerminalContent content={argumentsText!} tone="args" t={t} />;
  }
  if (typeof args.command === 'string' && args.command.trim()) {
    return (
      <TerminalContent
        content={formatToolCommand(toolName, args.command, cwd)}
        tone="command"
        t={t}
      />
    );
  }
  // For non-execute tools, show structured args so the bubble is still useful.
  const formatted = formatToolArguments(args);
  if (!formatted) return null;
  return <TerminalContent content={formatted} tone="args" t={t} />;
}

function TerminalOutputs({
  stdout,
  stderr,
  structuredContent,
  attachments,
  failed,
  t,
}: {
  stdout?: string | null;
  stderr?: string | null;
  structuredContent?: unknown;
  attachments?: ImageAttachment[];
  failed: boolean;
  t: (path: string, vars?: Record<string, string | number>) => string;
}) {
  const [previewAttachment, setPreviewAttachment] = useState<ImageAttachment | null>(null);
  const hasStdout = hasText(stdout);
  const hasStderr = hasText(stderr);
  const hasAttachments = Boolean(attachments?.length);
  const structuredText = formatStructuredContent(structuredContent);
  const hasStructured = !hasAttachments && hasText(structuredText);

  if (!hasAttachments && !hasStdout && !hasStderr && !hasStructured) {
    return (
      <div className={`tool-card__output-shell tool-card__output-shell--empty${failed ? ' tool-card__output-shell--empty-failed' : ''}`}>
        <TerminalHeading
          title={t(failed ? 'toolCard.stderrTitle' : 'toolCard.stdoutTitle')}
          tone={failed ? 'stderr' : 'stdout'}
        />
        <pre className="tool-card__output tool-card__output--empty">
          {t(failed ? 'toolCard.noOutputFailed' : 'toolCard.noOutput')}
        </pre>
      </div>
    );
  }

  return (
    <>
      {hasAttachments ? (
        <div className="tool-card__output-shell tool-card__image-output-shell">
          <TerminalHeading title={t('toolCard.stdoutTitle')} tone="stdout" />
          <div className="tool-card__image-output">
            {attachments?.map((attachment) => (
              <button
                key={attachment.id}
                type="button"
                className="tool-card__image-thumbnail-button"
                onClick={() => setPreviewAttachment(attachment)}
                aria-label={attachment.name}
                title={attachment.name}
              >
                <AttachmentImage
                  className="tool-card__image-thumbnail"
                  attachment={attachment}
                  alt={attachment.name}
                />
              </button>
            ))}
          </div>
        </div>
      ) : hasStdout ? <TerminalContent content={stdout!.trimEnd()} tone="stdout" t={t} /> : null}
      {hasStderr ? (
        <TerminalContent content={stderr!.trimEnd()} tone={failed ? 'stderr' : 'stdout'} t={t} />
      ) : null}
      {hasStructured ? <TerminalContent content={structuredText!} tone="stdout" t={t} /> : null}
      <Dialog
        open={Boolean(previewAttachment)}
        title={previewAttachment?.name || t('chat.imagePreview')}
        closeLabel={t('common.close')}
        onClose={() => setPreviewAttachment(null)}
        size="xl"
        className="image-preview-dialog"
        bodyClassName="image-preview-dialog__body"
      >
        {previewAttachment ? (
          <AttachmentImage
            className="image-preview-dialog__image"
            attachment={previewAttachment}
            alt={previewAttachment.name}
          />
        ) : null}
      </Dialog>
    </>
  );
}

function TerminalHeading({
  title,
  tone,
}: {
  title: string;
  tone: 'command' | 'stdout' | 'stderr';
}) {
  const icon = tone === 'command'
    ? <Terminal size={12} aria-hidden="true" />
    : tone === 'stderr'
      ? <AlertTriangle size={12} aria-hidden="true" />
      : <ArrowDownToLine size={12} aria-hidden="true" />;
  return (
    <div className={`tool-card__output-heading tool-card__output-heading--${tone}`}>
      {icon}
      <span>{title}</span>
    </div>
  );
}

function ToolPayloadSection({
  className,
  args,
  argumentsText,
  phase,
  toolName,
  cwd,
  t,
}: {
  className: string;
  args: ToolArguments;
  argumentsText?: string;
  phase?: 'generating_arguments' | 'executing';
  toolName: string;
  cwd?: string | null;
  t: (path: string, vars?: Record<string, string | number>) => string;
}) {
  if (!hasToolPayload(args, argumentsText, phase)) return null;
  return (
    <ToolCardSection className={className}>
      <ToolPayload
        args={args}
        argumentsText={argumentsText}
        phase={phase}
        toolName={toolName}
        cwd={cwd}
        t={t}
      />
    </ToolCardSection>
  );
}

export function ToolCallCard({
  agentId: _agentId,
  toolName,
  arguments: args,
  argumentsText,
  phase,
  status,
  result,
  processSnapshot,
  onApprove,
}: ToolCallCardProps) {
  const { t } = useI18n();
  const processId = result?.process_id;
  const process = processSnapshot?.process ?? null;

  const processError = processSnapshot?.notFound
    ? t('toolCard.processUnavailable')
    : processSnapshot?.error || '';
  const commandCwd = process?.cwd ?? result?.cwd ?? (typeof args.cwd === 'string' ? args.cwd : undefined);

  if (status === 'pending_approval' && onApprove) {
    return (
      <ToolCardSection className="tool-card--approval tool-card--config">
        <ToolPayload args={args} argumentsText={argumentsText} phase={phase} toolName={toolName} cwd={commandCwd} t={t} />
        <dl className="tool-card__approval-context">
          <div><dt>{t('toolCard.cwd')}</dt><dd>{String(args.cwd || t('toolCard.defaultCwd'))}</dd></div>
          <div><dt>{t('toolCard.runtime')}</dt><dd>{String(args.runtime || t('toolCard.runtimeUnknown'))}</dd></div>
          <div><dt>{t('toolCard.permission')}</dt><dd>{t('toolCard.fullAccessPermission')}</dd></div>
          <div><dt>{t('toolCard.background')}</dt><dd>{args.background ? t('toolCard.enabled') : t('toolCard.disabled')}</dd></div>
          {args.justification ? <div><dt>{t('toolCard.justification')}</dt><dd>{String(args.justification)}</dd></div> : null}
        </dl>
      </ToolCardSection>
    );
  }

  if (processId) {
    // Only treat as running after the process manager confirms it. After restart the
    // process_id remains in chat history but the in-memory process is gone.
    const missing = !process && Boolean(processError);
    const loading = Boolean(processSnapshot?.loading) || (!process && !processError);
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
    const hasProcessOutput = Boolean(
      process && (hasText(process.stdout) || hasText(process.stderr)),
    );
    const showExecution = Boolean(
      missing
      || processError
      || (process && (!running || hasProcessOutput)),
    );
    return (
      <>
        <ToolPayloadSection
          className="tool-card--config"
          args={args}
          argumentsText={argumentsText}
          phase={phase}
          toolName={toolName}
          cwd={commandCwd}
          t={t}
        />
        {showExecution ? (
          <ToolCardSection className={`${cardTone} tool-card--execution`}>
            {missing ? (
              <p className="tool-card__reason" title={processError || title}>
                {t('toolCard.processStatus.notFoundBadge')}
              </p>
            ) : null}
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
          </ToolCardSection>
        ) : null}
      </>
    );
  }

  if (status === 'running') {
    if (!hasToolPayload(args, argumentsText, phase)) return null;
    return (
      <ToolCardSection className="tool-card--running tool-card--config">
        <ToolPayload args={args} argumentsText={argumentsText} phase={phase} toolName={toolName} cwd={commandCwd} t={t} />
      </ToolCardSection>
    );
  }

  if (status === 'completed' && result) {
    const failed = result.is_error === true || (result.exit_code ?? 0) !== 0;
    return (
      <>
        <ToolPayloadSection
          className="tool-card--config"
          args={args}
          argumentsText={argumentsText}
          phase={phase}
          toolName={toolName}
          cwd={commandCwd}
          t={t}
        />
        <ToolCardSection className={`${failed ? 'tool-card--error' : 'tool-card--done'} tool-card--execution`}>
          <TerminalOutputs
            stdout={result.stdout}
            stderr={result.stderr}
            structuredContent={result.structured_content}
            attachments={result.attachments}
            failed={failed}
            t={t}
          />
        </ToolCardSection>
      </>
    );
  }

  if (status === 'blocked' || status === 'rejected' || status === 'interrupted') {
    return (
      <>
        <ToolPayloadSection
          className="tool-card--config"
          args={args}
          argumentsText={argumentsText}
          phase={phase}
          toolName={toolName}
          cwd={commandCwd}
          t={t}
        />
        <ToolCardSection className="tool-card--blocked tool-card--execution">
          {result?.reason ? <p className="tool-card__reason">{result.reason}</p> : null}
        </ToolCardSection>
      </>
    );
  }

  if (status === 'error') {
    return (
      <>
        <ToolPayloadSection
          className="tool-card--config"
          args={args}
          argumentsText={argumentsText}
          phase={phase}
          toolName={toolName}
          cwd={commandCwd}
          t={t}
        />
        <ToolCardSection className="tool-card--error tool-card--execution">
          {result?.message ? <p className="tool-card__reason">{result.message}</p> : null}
        </ToolCardSection>
      </>
    );
  }

  return null;
}
