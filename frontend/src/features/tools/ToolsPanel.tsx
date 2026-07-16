import { useState } from 'react';
import { ArrowLeft, ShieldCheck, Terminal } from 'lucide-react';

import type { Agent } from '../../types';
import { useI18n } from '../../i18n';
import { PanelShell } from '../../components/layout';
import { Button, ListItem, PaneHeader } from '../../components/ui';
import { TerminalToolSettings } from '../settings/TerminalToolSettings';

interface ToolsPanelProps {
  agents: Agent[];
  onOpenMain: () => void;
  onBackToList: () => void;
}

type ToolKey = 'terminal';

const toolDefs: { key: ToolKey; icon: typeof Terminal; isBuiltin: boolean; getLabel: (t: (p: string) => string) => string; getDesc: (t: (p: string) => string) => string }[] = [
  { key: 'terminal', icon: Terminal, isBuiltin: true, getLabel: (t) => t('tools.terminal.name'), getDesc: (t) => t('tools.terminal.description') },
];

export function ToolsPanel({ agents, onOpenMain, onBackToList }: ToolsPanelProps) {
  const { t } = useI18n();
  const [activeTool, setActiveTool] = useState<ToolKey>('terminal');

  const activeDef = toolDefs.find((d) => d.key === activeTool);

  return (
    <PanelShell
      className="tools-panel"
      sidebar={(
        <div className="context-pane tools-nav">
          <PaneHeader
            className="mobile-pane-header"
            title={t('nav.tools')}
          />
          <div className="list-stack">
            {toolDefs.map((def) => (
              <ListItem
                key={def.key}
                type="button"
                active={activeTool === def.key}
                title={(
                  <span className="tool-list-title">
                    <span>{def.getLabel(t)}</span>
                    {def.isBuiltin ? (
                      <span className="mini-status-badge">
                        <ShieldCheck size={12} />
                        {t('tools.builtinBadge')}
                      </span>
                    ) : null}
                  </span>
                )}
                subtitle={def.getDesc(t)}
                leading={<def.icon size={18} />}
                onClick={() => { setActiveTool(def.key); onOpenMain(); }}
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
            leading={<Button className="main-back-button" variant="ghost" type="button" onClick={onBackToList} icon={<ArrowLeft size={17} />}>{t('common.back')}</Button>}
            eyebrow={t('nav.tools')}
            title={activeDef?.getLabel(t) ?? t('tools.title')}
            description={activeDef?.getDesc(t) ?? t('tools.description')}
          />

          {activeTool === 'terminal' ? (
            <TerminalToolSettings scope="defaults" agents={agents} />
          ) : null}
        </div>
      )}
    />
  );
}
