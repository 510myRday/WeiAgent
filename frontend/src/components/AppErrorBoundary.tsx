import { Component, type ErrorInfo, type ReactNode } from 'react';
import { RefreshCw, TriangleAlert } from 'lucide-react';

import { useI18n } from '../i18n';
import { Button } from './ui';

type ErrorBoundaryProps = {
  children: ReactNode;
  resetKey?: string;
  scope?: 'app' | 'feature';
};

type ErrorBoundaryState = {
  error: Error | null;
};

class ErrorBoundary extends Component<ErrorBoundaryProps, ErrorBoundaryState> {
  state: ErrorBoundaryState = { error: null };

  static getDerivedStateFromError(error: Error): ErrorBoundaryState {
    return { error };
  }

  componentDidCatch(error: Error, info: ErrorInfo) {
    console.error('WeiAgent render error', error, info);
  }

  componentDidUpdate(previousProps: ErrorBoundaryProps) {
    if (this.state.error && previousProps.resetKey !== this.props.resetKey) {
      this.setState({ error: null });
    }
  }

  render() {
    if (!this.state.error) return this.props.children;
    return (
      <ErrorFallback
        scope={this.props.scope ?? 'feature'}
        onRetry={() => this.setState({ error: null })}
      />
    );
  }
}

export function AppErrorBoundary(props: ErrorBoundaryProps) {
  return <ErrorBoundary {...props} />;
}

function ErrorFallback({
  scope,
  onRetry,
}: {
  scope: 'app' | 'feature';
  onRetry: () => void;
}) {
  const { t } = useI18n();
  const isApp = scope === 'app';
  return (
    <section className={`error-boundary-fallback error-boundary-fallback--${scope}`} role="alert">
      <TriangleAlert size={28} />
      <div>
        <strong>{t(isApp ? 'error.appCrashedTitle' : 'error.featureCrashedTitle')}</strong>
        <p>{t(isApp ? 'error.appCrashedMessage' : 'error.featureCrashedMessage')}</p>
      </div>
      <Button
        variant="secondary"
        type="button"
        onClick={isApp ? () => window.location.reload() : onRetry}
        icon={<RefreshCw size={15} />}
      >
        {t(isApp ? 'error.reloadApp' : 'common.retry')}
      </Button>
    </section>
  );
}
