import { Terminal, CheckCircle, XCircle, AlertTriangle, Loader2 } from 'lucide-react';

export type ToolCallStatus = 'running' | 'pending_approval' | 'completed' | 'blocked' | 'rejected' | 'error';

export interface ToolResultData {
  stdout?: string;
  stderr?: string;
  exit_code?: number;
  duration_ms?: number;
  process_id?: string;
  message?: string;
  reason?: string;
}

interface ToolCallCardProps {
  callId: string;
  name: string;
  arguments: Record<string, any>;
  status: ToolCallStatus;
  result?: ToolResultData;
  onApprove?: (decision: 'approve' | 'reject' | 'always_approve') => void;
}

export function ToolCallCard({ callId, name, arguments: args, status, result, onApprove }: ToolCallCardProps) {
  const command = args.command || '';

  if (status === 'pending_approval' && onApprove) {
    return (
      <div className="tool-card tool-card--approval">
        <div className="tool-card__header">
          <Terminal size={18} />
          <span>Request to execute terminal command</span>
        </div>
        <pre className="tool-card__command">{command}</pre>
        <div className="tool-card__actions">
          <button onClick={() => onApprove('approve')} className="btn-approve">Allow</button>
          <button onClick={() => onApprove('reject')} className="btn-reject">Reject</button>
        </div>
      </div>
    );
  }

  if (status === 'running') {
    return (
      <div className="tool-card tool-card--running">
        <Loader2 size={18} className="animate-spin" />
        <span>Executing: {command}</span>
      </div>
    );
  }

  if (status === 'completed' && result) {
    return (
      <div className="tool-card tool-card--done">
        <div className="tool-card__header">
          <CheckCircle size={18} />
          <span>Completed (exit code: {result.exit_code ?? 0})</span>
        </div>
        {result.stdout && <pre className="tool-card__output">{result.stdout}</pre>}
        {result.stderr && <pre className="tool-card__error">{result.stderr}</pre>}
      </div>
    );
  }

  if (status === 'blocked' || status === 'rejected') {
    return (
      <div className="tool-card tool-card--blocked">
        <XCircle size={18} />
        <span>{status === 'blocked' ? 'Blocked by security policy' : 'Rejected'}</span>
        {result?.reason && <p className="tool-card__reason">{result.reason}</p>}
      </div>
    );
  }

  if (status === 'error') {
    return (
      <div className="tool-card tool-card--error">
        <AlertTriangle size={18} />
        <span>Execution Error</span>
        {result?.message && <p className="tool-card__reason">{result.message}</p>}
      </div>
    );
  }

  return null;
}
