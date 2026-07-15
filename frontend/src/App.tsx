import { lazy, Suspense, useState, useEffect } from 'react';
import { Bot, MessageCircle } from 'lucide-react';
import type { AppSettings, MobileView, NavKey } from './types';
import { useSettings, useProviders, useAgents } from './hooks';
import { Sidebar } from './components/layout';
import { ChatPanel } from './features/chat/ChatPanel';
import { Button, EmptyState, ListItem, PaneHeader, useFeedback } from './components/ui';
import { isLocale, useI18n } from './i18n';

const AgentsPanel = lazy(() => import('./features/agents/AgentsPanel').then((module) => ({ default: module.AgentsPanel })));
const ToolsPanel = lazy(() => import('./features/tools/ToolsPanel').then((module) => ({ default: module.ToolsPanel })));
const SettingsPanel = lazy(() => import('./features/settings/SettingsPanel').then((module) => ({ default: module.SettingsPanel })));

type SettingsSection = 'runtime' | 'providers';

export function App() {
  const { t, locale, setLocaleStateOnly } = useI18n();
  const { toast } = useFeedback();
  const [activeNav, setActiveNav] = useState<NavKey>('chat');
  const [settingsSection, setSettingsSection] = useState<SettingsSection>('runtime');
  const [mobileView, setMobileView] = useState<MobileView>('list');

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
    try {
      const [settings] = await Promise.all([refreshSettings(), refreshProviders(), refreshAgents()]);
      if (isLocale(settings.language)) {
        setLocaleStateOnly(settings.language);
      }
    } catch (cause) {
      toast.error(cause instanceof Error ? cause.message : t('error.unknown'));
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
    setMobileView('list');
  }

  function openProviderSettings() {
    setSettingsSection('providers');
    setActiveNav('settings');
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
      {activeNav === 'chat' && (
        <ChatPanel
          agents={agents}
          providers={providers}
          activeAgentId={activeAgentId}
          onAgentChange={setActiveAgentId}
          onAgentEdit={editAgent}
          onOpenMain={openMainPane}
          onBackToList={openListPane}
          settings={settingsForm}
        />
      )}
      <Suspense fallback={<PanelLoading label={t('sidebar.loading')} />}>
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
          onBackToList={openListPane}
        />
      )}
      {activeNav === 'groups' && (
        <GroupsPlaceholder
          onOpenMain={openMainPane}
          onBackToList={openListPane}
        />
      )}
      {activeNav === 'tools' && (
        <ToolsPanel
          agents={agents}
          onOpenMain={openMainPane}
          onBackToList={openListPane}
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
          onBackToList={openListPane}
        />
      )}
      </Suspense>
    </main>
  );
}

function PanelLoading({ label }: { label: string }) {
  return (
    <section className="main-pane panel-loading" role="status" aria-live="polite">
      <span className="panel-loading-spinner" aria-hidden="true" />
      <span>{label}</span>
    </section>
  );
}

function GroupsPlaceholder({
  onOpenMain,
  onBackToList,
}: {
  onOpenMain: () => void;
  onBackToList: () => void;
}) {
  const { t } = useI18n();

  return (
    <>
      <section className="context-pane groups-list-pane">
        <PaneHeader
          className="mobile-pane-header"
          title={t('groups.title')}
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
        <Button className="main-back-button placeholder-back-button" variant="ghost" type="button" onClick={onBackToList}>{t('common.back')}</Button>
        <EmptyState icon={<MessageCircle size={34} />} title={t('groups.emptyTitle')} description={t('groups.emptyHint')} />
      </section>
    </>
  );
}
