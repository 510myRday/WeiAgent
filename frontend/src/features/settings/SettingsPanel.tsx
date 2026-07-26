import { useState, useEffect, useRef, FormEvent } from 'react';
import {
  ArrowLeft,
  KeyRound,
  Network,
  Plus,
  Server,
  Brain,
  Trash2,
} from 'lucide-react';

import type {
  AppSettings,
  IpProxy,
  ModelConfig,
  ModelProvider,
  ModelProviderPayload,
  ProviderType,
} from '../../types';
import { fetchModels, fetchModelsForProvider, listIpProxies } from '../../api';
import { useBeforeUnloadGuard } from '../../hooks';
import { useI18n } from '../../i18n';
import { PanelShell } from '../../components/layout';
import { AutoSaveIndicator, Button, FormField, ListItem, PaneHeader, useFeedback } from '../../components/ui';
import { userErrorMessage } from '../../utils/userError';
import { sortByName } from '../../utils/sort';
import { IpProxySettings } from './IpProxySettings';
import { ProxyPickerDialog } from './web-search/ProxyPickerDialog';
import {
  clearNewProviderDraft,
  loadNewProviderDraft,
  saveNewProviderDraft,
} from './settings-panel/newProviderDraft';
import { protocolPresetByType, providerTemplateById } from './settings-panel/providerTemplates';
import type { ProviderTemplateId } from './settings-panel/providerTemplates';
import {
  PROVIDER_AUTO_SAVE_DELAY_MS,
  PROVIDER_AUTO_SAVE_MAX_RETRY_DELAY_MS,
  PROVIDER_AUTO_SAVE_RETRY_DELAY_MS,
  RUNTIME_AUTO_SAVE_DELAY_MS,
  RUNTIME_AUTO_SAVE_MAX_RETRY_DELAY_MS,
  RUNTIME_AUTO_SAVE_RETRY_DELAY_MS,
  buildProviderPayload,
  defaultProviderForm,
  formFromSerializedSnapshot,
  isValidRuntimeSettings,
  parseExtraBodyText,
  parseModels,
  providerTemplateToForm,
  providerToForm,
  serializeProviderForm,
  serializeRuntimeSettings,
  syncModelConfigs,
} from './settings-panel/settingsPanelUtils';
import type {
  PendingProviderSave,
  PendingRuntimeSave,
  ProviderAutoSaveStatus,
  ProviderForm,
  RuntimeAutoSaveStatus,
} from './settings-panel/settingsPanelUtils';
import { AgentSection, RuntimeSection } from './settings-panel/RuntimeSettingsSections';
import { ProviderEditorForm } from './settings-panel/ProviderEditorForm';
import { ProviderTemplateDialog } from './settings-panel/ProviderTemplateDialog';

interface SettingsPanelProps {
  settingsForm: AppSettings;
  onSettingsFormChange: (form: AppSettings) => void;
  onSettingsSave: (payload: AppSettings) => Promise<AppSettings>;
  providers: ModelProvider[];
  activeProviderId: string;
  initialSection: SettingsSection;
  onProviderChange: (id: string) => void;
  onProviderAdd: (payload: ModelProviderPayload) => Promise<ModelProvider>;
  onProviderEdit: (id: string, payload: Partial<ModelProviderPayload>) => Promise<ModelProvider>;
  onProviderRemove: (id: string) => Promise<void>;
  onOpenMain: () => void;
  onBackToList: () => void;
}

type SettingsSection = 'runtime' | 'agent' | 'providers' | 'proxies';

export function SettingsPanel({
  settingsForm,
  onSettingsFormChange,
  onSettingsSave,
  providers,
  activeProviderId,
  initialSection,
  onProviderChange,
  onProviderAdd,
  onProviderEdit,
  onProviderRemove,
  onOpenMain,
  onBackToList,
}: SettingsPanelProps) {
  const { t } = useI18n();
  const { confirm, toast } = useFeedback();
  const [activeSection, setActiveSection] = useState<SettingsSection>('runtime');
  const [providerForm, setProviderForm] = useState(
    () => loadNewProviderDraft()?.form ?? defaultProviderForm,
  );
  const [isSubmittingProvider, setIsSubmittingProvider] = useState(false);
  const [fetchingModels, setFetchingModels] = useState(false);
  const [showApiKey, setShowApiKey] = useState(false);
  const [runtimeDirty, setRuntimeDirty] = useState(false);
  const [autoSaveStatus, setAutoSaveStatus] = useState<RuntimeAutoSaveStatus>('idle');
  const [providerDirty, setProviderDirty] = useState(false);
  const [providerAutoSaveStatus, setProviderAutoSaveStatus] = useState<ProviderAutoSaveStatus>('idle');
  const [advancedOpen, setAdvancedOpen] = useState(
    () => Boolean(loadNewProviderDraft()?.form.extraBodyText.trim()),
  );
  const [providerTemplateDialogOpen, setProviderTemplateDialogOpen] = useState(false);
  const [selectedProviderTemplateId, setSelectedProviderTemplateId] = useState<ProviderTemplateId>(
    () => loadNewProviderDraft()?.templateId ?? 'custom',
  );
  const [pendingProviderTemplateId, setPendingProviderTemplateId] = useState<ProviderTemplateId | null>(null);
  const [proxyCount, setProxyCount] = useState(0);
  const [ipProxies, setIpProxies] = useState<IpProxy[]>([]);
  const [isLoadingIpProxies, setIsLoadingIpProxies] = useState(false);
  const [providerProxyPickerOpen, setProviderProxyPickerOpen] = useState(false);

  const latestSerializedRef = useRef('');
  const persistedSerializedRef = useRef('');
  const pendingSaveRef = useRef<PendingRuntimeSave | null>(null);
  const saveInFlightRef = useRef(false);
  const saveRetryCountRef = useRef(0);
  const saveTimerRef = useRef<number | undefined>(undefined);
  const retryTimerRef = useRef<number | undefined>(undefined);
  const isMountedRef = useRef(true);
  const onSettingsSaveRef = useRef(onSettingsSave);
  const providerLoadedIdRef = useRef<string | null>(null);
  const activeProviderIdRef = useRef(activeProviderId);
  const providerLatestSerializedByIdRef = useRef(new Map<string, string>());
  const providerPersistedSerializedByIdRef = useRef(new Map<string, string>());
  const pendingProviderSaveRef = useRef<PendingProviderSave | null>(null);
  const providerSaveInFlightRef = useRef(false);
  const providerSaveRetryCountRef = useRef(0);
  const providerSaveTimerRef = useRef<number | undefined>(undefined);
  const providerRetryTimerRef = useRef<number | undefined>(undefined);
  const onProviderEditRef = useRef(onProviderEdit);

  onSettingsSaveRef.current = onSettingsSave;
  onProviderEditRef.current = onProviderEdit;
  activeProviderIdRef.current = activeProviderId;

  useBeforeUnloadGuard(
    runtimeDirty
    || autoSaveStatus === 'pending'
    || autoSaveStatus === 'saving'
    || autoSaveStatus === 'error'
    || providerDirty
    || providerAutoSaveStatus === 'pending'
    || providerAutoSaveStatus === 'saving'
    || providerAutoSaveStatus === 'error',
  );

  const activeProvider = providers.find((provider) => provider.id === activeProviderId);
  const selectedProviderTemplate = providerTemplateById[selectedProviderTemplateId];
  const providerConnectionLocked = !activeProvider && selectedProviderTemplate.fixed;

  useEffect(() => {
    isMountedRef.current = true;
    return () => {
      isMountedRef.current = false;
      if (saveTimerRef.current !== undefined) {
        window.clearTimeout(saveTimerRef.current);
        saveTimerRef.current = undefined;
      }
      if (retryTimerRef.current !== undefined) {
        window.clearTimeout(retryTimerRef.current);
        retryTimerRef.current = undefined;
      }
      if (providerSaveTimerRef.current !== undefined) {
        window.clearTimeout(providerSaveTimerRef.current);
        providerSaveTimerRef.current = undefined;
      }
      if (providerRetryTimerRef.current !== undefined) {
        window.clearTimeout(providerRetryTimerRef.current);
        providerRetryTimerRef.current = undefined;
      }
      void flushRuntimeSaveQueue();
      void flushProviderSaveQueue();
    };
  }, []);

  useEffect(() => {
    setActiveSection(initialSection);
  }, [initialSection]);

  // Refresh the proxy list whenever the provider page is shown so proxies
  // created on the IP proxy page are immediately selectable.
  useEffect(() => {
    if (activeSection !== 'providers') return;
    let cancelled = false;
    setIsLoadingIpProxies(true);
    listIpProxies()
      .then((next) => {
        if (!cancelled) setIpProxies(next);
      })
      .catch(() => {
        if (!cancelled) toast.warning(t('tools.webSearch.ipProxyLoadFailed'));
      })
      .finally(() => {
        if (!cancelled) setIsLoadingIpProxies(false);
      });
    return () => {
      cancelled = true;
    };
  }, [activeSection, t, toast]);

  useEffect(() => {
    if (providerLoadedIdRef.current === activeProviderId) return;
    if (activeProviderId && !activeProvider) return;
    providerLoadedIdRef.current = activeProviderId;

    if (providerSaveTimerRef.current !== undefined) {
      window.clearTimeout(providerSaveTimerRef.current);
      providerSaveTimerRef.current = undefined;
    }
    if (providerRetryTimerRef.current !== undefined) {
      window.clearTimeout(providerRetryTimerRef.current);
      providerRetryTimerRef.current = undefined;
    }

    if (!activeProvider) {
      const draft = loadNewProviderDraft();
      setProviderForm(draft?.form ?? defaultProviderForm);
      setShowApiKey(false);
      setProviderDirty(false);
      setProviderAutoSaveStatus('idle');
      setAdvancedOpen(Boolean(draft?.form.extraBodyText.trim()));
      setSelectedProviderTemplateId(draft?.templateId ?? 'custom');
      return;
    }
    const nextForm = providerToForm(activeProvider);
    const serialized = serializeProviderForm(nextForm);
    setProviderForm(nextForm);
    setShowApiKey(false);
    setProviderDirty(false);
    setProviderAutoSaveStatus('idle');
    setAdvancedOpen(Boolean(nextForm.extraBodyText.trim()));
    setSelectedProviderTemplateId('custom');
    providerLatestSerializedByIdRef.current.set(activeProvider.id, serialized);
    providerPersistedSerializedByIdRef.current.set(activeProvider.id, serialized);
  }, [activeProvider, activeProviderId]);

  useEffect(() => {
    if (!pendingProviderTemplateId || activeProviderId) return;
    const template = providerTemplateById[pendingProviderTemplateId];
    setProviderForm(providerTemplateToForm(template, template.fixed ? t(template.labelKey) : ''));
    setShowApiKey(false);
    setProviderDirty(false);
    setProviderAutoSaveStatus('idle');
    setAdvancedOpen(false);
    setSelectedProviderTemplateId(template.id);
    setPendingProviderTemplateId(null);
  }, [activeProviderId, pendingProviderTemplateId, t]);

  // Mirror the unsaved "create provider" form into a module-level draft so it
  // survives navigating to other pages before the user clicks create. The draft
  // is discarded when an existing provider is selected, a template is picked,
  // or creation succeeds.
  useEffect(() => {
    if (activeProviderId || pendingProviderTemplateId) return;
    saveNewProviderDraft({ form: providerForm, templateId: selectedProviderTemplateId });
  }, [activeProviderId, pendingProviderTemplateId, providerForm, selectedProviderTemplateId]);

  // Seed persisted snapshot once settings become valid (initial load / external refresh).
  useEffect(() => {
    if (!isValidRuntimeSettings(settingsForm)) return;
    const serialized = serializeRuntimeSettings(settingsForm);
    if (!persistedSerializedRef.current) {
      persistedSerializedRef.current = serialized;
      latestSerializedRef.current = serialized;
      setAutoSaveStatus('idle');
      return;
    }
    // External updates (avatar/language via Sidebar, successful save response) keep snapshot in sync
    // when the user is not actively editing runtime fields.
    if (!runtimeDirty && pendingSaveRef.current === null && !saveInFlightRef.current) {
      persistedSerializedRef.current = serialized;
      latestSerializedRef.current = serialized;
    }
  }, [runtimeDirty, settingsForm]);

  useEffect(() => {
    if (!runtimeDirty) return;

    if (!isValidRuntimeSettings(settingsForm)) {
      setAutoSaveStatus('pending');
      if (saveTimerRef.current !== undefined) {
        window.clearTimeout(saveTimerRef.current);
        saveTimerRef.current = undefined;
      }
      pendingSaveRef.current = null;
      return;
    }

    const serialized = serializeRuntimeSettings(settingsForm);
    if (latestSerializedRef.current !== serialized) {
      saveRetryCountRef.current = 0;
    }
    latestSerializedRef.current = serialized;

    if (persistedSerializedRef.current === serialized) {
      pendingSaveRef.current = null;
      setRuntimeDirty(false);
      setAutoSaveStatus('saved');
      return;
    }

    pendingSaveRef.current = {
      serialized,
      payload: { ...settingsForm },
    };
    setAutoSaveStatus('pending');
    if (saveTimerRef.current !== undefined) {
      window.clearTimeout(saveTimerRef.current);
    }
    saveTimerRef.current = window.setTimeout(() => {
      saveTimerRef.current = undefined;
      void flushRuntimeSaveQueue();
    }, RUNTIME_AUTO_SAVE_DELAY_MS);
  }, [runtimeDirty, settingsForm]);

  useEffect(() => {
    if (!providerDirty || !activeProviderId) return;

    const serialized = serializeProviderForm(providerForm);
    if (providerLatestSerializedByIdRef.current.get(activeProviderId) !== serialized) {
      providerSaveRetryCountRef.current = 0;
    }
    providerLatestSerializedByIdRef.current.set(activeProviderId, serialized);

    if (providerPersistedSerializedByIdRef.current.get(activeProviderId) === serialized) {
      pendingProviderSaveRef.current = null;
      setProviderDirty(false);
      setProviderAutoSaveStatus('saved');
      return;
    }

    const payload = buildProviderPayload(providerForm, false);
    if (!payload) {
      setProviderAutoSaveStatus('pending');
      if (providerSaveTimerRef.current !== undefined) {
        window.clearTimeout(providerSaveTimerRef.current);
        providerSaveTimerRef.current = undefined;
      }
      if (pendingProviderSaveRef.current?.providerId === activeProviderId) {
        pendingProviderSaveRef.current = null;
      }
      return;
    }

    pendingProviderSaveRef.current = {
      providerId: activeProviderId,
      serialized,
      payload,
    };
    setProviderAutoSaveStatus('pending');
    if (providerSaveTimerRef.current !== undefined) {
      window.clearTimeout(providerSaveTimerRef.current);
    }
    providerSaveTimerRef.current = window.setTimeout(() => {
      providerSaveTimerRef.current = undefined;
      void flushProviderSaveQueue();
    }, PROVIDER_AUTO_SAVE_DELAY_MS);
  }, [activeProviderId, providerDirty, providerForm]);

  async function flushRuntimeSaveQueue() {
    if (saveInFlightRef.current || !pendingSaveRef.current) return;
    const pending = pendingSaveRef.current;
    pendingSaveRef.current = null;
    saveInFlightRef.current = true;
    if (isMountedRef.current) {
      setAutoSaveStatus('saving');
    }

    let retryDelay = 0;
    try {
      const updated = await onSettingsSaveRef.current(pending.payload);
      const updatedSerialized = serializeRuntimeSettings(updated);
      persistedSerializedRef.current = updatedSerialized;
      saveRetryCountRef.current = 0;
      if (isMountedRef.current && latestSerializedRef.current === pending.serialized) {
        latestSerializedRef.current = updatedSerialized;
        const queuedSave = pendingSaveRef.current as PendingRuntimeSave | null;
        if (queuedSave?.serialized === pending.serialized) {
          pendingSaveRef.current = null;
        }
        if (saveTimerRef.current !== undefined) {
          window.clearTimeout(saveTimerRef.current);
          saveTimerRef.current = undefined;
        }
        setRuntimeDirty(false);
        setAutoSaveStatus('saved');
      }
    } catch (cause) {
      const attempt = saveRetryCountRef.current + 1;
      saveRetryCountRef.current = attempt;
      if (latestSerializedRef.current === pending.serialized) {
        pendingSaveRef.current = pending;
        retryDelay = Math.min(
          RUNTIME_AUTO_SAVE_MAX_RETRY_DELAY_MS,
          RUNTIME_AUTO_SAVE_RETRY_DELAY_MS * (2 ** Math.min(attempt - 1, 5)),
        );
      }
      if (isMountedRef.current) {
        setAutoSaveStatus('error');
        if (attempt === 1) {
          toast.error(userErrorMessage(cause, {
            summary: t('error.settingsSaveFailed'),
            recovery: t('error.retryConnection'),
          }));
        }
      }
    } finally {
      saveInFlightRef.current = false;
      if (pendingSaveRef.current) {
        if (retryTimerRef.current !== undefined) {
          window.clearTimeout(retryTimerRef.current);
        }
        retryTimerRef.current = window.setTimeout(() => {
          retryTimerRef.current = undefined;
          void flushRuntimeSaveQueue();
        }, retryDelay);
      }
    }
  }

  function retryRuntimeSaveNow() {
    if (saveTimerRef.current !== undefined) {
      window.clearTimeout(saveTimerRef.current);
      saveTimerRef.current = undefined;
    }
    if (retryTimerRef.current !== undefined) {
      window.clearTimeout(retryTimerRef.current);
      retryTimerRef.current = undefined;
    }
    void flushRuntimeSaveQueue();
  }

  async function flushProviderSaveQueue() {
    if (providerSaveInFlightRef.current || !pendingProviderSaveRef.current) return;
    const pending = pendingProviderSaveRef.current;
    pendingProviderSaveRef.current = null;
    providerSaveInFlightRef.current = true;
    if (isMountedRef.current) {
      setIsSubmittingProvider(true);
      if (activeProviderIdRef.current === pending.providerId) {
        setProviderAutoSaveStatus('saving');
      }
    }

    let retryDelay = 0;
    try {
      await onProviderEditRef.current(pending.providerId, pending.payload);
      providerSaveRetryCountRef.current = 0;

      // Restore from the draft snapshot we just saved. Do NOT rebuild from the API
      // response: that used to replace the textarea with placeholder text when
      // extra_body came back as null/{} or with different formatting.
      if (
        isMountedRef.current
        && activeProviderIdRef.current === pending.providerId
        && providerLatestSerializedByIdRef.current.get(pending.providerId) === pending.serialized
      ) {
        const nextForm = formFromSerializedSnapshot(pending.serialized);
        const nextSerialized = serializeProviderForm(nextForm);
        providerLatestSerializedByIdRef.current.set(pending.providerId, nextSerialized);
        providerPersistedSerializedByIdRef.current.set(pending.providerId, nextSerialized);

        const queuedSave = pendingProviderSaveRef.current as PendingProviderSave | null;
        if (queuedSave?.serialized === pending.serialized) {
          pendingProviderSaveRef.current = null;
        }
        if (providerSaveTimerRef.current !== undefined) {
          window.clearTimeout(providerSaveTimerRef.current);
          providerSaveTimerRef.current = undefined;
        }
        setProviderForm(nextForm);
        setShowApiKey(false);
        setProviderDirty(false);
        setProviderAutoSaveStatus('saved');
      } else {
        providerPersistedSerializedByIdRef.current.set(pending.providerId, pending.serialized);
      }
    } catch (cause) {
      const attempt = providerSaveRetryCountRef.current + 1;
      providerSaveRetryCountRef.current = attempt;
      if (providerLatestSerializedByIdRef.current.get(pending.providerId) === pending.serialized) {
        pendingProviderSaveRef.current = pending;
        retryDelay = Math.min(
          PROVIDER_AUTO_SAVE_MAX_RETRY_DELAY_MS,
          PROVIDER_AUTO_SAVE_RETRY_DELAY_MS * (2 ** Math.min(attempt - 1, 5)),
        );
      }
      if (isMountedRef.current && activeProviderIdRef.current === pending.providerId) {
        setProviderAutoSaveStatus('error');
        if (attempt === 1) {
          toast.error(userErrorMessage(cause, {
            summary: t('error.providerSaveFailed'),
            recovery: t('error.checkProviderFields'),
          }));
        }
      }
    } finally {
      providerSaveInFlightRef.current = false;
      if (isMountedRef.current) {
        setIsSubmittingProvider(false);
      }
      if (pendingProviderSaveRef.current) {
        if (providerRetryTimerRef.current !== undefined) {
          window.clearTimeout(providerRetryTimerRef.current);
        }
        providerRetryTimerRef.current = window.setTimeout(() => {
          providerRetryTimerRef.current = undefined;
          void flushProviderSaveQueue();
        }, retryDelay);
      }
    }
  }

  function retryProviderSaveNow() {
    if (providerSaveTimerRef.current !== undefined) {
      window.clearTimeout(providerSaveTimerRef.current);
      providerSaveTimerRef.current = undefined;
    }
    if (providerRetryTimerRef.current !== undefined) {
      window.clearTimeout(providerRetryTimerRef.current);
      providerRetryTimerRef.current = undefined;
    }
    void flushProviderSaveQueue();
  }

  function patchRuntimeSettings(patch: Partial<AppSettings>) {
    onSettingsFormChange({ ...settingsForm, ...patch });
    setRuntimeDirty(true);
  }

  function changeProviderForm(updater: (current: ProviderForm) => ProviderForm) {
    setProviderForm(updater);
    if (activeProviderIdRef.current) {
      setProviderDirty(true);
    }
  }

  function handleProviderChange(id: string) {
    if (id) {
      clearNewProviderDraft();
    }
    if (providerSaveTimerRef.current !== undefined) {
      window.clearTimeout(providerSaveTimerRef.current);
      providerSaveTimerRef.current = undefined;
      void flushProviderSaveQueue();
    }
    onProviderChange(id);
  }

  function handleProviderTemplateSelect(templateId: ProviderTemplateId) {
    setProviderTemplateDialogOpen(false);
    clearNewProviderDraft();
    setPendingProviderTemplateId(templateId);
    if (activeProviderId) {
      handleProviderChange('');
    }
  }

  async function handleProviderSubmit(e: FormEvent<HTMLFormElement>) {
    e.preventDefault();
    if (activeProvider) {
      retryProviderSaveNow();
      return;
    }
    const payload = buildProviderPayload(providerForm, true);
    if (!payload) {
      toast.warning(t('error.providerValidation'));
      return;
    }
    setIsSubmittingProvider(true);
    try {
      await onProviderAdd(payload);
      clearNewProviderDraft();
      toast.success(t('notification.providerCreated'));
      setProviderForm((current) => ({ ...current, api_key: '' }));
    } catch (cause) {
      toast.error(userErrorMessage(cause, {
        summary: t('error.providerSaveFailed'),
        recovery: t('error.checkProviderFields'),
      }));
    } finally {
      setIsSubmittingProvider(false);
    }
  }

  async function handleDeleteProvider() {
    if (!activeProvider) return;
    const confirmed = await confirm({
      title: t('provider.delete'),
      message: t('provider.deleteConfirm', { name: activeProvider.name }),
      confirmLabel: t('common.delete'),
      tone: 'danger',
    });
    if (!confirmed) return;
    try {
      await onProviderRemove(activeProvider.id);
      toast.success(t('notification.providerDeleted'));
    } catch (cause) {
      toast.error(userErrorMessage(cause, {
        summary: t('error.providerDeleteFailed'),
        recovery: t('error.retryConnection'),
      }));
    }
  }

  async function handleFetchModels() {
    const parsedExtraBody = parseExtraBodyText(providerForm.extraBodyText);
    if (parsedExtraBody.status === 'invalid') {
      toast.warning(t('error.invalidJson'));
      return;
    }
    const extraBody = parsedExtraBody.value;
    const baseUrl = providerForm.base_url.trim();
    const apiKey = providerForm.api_key.trim();
    const canUseStoredKey = Boolean(
      activeProvider
      && activeProvider.has_api_key
      && !apiKey
      && baseUrl === activeProvider.base_url
      && providerForm.provider_type === activeProvider.provider_type,
    );
    if (!baseUrl || (!apiKey && !canUseStoredKey)) {
      toast.warning(t('provider.fetchModelsMissingConfig'));
      return;
    }
    setFetchingModels(true);
    try {
      const models = canUseStoredKey && activeProvider
        ? await fetchModels(activeProvider.id)
        : await fetchModelsForProvider({
          provider_type: providerForm.provider_type,
          base_url: baseUrl,
          api_key: apiKey,
          extra_body: extraBody,
          proxy_url: providerForm.proxy_url,
        });
      changeProviderForm((current) => ({
        ...current,
        modelsText: models.join('\n'),
        modelConfigs: syncModelConfigs(models, current.modelConfigs),
      }));
      toast.success(t('notification.modelsLoaded', { count: models.length }));
    } catch (cause) {
      toast.error(userErrorMessage(cause, {
        summary: t('model.loadFailed'),
      }));
    } finally {
      setFetchingModels(false);
    }
  }

  function handleProtocolChange(providerType: ProviderType) {
    changeProviderForm((current) => {
      const currentDefault = protocolPresetByType[current.provider_type].defaultBaseUrl;
      const nextDefault = protocolPresetByType[providerType].defaultBaseUrl;
      return {
        ...current,
        provider_type: providerType,
        base_url: !current.base_url.trim() || current.base_url === currentDefault
          ? nextDefault
          : current.base_url,
      };
    });
  }

  function handleModelsTextChange(modelsText: string) {
    changeProviderForm((current) => ({
      ...current,
      modelsText,
      modelConfigs: syncModelConfigs(parseModels(modelsText), current.modelConfigs),
    }));
  }

  function patchModelConfig(modelId: string, patch: Partial<ModelConfig>) {
    changeProviderForm((current) => ({
      ...current,
      modelConfigs: current.modelConfigs.map((config) => (
        config.model_id === modelId ? { ...config, ...patch } : config
      )),
    }));
  }

  function handleExtraBodyChange(nextValue: string) {
    changeProviderForm((current) => ({ ...current, extraBodyText: nextValue }));
    if (nextValue.trim()) {
      setAdvancedOpen(true);
    }
  }

  return (
    <>
      <PanelShell
        className="settings-panel"
        sidebar={(
          <div className="context-pane settings-nav">
            <PaneHeader
              className="mobile-pane-header"
              title={t('settings.areaTitle')}
            />
            <div className="list-stack">
              <ListItem
                type="button"
                active={activeSection === 'runtime'}
                title={t('settings.title')}
                subtitle={t('settings.description')}
                leading={<Server size={18} />}
                onClick={() => { setActiveSection('runtime'); onOpenMain(); }}
              />
              <ListItem
                type="button"
                active={activeSection === 'agent'}
                title={t('settings.agentTitle')}
                subtitle={t('settings.agentDescription')}
                leading={<Brain size={18} />}
                onClick={() => { setActiveSection('agent'); onOpenMain(); }}
              />
              <ListItem
                type="button"
                active={activeSection === 'providers'}
                title={t('provider.title')}
                subtitle={t('provider.description')}
                leading={<KeyRound size={18} />}
                onClick={() => { setActiveSection('providers'); onOpenMain(); }}
              />
              <ListItem
                type="button"
                active={activeSection === 'proxies'}
                title={t('ipProxy.title')}
                subtitle={t('ipProxy.description')}
                leading={<Network size={18} />}
                onClick={() => { setActiveSection('proxies'); onOpenMain(); }}
              />
            </div>
          </div>
        )}
        main={(
          <div className={`main-pane config-main${activeSection === 'proxies' ? ' ip-proxy-main' : ''}`}>
          <PaneHeader
            className="config-page-header"
            leading={<Button className="main-back-button" variant="ghost" type="button" onClick={onBackToList} icon={<ArrowLeft size={17} />}>{t('common.back')}</Button>}
            eyebrow={t('settings.areaTitle')}
            title={activeSection === 'runtime'
              ? t('settings.title')
              : activeSection === 'agent'
                ? t('settings.agentTitle')
                : activeSection === 'providers' ? (
                  <span className="provider-page-title">
                    {t('provider.title')}
                    <span className="mini-status-badge provider-title-count">
                      {t('provider.configuredCount', { count: providers.length })}
                    </span>
                  </span>
                ) : (
                  <span className="provider-page-title">
                    {t('ipProxy.title')}
                    <span className="mini-status-badge provider-title-count">
                      {t('ipProxy.configuredCount', { count: proxyCount })}
                    </span>
                  </span>
                )}
            description={activeSection === 'runtime'
              ? t('settings.description')
              : activeSection === 'agent'
                ? t('settings.agentDescription')
                : activeSection === 'providers'
                  ? t('provider.description')
                  : t('ipProxy.description')}
            actions={activeSection === 'providers'
              ? <AutoSaveIndicator status={providerAutoSaveStatus} t={t} onRetry={retryProviderSaveNow} />
              : activeSection === 'proxies'
                ? undefined
                : <AutoSaveIndicator status={autoSaveStatus} t={t} onRetry={retryRuntimeSaveNow} />}
          />

          {activeSection === 'runtime' ? (
            <RuntimeSection settingsForm={settingsForm} onPatch={patchRuntimeSettings} />
          ) : activeSection === 'agent' ? (
            <AgentSection settingsForm={settingsForm} providers={providers} onPatch={patchRuntimeSettings} />
          ) : activeSection === 'proxies' ? (
            <IpProxySettings onCountChange={setProxyCount} />
          ) : (
            <div className="provider-page">
              <div className="form-card provider-picker-bar">
                <FormField label={t('provider.currentProvider')} htmlFor="provider-picker">
                  <select
                    id="provider-picker"
                    value={activeProviderId}
                    onChange={(event) => handleProviderChange(event.target.value)}
                  >
                    {!activeProvider ? (
                      <option value="">{t('provider.newTitle')}</option>
                    ) : null}
                    {sortByName(providers).map((provider) => (
                      <option key={provider.id} value={provider.id}>
                        {provider.name}
                      </option>
                    ))}
                  </select>
                </FormField>
                <div className="provider-picker-actions">
                  <Button
                    variant="secondary"
                    type="button"
                    onClick={() => {
                      setProviderTemplateDialogOpen(true);
                    }}
                    icon={<Plus size={16} />}
                  >
                    {t('provider.create')}
                  </Button>
                  {activeProvider ? (
                    <Button
                      variant="danger"
                      type="button"
                      disabled={isSubmittingProvider}
                      onClick={handleDeleteProvider}
                      icon={<Trash2 size={16} />}
                    >
                      {t('common.delete')}
                    </Button>
                  ) : null}
                </div>
              </div>

              <ProviderEditorForm
                providerForm={providerForm}
                activeProvider={activeProvider}
                isSubmittingProvider={isSubmittingProvider}
                fetchingModels={fetchingModels}
                showApiKey={showApiKey}
                advancedOpen={advancedOpen}
                providerConnectionLocked={providerConnectionLocked}
                ipProxies={ipProxies}
                onSubmit={handleProviderSubmit}
                onToggleShowApiKey={() => setShowApiKey((visible) => !visible)}
                onAdvancedOpenChange={setAdvancedOpen}
                onNameChange={(name) => changeProviderForm((current) => ({ ...current, name }))}
                onProtocolChange={handleProtocolChange}
                onBaseUrlChange={(base_url) => changeProviderForm((current) => ({ ...current, base_url }))}
                onApiKeyChange={(api_key) => changeProviderForm((current) => ({ ...current, api_key }))}
                onModelsTextChange={handleModelsTextChange}
                onPatchModelConfig={patchModelConfig}
                onExtraBodyChange={handleExtraBodyChange}
                onFetchModels={handleFetchModels}
                onOpenProxyPicker={() => setProviderProxyPickerOpen(true)}
              />
            </div>
          )}
          </div>
        )}
      />
      <ProviderTemplateDialog
        open={providerTemplateDialogOpen}
        onClose={() => setProviderTemplateDialogOpen(false)}
        onSelect={handleProviderTemplateSelect}
      />
      {providerProxyPickerOpen ? (
        <ProxyPickerDialog
          value={providerForm.proxy_url}
          ipProxies={ipProxies}
          isLoading={isLoadingIpProxies}
          onSelect={(url) => {
            changeProviderForm((current) => ({ ...current, proxy_url: url }));
            setProviderProxyPickerOpen(false);
          }}
          onClose={() => setProviderProxyPickerOpen(false)}
          onOpenIpProxies={() => {
            setProviderProxyPickerOpen(false);
            setActiveSection('proxies');
            onOpenMain();
          }}
        />
      ) : null}
    </>
  );
}
