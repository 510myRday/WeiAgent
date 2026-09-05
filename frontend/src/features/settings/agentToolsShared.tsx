import type { ReactNode } from 'react';
import { Download, RefreshCw } from 'lucide-react';

import type { AgentToolsConfig, EnvironmentRuntimeStatus, RuntimeComponentStatus } from '../../toolTypes';
import type { Agent } from '../../types';
import { Button, IconButton } from '../../components/ui';

export function normalizeRuntimeComponents(runtime: EnvironmentRuntimeStatus | null): RuntimeComponentStatus[] {
  if (runtime?.components?.length) return runtime.components;
  return [
    ...(['node', 'python', 'uv', 'git', 'ffmpeg'] as const).map((kind) => ({
      kind,
      available: false,
      required: false,
      managed_installed: false,
      installing: false,
      update_available: false,
    })),
  ];
}

export function RuntimeComponentRow({
  component,
  installing,
  refreshing,
  actionsDisabled,
  onInstall,
  onRefresh,
  t,
}: {
  component: RuntimeComponentStatus;
  installing: boolean;
  refreshing: boolean;
  actionsDisabled: boolean;
  onInstall: () => void;
  onRefresh: () => void;
  t: (key: string, values?: Record<string, string | number>) => string;
}) {
  const icon = {
    node: '/tool-icons/nodejs.svg',
    python: '/tool-icons/python.svg',
    uv: '/tool-icons/uv.svg',
    git: '/tool-icons/git.svg',
    ffmpeg: '/tool-icons/ffmpeg.svg',
  }[component.kind];
  const source = component.source ? t(`tools.terminal.runtimeSource.${component.source}`) : '';
  const latest = component.target_version
    ? t('tools.terminal.runtimeLatestVersion', { version: component.target_version })
    : t('tools.terminal.runtimeLatestUnknown');
  const meta = [component.detected_version, latest, source].filter(Boolean).join(' · ');
  const statusLabel = installing
    ? t(component.available ? 'tools.terminal.runtimeUpdating' : 'tools.terminal.runtimeInstalling')
    : !component.available
    ? t('tools.terminal.runtimeMissing')
    : component.update_available
      ? t('tools.terminal.runtimeUpdateAvailable', { version: component.target_version ?? '' })
      : component.target_version
        ? t('tools.terminal.runtimeLatest')
        : t('tools.terminal.runtimeAvailable');

  return (
    <div className="runtime-component-row">
      <span className={`runtime-component-row__icon is-${component.kind}`}>
        <img src={icon} alt="" aria-hidden="true" />
      </span>
      <div className="runtime-component-row__details">
        <div className="runtime-component-row__title">
          <strong>{t(`tools.terminal.runtimeComponent.${component.kind}`)}</strong>
          <span className={`runtime-status ${installing ? 'is-progress' : component.available && !component.update_available ? 'is-ready' : 'is-missing'}`}>
            {statusLabel}
          </span>
          {component.required ? <span className="runtime-required">{t('tools.terminal.runtimeRequired')}</span> : null}
        </div>
        <small>{meta || t('tools.terminal.runtimeNotDetected')}</small>
        {component.error ? <code className="runtime-component-row__error">{component.error}</code> : null}
      </div>
      <div className="runtime-component-row__actions">
        {!component.available || component.update_available ? (
          <Button
            variant="secondary"
            onClick={onInstall}
            disabled={actionsDisabled}
            icon={installing
              ? <RefreshCw size={16} className="is-spinning" />
              : <Download size={16} />}
          >
            {installing
              ? t(component.available ? 'tools.terminal.runtimeUpdating' : 'tools.terminal.runtimeInstalling')
              : component.update_available && component.target_version
                ? t('tools.terminal.runtimeUpdateTo', { version: component.target_version })
                : t('tools.terminal.runtimeInstall')}
          </Button>
        ) : null}
        <IconButton
          type="button"
          variant="ghost"
          onClick={onRefresh}
          disabled={actionsDisabled}
          label={t('tools.terminal.runtimeRefreshComponent', {
            name: t(`tools.terminal.runtimeComponent.${component.kind}`),
          })}
        >
          <RefreshCw size={17} className={refreshing ? 'is-spinning' : ''} />
        </IconButton>
      </div>
    </div>
  );
}

export function ToolRecordRow({
  command,
  status,
  meta,
  action,
  children,
  className = '',
}: {
  command: string;
  status: ReactNode;
  meta: ReactNode;
  action?: ReactNode;
  children?: ReactNode;
  className?: string;
}) {
  return (
    <article className={`tool-process-row ${className}`.trim()}>
      <div className="tool-process-row__header">
        <code title={command}>{command}</code>
        <span>{status}</span>
        {action}
      </div>
      <small>{meta}</small>
      {children}
    </article>
  );
}


export function configWithClearedGrants(config: AgentToolsConfig): AgentToolsConfig {
  return {
    ...config,
    terminal: {
      ...config.terminal,
      approval_grants: [],
    },
  };
}

export function serializeDefaultConfig(config: AgentToolsConfig): string {
  return JSON.stringify(configWithClearedGrants(config));
}

export function agentName(agentId: string, agents: Agent[], t: (path: string) => string): string {
  const agent = agents.find((item) => item.id === agentId);
  if (!agent) return agentId;
  return agent.is_builtin ? t('agentConfig.builtinAgentName') : agent.name;
}
