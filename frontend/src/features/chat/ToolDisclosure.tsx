import { useEffect, useState } from 'react';
import {
  AlertTriangle,
  CheckCircle,
  ChevronDown,
  Hand,
  Loader2,
  Wrench,
  XCircle,
} from 'lucide-react';

import { ToolCallCard } from '../../components/ToolCallCard';
import { useI18n } from '../../i18n';
import type { ToolCallState } from '../../types';

type ToolDisclosureProps = {
  agentId: string;
  tool: ToolCallState;
  onApprove?: (decision: 'approve' | 'reject' | 'always_approve') => Promise<void> | void;
};

function isTerminalTool(name: string): boolean {
  return name === 'execute_terminal' || name === 'terminal_session';
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
  if (tool.status === 'blocked') return t('toolCard.blocked');
  if (tool.status === 'rejected') return t('toolCard.rejected');
  if (tool.status === 'error') return t('toolCard.executionError');
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
  } else if (tool.status === 'blocked' || tool.status === 'rejected') {
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
  const isActive = tool.status === 'running' || tool.status === 'pending_approval';
  const [isOpen, setIsOpen] = useState(isActive);
  const [userToggled, setUserToggled] = useState(false);
  const [elapsedSeconds, setElapsedSeconds] = useState(0);
  const [approvalDecision, setApprovalDecision] = useState<'approve' | 'reject' | 'always_approve' | null>(null);

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
    if (userToggled) return;
    setIsOpen(isActive);
  }, [isActive, userToggled]);

  const label = tool.phase === 'generating_arguments' && tool.status === 'running'
    ? t('toolCard.generatingArguments')
    : statusLabel(tool, t);
  const tone = statusTone(tool);
  const toneClass = tone === 'default' ? '' : ` is-${tone}`;

  return (
    <section
      className={`message-tool${isOpen ? ' is-open' : ''}${isActive ? ' is-active' : ''}${toneClass}`}
    >
      <div className="message-tool__summary">
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
            <StatusIcon tool={tool} />
            <span className="message-tool__title">{label}</span>
            <code className="message-tool__name">{tool.name}</code>
            {isActive && elapsedSeconds > 0 ? (
              <span className="message-tool__elapsed">{t('toolCard.elapsed', { seconds: elapsedSeconds })}</span>
            ) : null}
            {tool.status === 'running' ? <span className="message-tool__pulse" aria-hidden="true" /> : null}
          </span>
          <ChevronDown className="message-tool__chevron" size={15} aria-hidden="true" />
        </button>
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
            callId={tool.call_id}
            name={tool.name}
            arguments={tool.arguments}
            status={tool.status}
            result={tool.result}
            argumentsText={tool.arguments_text}
            phase={tool.phase}
            hideHeader
            onApprove={onApprove}
          />
        </div>
      </div>
    </section>
  );
}
