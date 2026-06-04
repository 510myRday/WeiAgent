import { useState, useEffect } from 'react';
import { Bot, MessageCircle } from 'lucide-react';
import type { AppSettings, MobileView, NavKey } from './types';
import { useSettings, useProviders, useAgents } from './hooks';
import { Sidebar } from './components/layout';
import { ChatPanel } from './features/chat/ChatPanel';
import { AgentsPanel } from './features/agents/AgentsPanel';
import { ToolsPanel } from './features/tools/ToolsPanel';
import { SettingsPanel } from './features/settings/SettingsPanel';
import { AlertBanner, Button, EmptyState, ListItem, PaneHeader } from './components/ui';
import { isLocale, useI18n } from './i18n';

type SettingsSection = 'runtime' | 'providers';

export function App() {
  const { t, locale, setLocaleStateOnly } = useI18n();
  const [activeNav, setActiveNav] = useState<NavKey>('chat');
  const [settingsSection, setSettingsSection] = useState<SettingsSection>('runtime');
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
      const [settings] = await Promise.all([refreshSettings(), refreshProviders(), refreshAgents()]);
      if (isLocale(settings.language)) {
        setLocaleStateOnly(settings.language);
      }
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : 'Error');
    }
  }

  async function saveSettingsWithLocale(payload: AppSettings) {
    const next = { ...payload, language: isLocale(payload.language) ? payload.language : locale };
    const updated = await saveSettings(next);
    if (isLocale(updated.language)) {
      setLocaleStateOnly(updated.language);
    }
    return updated;
  }

  function handleNavChange(nav: NavKey) {
    setActiveNav(nav);
    setError('');
    setMobileView('list');
  }

  function openProviderSettings() {
    setSettingsSection('providers');
    setActiveNav('settings');
    setError('');
    setMobileView('main');
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
        settings={settingsForm}
        onNavChange={handleNavChange}
        onSettingsSave={saveSettingsWithLocale}
      />
      {error ? <AlertBanner message={error} onDismiss={() => setError('')} dismissLabel={t('error.dismiss')} global /> : null}
      {activeNav === 'chat' && (
        <ChatPanel
          agents={agents}
          activeAgentId={activeAgentId}
          onAgentChange={setActiveAgentId}
          onError={setError}
          onOpenMain={openMainPane}
          onBackToList={openListPane}
          settings={settingsForm}
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
          onOpenProviderSettings={openProviderSettings}
          onOpenMain={openMainPane}
        />
      )}
      {activeNav === 'groups' && (
        <GroupsPlaceholder
          onOpenMain={openMainPane}
        />
      )}
      {activeNav === 'tools' && (
        <ToolsPanel
          agents={agents}
          activeAgentId={activeAgentId}
          onOpenMain={openMainPane}
        />
      )}
      {activeNav === 'settings' && (
        <SettingsPanel
          settingsForm={settingsForm}
          onSettingsFormChange={setSettingsForm}
          onSettingsSave={saveSettingsWithLocale}
          providers={providers}
          activeProviderId={activeProviderId}
          initialSection={settingsSection}
          onProviderChange={setActiveProviderId}
          onProviderAdd={async (payload) => {
            const created = await addProvider(payload);
            await refreshAgents();
            return created;
          }}
          onProviderEdit={async (id, payload) => {
            const updated = await editProvider(id, payload);
            await refreshAgents();
            return updated;
          }}
          onProviderRemove={async (id) => {
            await removeProvider(id);
            await refreshAgents();
          }}
          onOpenMain={openMainPane}
        />
      )}
    </main>
  );
}

function GroupsPlaceholder({
  onOpenMain,
}: {
  onOpenMain: () => void;
}) {
  const { t } = useI18n();

  return (
    <>
      <section className="context-pane groups-list-pane">
        <PaneHeader
          className="mobile-pane-header"
          title={t('groups.title')}
          description={t('groups.description')}
          leading={<Button className="back-button" type="button" onClick={onOpenMain}>{t('common.back')}</Button>}
        />
        <div className="list-stack">
          <ListItem
            active
            type="button"
            onClick={onOpenMain}
            title={t('groups.entryTitle')}
            subtitle={t('groups.entryHint')}
            leading={<span className="avatar-badge avatar-muted"><Bot size={18} /></span>}
          />
        </div>
      </section>
      <section className="main-pane placeholder-pane">
        <EmptyState icon={<MessageCircle size={34} />} title={t('groups.emptyTitle')} description={t('groups.emptyHint')} />
      </section>
    </>
  );
}
