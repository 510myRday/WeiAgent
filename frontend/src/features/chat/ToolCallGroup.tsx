import { useEffect, useState } from 'react';
import {
  AlertTriangle,
  CheckCircle,
  ChevronDown,
  Loader2,
  ListChecks,
  XCircle,
} from 'lucide-react';

import { useI18n } from '../../i18n';
import type { McpServer } from '../../toolTypes';
import type { ToolCallState, ToolCallStatus } from '../../types';
import { ToolDisclosure } from './ToolDisclosure';

type ToolCallGroupProps = {
  agentId: string;
  tools: ToolCallState[];
  mcpServers: McpServer[];
  onApprove?: (tool: ToolCallState, decision: 'approve' | 'reject' | 'always_approve') => Promise<void> | void;
};

type GroupTone = 'running' | 'approval' | 'done' | 'error' | 'default';

function isActiveStatus(status: ToolCallStatus): boolean {
  return status === 'running' || status === 'pending_approval';
}

function isErrorStatus(status: ToolCallStatus): boolean {
  return status === 'error'
    || status === 'blocked'
    || status === 'rejected'
    || status === 'interrupted';
}

function toolFailed(tool: ToolCallState): boolean {
  return isErrorStatus(tool.status)
    || tool.result?.is_error === true
    || (tool.result?.exit_code ?? 0) !== 0;
}

function groupTone(tools: ToolCallState[]): GroupTone {
  const lastTool = tools[tools.length - 1];
  if (!lastTool) return 'default';
  if (lastTool.status === 'pending_approval') return 'approval';
  if (lastTool.status === 'running') return 'running';
  if (toolFailed(lastTool)) return 'error';
  if (lastTool.status === 'completed') return 'done';
  return 'default';
}

function GroupStatusIcon({ tone }: { tone: GroupTone }) {
  if (tone === 'running') return <Loader2 size={16} className="animate-spin" aria-hidden="true" />;
  if (tone === 'approval') return <ListChecks size={16} aria-hidden="true" />;
  if (tone === 'error') return <XCircle size={16} aria-hidden="true" />;
  if (tone === 'done') return <CheckCircle size={16} aria-hidden="true" />;
  return <AlertTriangle size={16} aria-hidden="true" />;
}

export function ToolCallGroup({
  agentId,
  tools,
  mcpServers,
  onApprove,
}: ToolCallGroupProps) {
  const { t } = useI18n();
  const tone = groupTone(tools);
  const activeCount = tools.filter((tool) => isActiveStatus(tool.status)).length;
  const failedCount = tools.filter(toolFailed).length;
  const successCount = tools.filter((tool) => tool.status === 'completed' && !toolFailed(tool)).length;
  const isActive = activeCount > 0;
  const [isOpen, setIsOpen] = useState(isActive);
  const [userToggled, setUserToggled] = useState(false);
  const firstToolCallId = tools[0]?.call_id;

  useEffect(() => {
    setUserToggled(false);
    setIsOpen(isActive);
  }, [firstToolCallId, isActive]);

  useEffect(() => {
    if (!userToggled) setIsOpen(isActive);
  }, [isActive, userToggled]);

  const statusSummary = isActive
    ? t('toolCard.toolBatchRunningStats', {
      active: activeCount,
      success: successCount,
      failed: failedCount,
    })
    : (failedCount > 0 && successCount > 0)
      ? t('toolCard.toolBatchStats', { success: successCount, failed: failedCount })
      : failedCount > 0
        ? t('toolCard.toolBatchError', { count: failedCount })
        : t('toolCard.toolBatchCompleted', { count: successCount });

  return (
    <section className={`message-tool-group${isOpen ? ' is-open' : ''}${isActive ? ' is-active' : ''} is-${tone}`}>
      <button
        className="message-tool-group__toggle"
        type="button"
        aria-expanded={isOpen}
        onClick={() => {
          setUserToggled(true);
          setIsOpen((open) => !open);
        }}
      >
        <span className="message-tool-group__identity">
          <span className="message-tool-group__icon" aria-hidden="true"><GroupStatusIcon tone={tone} /></span>
          <span className="message-tool-group__title">{t('toolCard.toolBatchTitle')}</span>
          <span className="message-tool-group__count">{tools.length}</span>
        </span>
        <span className="message-tool-group__meta">{statusSummary}</span>
        <ChevronDown className="message-tool-group__chevron" size={16} aria-hidden="true" />
      </button>
      <div className="message-tool-group__region" hidden={!isOpen}>
        <div className="message-tool-group__content">
          {tools.map((tool) => (
            <ToolDisclosure
              key={tool.call_id}
              agentId={agentId}
              tool={tool}
              mcpServers={mcpServers}
              onApprove={onApprove ? (decision) => onApprove(tool, decision) : undefined}
            />
          ))}
        </div>
      </div>
    </section>
  );
}
