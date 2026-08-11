import { useEffect, useState } from 'react';
import {
  AlertTriangle,
  CheckCircle,
  ChevronDown,
  Hand,
  Loader2,
  Square,
  Wrench,
  XCircle,
} from 'lucide-react';

import { stopToolProcess } from '../../api';
import { ToolCallCard } from '../../components/ToolCallCard';
import { useI18n } from '../../i18n';
import type { ToolCallState } from '../../types';
import { updateToolProcessCache, useToolProcess } from '../../hooks';

type ToolDisclosureProps = {
  agentId: string;
  tool: ToolCallState;
  onApprove?: (decision: 'approve' | 'reject' | 'always_approve') => Promise<void> | void;
};

function isTerminalTool(name: string): boolean {
  return name === 'execute_terminal' || name === 'terminal_session';
}

function isBackgroundTerminal(tool: ToolCallState): boolean {
  return tool.name === 'execute_terminal' && tool.arguments.background === true;
}

function statusLabel(
  tool: ToolCallState,
  t: (path: string, vars?: Record<string, string | number>) => string,
): string {
  if (tool.status === 'pending_approval') return t('toolCard.approvalTitle');
  if (tool.status === 'running') {
    return isTerminalTool(tool.name)
      ? t('toolCard.executingTitle')
      : t('toolCard.executingGeneric');
  }
  if (tool.status === 'blocked') {
    if (tool.result?.code === 'PlanningRequired') return t('toolCard.planningRequired');
    if (tool.result?.code === 'PlanningIdInvalid') return t('toolCard.planningIdInvalid');
    return t('toolCard.blocked');
  }
  if (tool.status === 'rejected') return t('toolCard.rejected');
  if (tool.status === 'interrupted') return t('toolCard.interrupted');
  if (tool.status === 'error') {
    if (tool.result?.code === 'PlanningRequired') return t('toolCard.planningRequired');
    if (tool.result?.code === 'PlanningIdInvalid') return t('toolCard.planningIdInvalid');
    return t('toolCard.executionError');
  }
  if (tool.status === 'completed') {
    if (tool.result?.exit_code !== undefined && tool.result?.exit_code !== null) {
      return t('toolCard.completed', { code: tool.result.exit_code });
    }
    return t('toolCard.completedGeneric');
  }
  return tool.name;
}

function statusTone(tool: ToolCallState): 'running' | 'approval' | 'done' | 'error' | 'default' {
  if (tool.status === 'pending_approval') return 'approval';
  if (tool.status === 'running') return 'running';
  if (
    tool.status === 'error'
    || tool.status === 'blocked'
    || tool.status === 'rejected'
    || tool.status === 'interrupted'
    || (tool.status === 'completed' && (tool.result?.exit_code ?? 0) !== 0)
  ) {
    return 'error';
  }
  if (tool.status === 'completed') return 'done';
  return 'default';
}

function StatusIcon({ tool }: { tool: ToolCallState }) {
  const tone = statusTone(tool);
  let icon = <Wrench size={15} aria-hidden="true" />;
  if (tool.status === 'running') {
    icon = <Loader2 size={15} className="animate-spin" aria-hidden="true" />;
  } else if (tool.status === 'pending_approval') {
    icon = <Hand size={15} aria-hidden="true" />;
  } else if (tool.status === 'blocked' || tool.status === 'rejected' || tool.status === 'interrupted') {
    icon = <XCircle size={15} aria-hidden="true" />;
  } else if (tone === 'error') {
    icon = <AlertTriangle size={15} aria-hidden="true" />;
  } else if (tool.status === 'completed') {
    icon = <CheckCircle size={15} aria-hidden="true" />;
  }
  return <span className={`message-tool__icon message-tool__icon--${tone}`}>{icon}</span>;
}

export function ToolDisclosure({
  agentId,
  tool,
  onApprove,
}: ToolDisclosureProps) {
  const { t } = useI18n();
  const processId = tool.result?.process_id;
  const processSnapshot = useToolProcess(processId, agentId);
  const process = processSnapshot.process;
  let displayStatus = tool.status;
  let displayResult = tool.result;
  if (tool.status === 'running' && processId) {
    if (process?.status === 'completed' || process?.status === 'failed') {
      displayStatus = 'completed';
      displayResult = {
        ...tool.result,
        stdout: process.stdout,
        stderr: process.stderr,
        exit_code: process.exit_code ?? (process.status === 'completed' ? 0 : -1),
        duration_ms: process.duration_ms,
        runtime: process.runtime,
      };
    } else if (process?.status === 'stopped') {
      displayStatus = 'rejected';
      displayResult = {
        ...tool.result,
        reason: t('toolCard.processStatus.stopped'),
      };
    } else if (processSnapshot.notFound) {
      displayStatus = 'error';
      displayResult = {
        ...tool.result,
        message: t('toolCard.processUnavailable'),
      };
    }
  }
  const displayTool = { ...tool, status: displayStatus, result: displayResult };
  const checkingProcess = Boolean(
    tool.status === 'running'
      && processId
      && !process
      && !processSnapshot.error
      && !processSnapshot.notFound,
  );
  const stoppedProcess = process?.status === 'stopped';
  const isActive = displayStatus === 'running' || displayStatus === 'pending_approval';
  const [isOpen, setIsOpen] = useState(isActive);
  const [userToggled, setUserToggled] = useState(false);
  const [elapsedSeconds, setElapsedSeconds] = useState(0);
  const [approvalDecision, setApprovalDecision] = useState<'approve' | 'reject' | 'always_approve' | null>(null);
  const [isStopping, setIsStopping] = useState(false);

  useEffect(() => {
    setUserToggled(false);
    setIsOpen(isActive);
    setElapsedSeconds(0);
    setApprovalDecision(null);
  }, [isActive, tool.call_id]);

  useEffect(() => {
    if (!isActive || !tool.started_at_ms) return undefined;
    const update = () => {
      setElapsedSeconds(Math.max(0, Math.floor((Date.now() - tool.started_at_ms!) / 1000)));
    };
    update();
    const timer = window.setInterval(update, 1000);
    return () => window.clearInterval(timer);
  }, [isActive, tool.started_at_ms]);

  useEffect(() => {
    setApprovalDecision(null);
  }, [tool.status, tool.arguments.approval_id]);

  useEffect(() => {
    setIsStopping(false);
  }, [tool.call_id, processId, process?.status]);

  useEffect(() => {
    if (userToggled) return;
    setIsOpen(isActive);
  }, [isActive, userToggled]);

  const label = checkingProcess
    ? t('toolCard.processStatus.checking')
    : stoppedProcess
    ? t('toolCard.processStatus.stopped')
    : tool.phase === 'generating_arguments' && displayStatus === 'running'
    ? t('toolCard.generatingArguments')
    : statusLabel(displayTool, t);
  const tone = statusTone(displayTool);
  const toneClass = tone === 'default' ? '' : ` is-${tone}`;
  const canStopProcess = tool.status === 'running' && process?.status === 'running' && Boolean(processId);

  async function handleStopProcess() {
    if (!processId || isStopping) return;
    setIsStopping(true);
    try {
      const stopped = await stopToolProcess(processId, agentId);
      updateToolProcessCache(stopped);
    } finally {
      setIsStopping(false);
    }
  }

  return (
    <section
      className={`message-tool${isOpen ? ' is-open' : ''}${isActive ? ' is-active' : ''}${toneClass}`}
    >
      <div className={`message-tool__summary${canStopProcess ? ' message-tool__summary--has-stop' : ''}`}>
        <button
          className="message-tool__toggle"
          type="button"
          aria-expanded={isOpen}
          onClick={() => {
            setUserToggled(true);
            setIsOpen((open) => !open);
          }}
        >
          <span className="message-tool__identity">
            <StatusIcon tool={displayTool} />
            <span className="message-tool__title">{label}</span>
            <code className="message-tool__name">{tool.name}</code>
            {isBackgroundTerminal(tool) ? (
              <span className="message-tool__background-badge">{t('toolCard.backgroundBadge')}</span>
            ) : null}
            {isActive && elapsedSeconds > 0 ? (
              <span className="message-tool__elapsed">{t('toolCard.elapsed', { seconds: elapsedSeconds })}</span>
            ) : null}
            {displayStatus === 'running' ? <span className="message-tool__pulse" aria-hidden="true" /> : null}
          </span>
          <ChevronDown className="message-tool__chevron" size={15} aria-hidden="true" />
        </button>
        {canStopProcess ? (
          <div className="message-tool__inline-actions message-tool__inline-actions--stop">
            <button
              type="button"
              className="message-tool__action message-tool__action--stop"
              disabled={isStopping}
              onClick={(event) => {
                event.stopPropagation();
                void handleStopProcess();
              }}
            >
              <Square size={13} />
              {isStopping ? t('toolCard.stopping') : t('toolCard.stop')}
            </button>
          </div>
        ) : null}
        {tool.status === 'pending_approval' && onApprove ? (
          <div className="message-tool__inline-actions">
            <button
              type="button"
              className="message-tool__action message-tool__action--allow"
              disabled={approvalDecision !== null}
              onClick={(event) => {
                event.stopPropagation();
                if (approvalDecision) return;
                setApprovalDecision('approve');
                void Promise.resolve(onApprove('approve')).catch(() => setApprovalDecision(null));
              }}
            >
              {approvalDecision === 'approve' ? t('toolCard.approvalSubmitting') : t('toolCard.allow')}
            </button>
            <button
              type="button"
              className="message-tool__action message-tool__action--always"
              disabled={approvalDecision !== null}
              title={t('toolCard.alwaysAllow')}
              onClick={(event) => {
                event.stopPropagation();
                if (approvalDecision) return;
                setApprovalDecision('always_approve');
                void Promise.resolve(onApprove('always_approve')).catch(() => setApprovalDecision(null));
              }}
            >
              {approvalDecision === 'always_approve' ? t('toolCard.approvalSubmitting') : t('toolCard.alwaysAllow')}
            </button>
            <button
              type="button"
              className="message-tool__action message-tool__action--reject"
              disabled={approvalDecision !== null}
              onClick={(event) => {
                event.stopPropagation();
                if (approvalDecision) return;
                setApprovalDecision('reject');
                void Promise.resolve(onApprove('reject')).catch(() => setApprovalDecision(null));
              }}
            >
              {approvalDecision === 'reject' ? t('toolCard.approvalSubmitting') : t('toolCard.reject')}
            </button>
          </div>
        ) : null}
      </div>
      <div className="message-tool__region" hidden={!isOpen}>
        <div className="message-tool__content">
          <ToolCallCard
            agentId={agentId}
            arguments={tool.arguments}
            status={displayStatus}
            result={displayResult}
            argumentsText={tool.arguments_text}
            phase={tool.phase}
            onApprove={onApprove}
          />
        </div>
      </div>
    </section>
  );
}
