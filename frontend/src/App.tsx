import { useState, useEffect } from 'react';
import type { NavKey } from './types';
import { useSettings, useProviders, useAgents } from './hooks';
import { Sidebar } from './components/Sidebar';
import { ChatPanel } from './components/ChatPanel';
import { AgentsPanel } from './components/AgentsPanel';
import { HistoryPanel } from './components/HistoryPanel';
import { SettingsPanel } from './components/SettingsPanel';

export function App() {
  const [activeNav, setActiveNav] = useState<NavKey>('chat');
  const [sidebarOpen, setSidebarOpen] = useState(true);
  const [error, setError] = useState('');

  useEffect(() => {
    const mq = window.matchMedia('(min-width: 921px)');
    setSidebarOpen(mq.matches);
    const handler = (e: MediaQueryListEvent) => setSidebarOpen(e.matches);
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

  return (
    <main className="workspace-shell">
      <Sidebar
        activeNav={activeNav}
        sidebarOpen={sidebarOpen}
        onNavChange={setActiveNav}
        onToggleSidebar={() => setSidebarOpen((v) => !v)}
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
        />
      )}
      {activeNav === 'history' && <HistoryPanel agents={agents} />}
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
        />
      )}
    </main>
  );
}
