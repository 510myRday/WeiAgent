import { lazy, Suspense, useCallback, useMemo, useRef, useState, useEffect } from 'react';
import { RefreshCw } from 'lucide-react';
import type { AppSettings, MobileView, NavKey, SettingsSection } from './types';
import { useSettings, useProviders, useAgents, useAgentActivity } from './hooks';
import { Sidebar } from './components/layout';
import { AppErrorBoundary } from './components/AppErrorBoundary';
import { ChatPanel } from './features/chat/ChatPanel';
import { Button, useFeedback } from './components/ui';
import { isLocale, useI18n } from './i18n';
import { userErrorMessage } from './utils/userError';
import { sortAgentsByRecentConversation } from './utils/agentSorting';

const AgentsPanel = lazy(() => import('./features/agents/AgentsPanel').then((module) => ({ default: module.AgentsPanel })));
const ToolsPanel = lazy(() => import('./features/tools/ToolsPanel').then((module) => ({ default: module.ToolsPanel })));
const SkillsPanel = lazy(() => import('./features/tools/SkillsPanel').then((module) => ({ default: module.SkillsPanel })));
const SettingsPanel = lazy(() => import('./features/settings/SettingsPanel').then((module) => ({ default: module.SettingsPanel })));
const MonitorPanel = lazy(() => import('./features/monitor/MonitorPanel').then((module) => ({ default: module.MonitorPanel })));
const SchedulingPanel = lazy(() => import('./features/scheduling/SchedulingPanel').then((module) => ({ default: module.SchedulingPanel })));


export function App() {
  const { t, locale, setLocale } = useI18n();
  const { toast } = useFeedback();
  const [activeNav, setActiveNav] = useState<NavKey>('chat');
  const [settingsSection, setSettingsSection] = useState<SettingsSection>('runtime');
  const [mobileView, setMobileView] = useState<MobileView>('list');
  const [pendingConversationId, setPendingConversationId] = useState('');
  const [chatUnreadCount, setChatUnreadCount] = useState(0);
  const [initialLoadError, setInitialLoadError] = useState('');
  const [isRefreshingInitialData, setIsRefreshingInitialData] = useState(false);
  const hasRestoredInitialChatSelectionRef = useRef(false);
  const hasUserSelectedAgentRef = useRef(false);

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
  const agentIds = useMemo(() => agents.map((agent) => agent.id), [agents]);
  const {
    latestConversationUpdatedAtByAgent,
    isReady: isAgentActivityReady,
    touchAgentActivity,
  } = useAgentActivity(agentIds);

  const handleAgentChange = useCallback((id: string) => {
    hasUserSelectedAgentRef.current = true;
    setActiveAgentId(id);
  }, [setActiveAgentId]);

  useEffect(() => {
    if (
      hasRestoredInitialChatSelectionRef.current
      || hasUserSelectedAgentRef.current
      || !isAgentActivityReady
      || agents.length === 0
    ) return;

    const preferredAgent = sortAgentsByRecentConversation(
      agents,
      latestConversationUpdatedAtByAgent,
    )[0];
    if (preferredAgent) setActiveAgentId(preferredAgent.id);
    hasRestoredInitialChatSelectionRef.current = true;
  }, [agents, isAgentActivityReady, latestConversationUpdatedAtByAgent, setActiveAgentId]);

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
    const initialResults: Array<[string, PromiseSettledResult<unknown>]> = [
      ['settings', settingsResult],
      ['providers', providersResult],
      ['agents', agentsResult],
    ];
    const errors = initialResults
      .filter((entry): entry is [string, PromiseRejectedResult] => entry[1].status === 'rejected')
      .map(([name, result]) => {
        const detail = result.reason instanceof Error ? result.reason.message.trim() : '';
        return detail ? `${name}: ${detail}` : `${name}: ${t('error.unknown')}`;
      });
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
    hasUserSelectedAgentRef.current = true;
    setActiveAgentId('');
    setActiveNav('agents');
    setMobileView('main');
  }

  function openAgentSettings(id: string) {
    hasUserSelectedAgentRef.current = true;
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

  function openConversation(agentId: string, conversationId: string) {
    hasUserSelectedAgentRef.current = true;
    setActiveAgentId(agentId);
    setPendingConversationId(conversationId);
    setActiveNav('chat');
    setMobileView('main');
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
        chatUnreadCount={chatUnreadCount}
        settings={settingsForm}
        onNavChange={handleNavChange}
        onSettingsSave={saveSettingsWithLocale}
      />
      <AppErrorBoundary scope="feature" resetKey={`${activeNav}:${activeAgentId}`}>
        <ChatPanel
          agents={agents}
          providers={providers}
          isVisible={activeNav === 'chat'}
          activeAgentId={activeAgentId}
          onAgentChange={handleAgentChange}
          onAgentEdit={editAgent}
          onProviderEdit={editProvider}
          onOpenAgentSettings={openAgentSettings}
          onOpenMain={openMainPane}
          onBackToList={openListPane}
          onCreateAgent={openAgentCreation}
          settings={settingsForm}
          openConversationId={pendingConversationId}
          onConversationOpened={() => setPendingConversationId('')}
          latestConversationUpdatedAtByAgent={latestConversationUpdatedAtByAgent}
          onAgentActivity={touchAgentActivity}
          onUnreadCountChange={setChatUnreadCount}
        />
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
        {activeNav === 'scheduling' && (
          <SchedulingPanel
            agents={agents}
            providers={providers}
            onOpenMain={openMainPane}
            onBackToList={openListPane}
            onOpenConversation={openConversation}
          />
        )}
        {activeNav === 'agents' && (
          <AgentsPanel
            agents={agents}
            providers={providers}
            activeAgentId={activeAgentId}
            isLoadingAgents={isLoadingAgents}
            onAgentChange={handleAgentChange}
            onAgentAdd={addAgent}
            onAgentProfileSave={editAgentProfile}
            onAgentRemove={removeAgent}
            onOpenProviderSettings={openProviderSettings}
            onOpenIpProxies={openIpProxySettings}
            onOpenMain={openMainPane}
            onBackToList={openListPane}
            latestConversationUpdatedAtByAgent={latestConversationUpdatedAtByAgent}
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
        {activeNav === 'skills' && (
          <SkillsPanel
            onOpenMain={openMainPane}
            onBackToList={openListPane}
            onOpenIpProxies={openIpProxySettings}
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

