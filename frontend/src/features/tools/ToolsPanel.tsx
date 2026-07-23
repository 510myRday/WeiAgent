import { useState } from 'react';
import { ArrowLeft, Globe, ShieldCheck, Terminal } from 'lucide-react';

import type { Agent } from '../../types';
import { useI18n } from '../../i18n';
import { PanelShell } from '../../components/layout';
import { Button, ListItem, PaneHeader } from '../../components/ui';
import { TerminalToolSettings } from '../settings/TerminalToolSettings';
import { WebSearchToolSettings } from '../settings/WebSearchToolSettings';

interface ToolsPanelProps {
  agents: Agent[];
  onOpenMain: () => void;
  onBackToList: () => void;
  onOpenIpProxies: () => void;
}

type ToolKey = 'terminal' | 'web_search';

const toolDefs: {
  key: ToolKey;
  icon: typeof Terminal;
  isBuiltin: boolean;
  getLabel: (t: (p: string) => string) => string;
  getDesc: (t: (p: string) => string) => string;
}[] = [
  {
    key: 'terminal',
    icon: Terminal,
    isBuiltin: true,
    getLabel: (t) => t('tools.terminal.name'),
    getDesc: (t) => t('tools.terminal.description'),
  },
  {
    key: 'web_search',
    icon: Globe,
    isBuiltin: true,
    getLabel: (t) => t('tools.webSearch.name'),
    getDesc: (t) => t('tools.webSearch.description'),
  },
];

export function ToolsPanel({ agents, onOpenMain, onBackToList, onOpenIpProxies }: ToolsPanelProps) {
  const { t } = useI18n();
  const [activeTool, setActiveTool] = useState<ToolKey>('terminal');
  const activeDef = toolDefs.find((d) => d.key === activeTool);

  return (
    <PanelShell
      className="tools-panel"
      sidebar={(
        <div className="context-pane tools-nav">
          <PaneHeader className="mobile-pane-header" title={t('tools.areaTitle')} />
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
          </div>
        </div>
      )}
      main={(
        <div className="main-pane config-main">
          <PaneHeader
            className="config-page-header"
            leading={<Button className="main-back-button" variant="ghost" type="button" onClick={onBackToList} icon={<ArrowLeft size={17} />}>{t('common.back')}</Button>}
            eyebrow={t('tools.areaTitle')}
            title={activeDef?.getLabel(t) ?? t('tools.title')}
            description={activeDef?.getDesc(t) ?? t('tools.description')}
          />
          {activeTool === 'terminal' ? <TerminalToolSettings scope="defaults" agents={agents} onOpenIpProxies={onOpenIpProxies} /> : null}
          {activeTool === 'web_search' ? <WebSearchToolSettings scope="defaults" agents={agents} onOpenIpProxies={onOpenIpProxies} /> : null}
        </div>
      )}
    />
  );
}
