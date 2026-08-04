import { lazy, Suspense, useCallback, useState, useEffect } from 'react';
import { RefreshCw } from 'lucide-react';
import type { AppSettings, MobileView, NavKey } from './types';
import { useSettings, useProviders, useAgents } from './hooks';
import { Sidebar } from './components/layout';
import { AppErrorBoundary } from './components/AppErrorBoundary';
import { ChatPanel } from './features/chat/ChatPanel';
import { Button, useFeedback } from './components/ui';
import { isLocale, useI18n } from './i18n';
import { userErrorMessage } from './utils/userError';

const AgentsPanel = lazy(() => import('./features/agents/AgentsPanel').then((module) => ({ default: module.AgentsPanel })));
const ToolsPanel = lazy(() => import('./features/tools/ToolsPanel').then((module) => ({ default: module.ToolsPanel })));
const SettingsPanel = lazy(() => import('./features/settings/SettingsPanel').then((module) => ({ default: module.SettingsPanel })));
const MonitorPanel = lazy(() => import('./features/monitor/MonitorPanel').then((module) => ({ default: module.MonitorPanel })));

type SettingsSection = 'runtime' | 'agent' | 'providers' | 'proxies';

export function App() {
  const { t, locale, setLocale } = useI18n();
  const { toast } = useFeedback();
  const [activeNav, setActiveNav] = useState<NavKey>('chat');
  const [settingsSection, setSettingsSection] = useState<SettingsSection>('runtime');
  const [mobileView, setMobileView] = useState<MobileView>('list');
  const [initialLoadError, setInitialLoadError] = useState('');
  const [isRefreshingInitialData, setIsRefreshingInitialData] = useState(false);

  useEffect(() => {
    const desktopQuery = window.matchMedia('(min-width: 921px)');
    const syncResponsiveState = () => {
      if (desktopQuery.matches) {
        setMobileView('list');
      }
    };
    syncResponsiveState();
    desktopQuery.addEventListener('change', syncResponsiveState);
    return () => {
      desktopQuery.removeEventListener('change', syncResponsiveState);
    };
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
    editAgentProfile,
    removeAgent,
  } = useAgents();

  const refreshAll = useCallback(async () => {
    setIsRefreshingInitialData(true);
    setInitialLoadError('');
    const [settingsResult, providersResult, agentsResult] = await Promise.allSettled([
      refreshSettings(),
      refreshProviders(),
      refreshAgents(),
    ]);
    if (settingsResult.status === 'fulfilled' && isLocale(settingsResult.value.language)) {
      setLocale(settingsResult.value.language);
    }
    const errors = [settingsResult, providersResult, agentsResult]
      .filter((result): result is PromiseRejectedResult => result.status === 'rejected')
      .map(() => t('error.unknown'));
    if (errors.length > 0) {
      const message = userErrorMessage(new Error(errors.join(' ')), {
        summary: t('error.initialLoadFailed'),
        recovery: t('error.retryConnection'),
      });
      setInitialLoadError(message);
      toast.error(message);
    }
    setIsRefreshingInitialData(false);
  }, [refreshAgents, refreshProviders, refreshSettings, setLocale, t, toast]);

  async function saveSettingsWithLocale(payload: AppSettings) {
    const next = { ...payload, language: isLocale(payload.language) ? payload.language : locale };
    const updated = await saveSettings(next);
    if (isLocale(updated.language)) {
      setLocale(updated.language);
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

  useEffect(() => {
    void refreshAll();
  }, [refreshAll]);

  function openIpProxySettings() {
    setSettingsSection('proxies');
    setActiveNav('settings');
    setMobileView('main');
  }

  function openAgentCreation() {
    setActiveAgentId('');
    setActiveNav('agents');
    setMobileView('main');
  }

  function openAgentSettings(id: string) {
    setActiveAgentId(id);
    setActiveNav('agents');
    setMobileView('main');
  }

  function openMainPane() {
    setMobileView('main');
  }

  function openListPane() {
    setMobileView('list');
  }

  useEffect(() => {
    const openSettingsShortcut = (event: globalThis.KeyboardEvent) => {
      if (
        !(event.ctrlKey || event.metaKey)
        || event.key !== ','
        || document.querySelector('[role="dialog"]')
      ) {
        return;
      }
      event.preventDefault();
      setActiveNav('settings');
      setSettingsSection('runtime');
      setMobileView('list');
    };
    window.addEventListener('keydown', openSettingsShortcut);
    return () => window.removeEventListener('keydown', openSettingsShortcut);
  }, []);

  return (
    <main className={`workspace-shell mobile-${mobileView}`}>
      {initialLoadError ? (
        <div className="app-load-error" role="alert">
          <span>{initialLoadError}</span>
          <Button
            variant="secondary"
            type="button"
            disabled={isRefreshingInitialData}
            onClick={() => void refreshAll()}
            icon={<RefreshCw size={15} className={isRefreshingInitialData ? 'is-spinning' : ''} />}
          >
            {t('common.retry')}
          </Button>
        </div>
      ) : null}
      <Sidebar
        activeNav={activeNav}
        settings={settingsForm}
        onNavChange={handleNavChange}
        onSettingsSave={saveSettingsWithLocale}
      />
      <AppErrorBoundary scope="feature" resetKey={`${activeNav}:${activeAgentId}`}>
        {activeNav === 'chat' && (
          <ChatPanel
            agents={agents}
            providers={providers}
            activeAgentId={activeAgentId}
            onAgentChange={setActiveAgentId}
            onAgentEdit={editAgent}
            onOpenAgentSettings={openAgentSettings}
            onOpenMain={openMainPane}
            onBackToList={openListPane}
            onCreateAgent={openAgentCreation}
            settings={settingsForm}
          />
        )}
        <Suspense fallback={<PanelLoading label={t('sidebar.loading')} />}>
        {activeNav === 'monitor' && (
          <MonitorPanel
            agents={agents}
            providers={providers}
            settings={settingsForm}
            onSettingsSave={saveSettingsWithLocale}
            onOpenMain={openMainPane}
            onBackToList={openListPane}
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
            onAgentProfileSave={editAgentProfile}
            onAgentRemove={removeAgent}
            onOpenProviderSettings={openProviderSettings}
            onOpenIpProxies={openIpProxySettings}
            onOpenMain={openMainPane}
            onBackToList={openListPane}
          />
        )}
        {activeNav === 'tools' && (
          <ToolsPanel
            agents={agents}
            onOpenIpProxies={openIpProxySettings}
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
      </AppErrorBoundary>
    </main>
  );
}

function PanelLoading({ label }: { label: string }) {
  return (
    <section className="main-pane panel-loading" role="status" aria-live="polite" aria-busy="true">
      <span className="panel-loading-spinner" aria-hidden="true" />
      <span>{label}</span>
    </section>
  );
}

