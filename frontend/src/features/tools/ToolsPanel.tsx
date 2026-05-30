import { useState } from 'react';
import { Terminal, Wrench } from 'lucide-react';

import type { Agent } from '../../types';
import { useI18n } from '../../i18n';
import { PanelShell } from '../../components/layout';
import { Button, EmptyState, ListItem, PaneHeader } from '../../components/ui';
import { TerminalToolSettings } from '../settings/TerminalToolSettings';

interface ToolsPanelProps {
  agents: Agent[];
  activeAgentId: string;
  onOpenMain: () => void;
}

type ToolKey = 'terminal';

const toolDefs: { key: ToolKey; icon: typeof Terminal; getLabel: (t: (p: string) => string) => string; getDesc: (t: (p: string) => string) => string }[] = [
  { key: 'terminal', icon: Terminal, getLabel: (t) => t('tools.terminal.name'), getDesc: (t) => t('tools.terminal.description') },
];

export function ToolsPanel({ agents, activeAgentId, onOpenMain }: ToolsPanelProps) {
  const { t } = useI18n();
  const [activeTool, setActiveTool] = useState<ToolKey>('terminal');
  const [error, setError] = useState('');

  const activeAgent = agents.find((a) => a.id === activeAgentId);
  const activeDef = toolDefs.find((d) => d.key === activeTool);

  return (
    <PanelShell
      className="tools-panel"
      sidebar={(
        <div className="context-pane tools-nav">
          <PaneHeader
            className="mobile-pane-header"
            title={t('nav.tools')}
            leading={<Button className="back-button" type="button" onClick={onOpenMain}>{t('common.back')}</Button>}
          />
          <div className="list-stack">
            {toolDefs.map((def) => (
              <ListItem
                key={def.key}
                type="button"
                active={activeTool === def.key}
                title={def.getLabel(t)}
                subtitle={def.getDesc(t)}
                leading={<def.icon size={18} />}
                onClick={() => { setActiveTool(def.key); setError(''); onOpenMain(); }}
              />
            ))}
            <p className="muted" style={{ padding: '12px 16px', fontSize: '13px' }}>{t('tools.comingSoon')}</p>
          </div>
        </div>
      )}
      main={(
        <div className="main-pane config-main">
          <PaneHeader
            className="config-page-header"
            eyebrow={t('nav.tools')}
            title={activeDef?.getLabel(t) ?? t('tools.title')}
            description={activeDef?.getDesc(t) ?? t('tools.description')}
          />

          {!activeAgent ? (
            <EmptyState
              icon={<Wrench size={34} />}
              title={t('tools.title')}
              description={t('tools.selectHint')}
            />
          ) : activeTool === 'terminal' ? (
            <TerminalToolSettings agentId={activeAgentId} onError={setError} />
          ) : null}

          {error ? (
            <div className="alert-banner error" style={{ margin: '0 24px 24px' }}>
              <span>{error}</span>
              <button type="button" onClick={() => setError('')}>{t('error.dismiss')}</button>
            </div>
          ) : null}
        </div>
      )}
    />
  );
}
