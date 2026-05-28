import { useState, useEffect } from 'react';
import { Bot, MessageCircle } from 'lucide-react';
import type { MobileView, NavKey } from './types';
import { useSettings, useProviders, useAgents } from './hooks';
import { Sidebar } from './components/Sidebar';
import { ChatPanel } from './components/ChatPanel';
import { AgentsPanel } from './components/AgentsPanel';
import { SettingsPanel } from './components/SettingsPanel';
import { useI18n } from './i18n';

export function App() {
  const [activeNav, setActiveNav] = useState<NavKey>('chat');
  const [mobileView, setMobileView] = useState<MobileView>('list');
  const [error, setError] = useState('');

  useEffect(() => {
    const mq = window.matchMedia('(min-width: 921px)');
    const syncMobileView = (matches: boolean) => {
      if (matches) {
        setMobileView('list');
      }
    };
    syncMobileView(mq.matches);
    const handler = (e: MediaQueryListEvent) => syncMobileView(e.matches);
    mq.addEventListener('change', handler);
    return () => mq.removeEventListener('change', handler);
  }, []);

  const {
    settingsForm,
    setSettingsForm,
    refreshSettings,
    saveSettings,
  } = useSettings();
  const {
    providers,
    activeProviderId,
    setActiveProviderId,
    refreshProviders,
    addProvider,
    editProvider,
    removeProvider,
  } = useProviders();
  const {
    agents,
    activeAgentId,
    setActiveAgentId,
    isLoadingAgents,
    refreshAgents,
    addAgent,
    editAgent,
    removeAgent,
  } = useAgents();

  useEffect(() => {
    void refreshAll();
  }, []);

  async function refreshAll() {
    setError('');
    try {
      await Promise.all([refreshSettings(), refreshProviders(), refreshAgents()]);
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : 'Error');
    }
  }

  function handleNavChange(nav: NavKey) {
    setActiveNav(nav);
    setError('');
    setMobileView('list');
  }

  function openMainPane() {
    setMobileView('main');
  }

  function openListPane() {
    setMobileView('list');
  }

  return (
    <main className={`workspace-shell mobile-${mobileView}`}>
      <Sidebar
        activeNav={activeNav}
        onNavChange={handleNavChange}
        onMobileHome={() => setMobileView('rail')}
      />
      {error ? (
        <div className="error-banner global-error">
          <span>{error}</span>
          <button className="error-dismiss" type="button" onClick={() => setError('')}>&times;</button>
        </div>
      ) : null}
      {activeNav === 'chat' && (
        <ChatPanel
          agents={agents}
          activeAgentId={activeAgentId}
          onAgentChange={setActiveAgentId}
          onError={setError}
          onOpenMain={openMainPane}
          onBackToList={openListPane}
          onBackToRail={() => setMobileView('rail')}
        />
      )}
      {activeNav === 'agents' && (
        <AgentsPanel
          agents={agents}
          providers={providers}
          activeAgentId={activeAgentId}
          isLoadingAgents={isLoadingAgents}
          onAgentChange={setActiveAgentId}
          onAgentAdd={addAgent}
          onAgentEdit={editAgent}
          onAgentRemove={removeAgent}
          onOpenMain={openMainPane}
          onBackToRail={() => setMobileView('rail')}
        />
      )}
      {activeNav === 'groups' && (
        <GroupsPlaceholder
          onOpenMain={openMainPane}
          onBackToRail={() => setMobileView('rail')}
        />
      )}
      {activeNav === 'settings' && (
        <SettingsPanel
          settingsForm={settingsForm}
          onSettingsFormChange={setSettingsForm}
          onSettingsSave={saveSettings}
          providers={providers}
          activeProviderId={activeProviderId}
          onProviderChange={setActiveProviderId}
          onProviderAdd={addProvider}
          onProviderEdit={editProvider}
          onProviderRemove={removeProvider}
          onOpenMain={openMainPane}
          onBackToRail={() => setMobileView('rail')}
        />
      )}
    </main>
  );
}

function GroupsPlaceholder({
  onOpenMain,
  onBackToRail,
}: {
  onOpenMain: () => void;
  onBackToRail: () => void;
}) {
  const { t } = useI18n();

  return (
    <>
      <section className="context-pane groups-list-pane">
        <header className="pane-header mobile-pane-header">
          <button className="back-button" type="button" onClick={onBackToRail}>{t('common.back')}</button>
          <div>
            <h2>{t('groups.title')}</h2>
            <p>{t('groups.description')}</p>
          </div>
        </header>
        <button className="list-row active" type="button" onClick={onOpenMain}>
          <span className="row-avatar muted-avatar"><Bot size={18} /></span>
          <span className="row-main">
            <strong>{t('groups.entryTitle')}</strong>
            <small>{t('groups.entryHint')}</small>
          </span>
        </button>
      </section>
      <section className="main-pane placeholder-pane">
        <div className="empty-chat">
          <MessageCircle size={34} />
          <h3>{t('groups.emptyTitle')}</h3>
          <p>{t('groups.emptyHint')}</p>
        </div>
      </section>
    </>
  );
}
