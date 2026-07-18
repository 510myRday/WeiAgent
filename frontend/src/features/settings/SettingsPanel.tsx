import { useState, useEffect, useRef, FormEvent, type SyntheticEvent } from 'react';
import {
  ArrowLeft,
  ChevronDown,
  Code2,
  Download,
  Eye,
  EyeOff,
  KeyRound,
  Plus,
  Save,
  Server,
  Brain,
  Trash2,
} from 'lucide-react';

import type {
  AppSettings,
  ModelConfig,
  ModelProvider,
  ModelProviderPayload,
  ProviderType,
} from '../../types';
import { fetchModels, fetchModelsForProvider } from '../../api';
import { useBeforeUnloadGuard } from '../../hooks';
import { useI18n } from '../../i18n';
import { PanelShell } from '../../components/layout';
import { AutoSaveIndicator, Button, FormField, IconButton, ListItem, PaneHeader, useFeedback } from '../../components/ui';
import { userErrorMessage } from '../../utils/userError';

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

type SettingsSection = 'runtime' | 'agent' | 'providers';
type RuntimeAutoSaveStatus = 'idle' | 'pending' | 'saving' | 'saved' | 'error';
type PendingRuntimeSave = {
  serialized: string;
  payload: AppSettings;
};
type ProviderAutoSaveStatus = 'idle' | 'pending' | 'saving' | 'saved' | 'error';
type PendingProviderSave = {
  providerId: string;
  serialized: string;
  payload: ModelProviderPayload;
};
type ExtraBodyStatus = 'empty' | 'invalid' | 'valid';

type ProviderForm = {
  provider_type: ProviderType;
  name: string;
  base_url: string;
  api_key: string;
  modelsText: string;
  modelConfigs: ModelConfig[];
  extraBodyText: string;
};

type ProtocolPreset = {
  type: ProviderType;
  labelKey: string;
  shortKey: string;
  endpoint: string;
  defaultBaseUrl: string;
};

const PROTOCOL_PRESETS: ProtocolPreset[] = [
  {
    type: 'openai_compatible',
    labelKey: 'provider.protocol.openaiChat',
    shortKey: 'provider.protocol.openaiChatShort',
    endpoint: 'Chat Completions',
    defaultBaseUrl: 'https://api.openai.com/v1',
  },
  {
    type: 'openai_responses',
    labelKey: 'provider.protocol.openaiResponses',
    shortKey: 'provider.protocol.openaiResponsesShort',
    endpoint: 'Responses API',
    defaultBaseUrl: 'https://api.openai.com/v1',
  },
  {
    type: 'anthropic_messages',
    labelKey: 'provider.protocol.anthropic',
    shortKey: 'provider.protocol.anthropicShort',
    endpoint: 'Messages API',
    defaultBaseUrl: 'https://api.anthropic.com/v1',
  },
  {
    type: 'google_gemini',
    labelKey: 'provider.protocol.gemini',
    shortKey: 'provider.protocol.geminiShort',
    endpoint: 'Gemini generateContent',
    defaultBaseUrl: 'https://generativelanguage.googleapis.com/v1beta',
  },
];

const protocolPresetByType = Object.fromEntries(
  PROTOCOL_PRESETS.map((preset) => [preset.type, preset]),
) as Record<ProviderType, ProtocolPreset>;

const defaultProviderForm: ProviderForm = {
  provider_type: 'openai_compatible',
  name: '',
  base_url: protocolPresetByType.openai_compatible.defaultBaseUrl,
  api_key: '',
  modelsText: '',
  modelConfigs: [],
  extraBodyText: '',
};

const VALID_HOSTS = new Set(['127.0.0.1', '::1']);
const VALID_LOG_LEVELS = new Set(['trace', 'debug', 'info', 'warn', 'error']);
const RUNTIME_AUTO_SAVE_DELAY_MS = 600;
const RUNTIME_AUTO_SAVE_RETRY_DELAY_MS = 1200;
const RUNTIME_AUTO_SAVE_MAX_RETRY_DELAY_MS = 30000;
const PROVIDER_AUTO_SAVE_DELAY_MS = 600;
const PROVIDER_AUTO_SAVE_RETRY_DELAY_MS = 1200;
const PROVIDER_AUTO_SAVE_MAX_RETRY_DELAY_MS = 30000;

function parseModels(value: string): string[] {
  return Array.from(new Set(value.split(/\r?\n|,/).map((model) => model.trim()).filter(Boolean)));
}

function defaultModelConfig(modelId: string): ModelConfig {
  return {
    model_id: modelId,
    multimodal: false,
    thinking: false,
    context_size: 128000,
  };
}

function normalizeModelConfig(modelId: string, config: ModelConfig): ModelConfig {
  const legacyConfig = config as ModelConfig & { cache_price?: number };
  return {
    model_id: modelId,
    multimodal: Boolean(config.multimodal),
    thinking: Boolean(config.thinking),
    context_size: Number.isFinite(config.context_size) && config.context_size > 0
      ? Math.floor(config.context_size)
      : 128000,
    input_price: config.input_price,
    output_price: config.output_price,
    cache_write_price: config.cache_write_price,
    cache_read_price: config.cache_read_price ?? legacyConfig.cache_price,
  };
}

function syncModelConfigs(models: string[], current: ModelConfig[]): ModelConfig[] {
  const exactMatches = new Map(current.map((config) => [config.model_id, config]));
  const nextModelSet = new Set(models);
  const usedConfigIds = new Set<string>();

  return models.map((modelId, index) => {
    const exact = exactMatches.get(modelId);
    if (exact) {
      usedConfigIds.add(exact.model_id);
      return normalizeModelConfig(modelId, exact);
    }

    const sameRow = current[index];
    if (
      sameRow
      && !nextModelSet.has(sameRow.model_id)
      && !usedConfigIds.has(sameRow.model_id)
    ) {
      usedConfigIds.add(sameRow.model_id);
      return normalizeModelConfig(modelId, sameRow);
    }

    return defaultModelConfig(modelId);
  });
}

function parseOptionalPrice(value: string): number | undefined {
  if (!value.trim()) return undefined;
  const parsed = Number(value);
  return Number.isFinite(parsed) ? parsed : undefined;
}

function parseContextSize(value: string): number {
  if (!value.trim()) return 128000;
  const parsed = Number(value);
  if (!Number.isFinite(parsed) || parsed <= 0) return 128000;
  return Math.floor(parsed);
}

function isValidRuntimeSettings(settings: AppSettings): boolean {
  if (!VALID_HOSTS.has(settings.host)) return false;
  if (!Number.isInteger(settings.port) || settings.port < 1 || settings.port > 65535) return false;
  if (!VALID_LOG_LEVELS.has(settings.log_level)) return false;
  if (
    !Number.isInteger(settings.request_timeout_seconds)
    || settings.request_timeout_seconds < 1
    || settings.request_timeout_seconds > 600
  ) {
    return false;
  }
  if (
    !Number.isInteger(settings.request_retry_attempts)
    || settings.request_retry_attempts < 1
    || settings.request_retry_attempts > 10
  ) {
    return false;
  }
  return true;
}

function serializeRuntimeSettings(settings: AppSettings): string {
  return JSON.stringify({
    host: settings.host,
    port: settings.port,
    log_level: settings.log_level,
    request_timeout_seconds: settings.request_timeout_seconds,
    request_retry_attempts: settings.request_retry_attempts,
    llm_log_enabled: settings.llm_log_enabled,
    // Keep avatar/language in the payload for saves, but treat them as owned elsewhere
    // so external Sidebar updates do not thrash auto-save status.
    avatar_icon: settings.avatar_icon,
    avatar_color: settings.avatar_color,
    language: settings.language,
  });
}

function formatExtraBodyText(extraBody: unknown): string {
  if (extraBody == null || typeof extraBody !== 'object' || Array.isArray(extraBody)) {
    return '';
  }
  if (Object.keys(extraBody as Record<string, unknown>).length === 0) {
    return '';
  }
  return JSON.stringify(extraBody, null, 2);
}

function providerToForm(provider: ModelProvider): ProviderForm {
  return {
    provider_type: provider.provider_type,
    name: provider.name,
    base_url: provider.base_url,
    api_key: '',
    modelsText: provider.models.join('\n'),
    modelConfigs: syncModelConfigs(provider.models, provider.model_configs ?? []),
    extraBodyText: formatExtraBodyText(provider.extra_body),
  };
}

function parseExtraBodyText(text: string): {
  status: ExtraBodyStatus;
  value?: Record<string, unknown>;
} {
  const trimmed = text.trim();
  if (!trimmed) {
    return { status: 'empty' };
  }
  try {
    const parsed: unknown = JSON.parse(trimmed);
    if (parsed === null || typeof parsed !== 'object' || Array.isArray(parsed)) {
      return { status: 'invalid' };
    }
    return {
      status: 'valid',
      value: parsed as Record<string, unknown>,
    };
  } catch {
    return { status: 'invalid' };
  }
}

function buildProviderPayload(
  form: ProviderForm,
  requireApiKey: boolean,
): ModelProviderPayload | null {
  const parsedExtraBody = parseExtraBodyText(form.extraBodyText);
  if (parsedExtraBody.status === 'invalid') {
    return null;
  }

  const models = parseModels(form.modelsText);
  const payload: ModelProviderPayload = {
    provider_type: form.provider_type,
    name: form.name.trim(),
    base_url: form.base_url.trim(),
    api_key: form.api_key.trim(),
    models,
    model_configs: syncModelConfigs(models, form.modelConfigs),
    // Always send an object so PUT updates apply (JSON null becomes Option::None
    // on the backend and would leave the previous extra_body unchanged).
    extra_body: parsedExtraBody.status === 'valid'
      ? parsedExtraBody.value
      : {},
  };

  if (
    !payload.name
    || !payload.base_url
    || (requireApiKey && !payload.api_key)
    || payload.models.length === 0
  ) {
    return null;
  }
  return payload;
}

function serializeProviderForm(form: ProviderForm): string {
  const models = parseModels(form.modelsText);
  return JSON.stringify({
    provider_type: form.provider_type,
    name: form.name.trim(),
    base_url: form.base_url.trim(),
    api_key: form.api_key.trim(),
    models,
    model_configs: syncModelConfigs(models, form.modelConfigs),
    extra_body_text: form.extraBodyText.trim(),
  });
}

/** Rebuild editor state from a serializeProviderForm snapshot, clearing api_key. */
function formFromSerializedSnapshot(serialized: string): ProviderForm {
  const snap = JSON.parse(serialized) as {
    provider_type: ProviderType;
    name: string;
    base_url: string;
    models: string[];
    model_configs: ModelConfig[];
    extra_body_text: string;
  };
  return {
    provider_type: snap.provider_type,
    name: snap.name,
    base_url: snap.base_url,
    api_key: '',
    modelsText: snap.models.join('\n'),
    modelConfigs: snap.model_configs ?? [],
    // Keep the exact extra-body text the user saved (not server pretty-print).
    extraBodyText: snap.extra_body_text ?? '',
  };
}

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
  const [providerForm, setProviderForm] = useState(defaultProviderForm);
  const [isSubmittingProvider, setIsSubmittingProvider] = useState(false);
  const [fetchingModels, setFetchingModels] = useState(false);
  const [showApiKey, setShowApiKey] = useState(false);
  const [runtimeDirty, setRuntimeDirty] = useState(false);
  const [autoSaveStatus, setAutoSaveStatus] = useState<RuntimeAutoSaveStatus>('idle');
  const [providerDirty, setProviderDirty] = useState(false);
  const [providerAutoSaveStatus, setProviderAutoSaveStatus] = useState<ProviderAutoSaveStatus>('idle');
  const [advancedOpen, setAdvancedOpen] = useState(false);

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
  const modelCount = parseModels(providerForm.modelsText).length;
  const extraBodyStatus = parseExtraBodyText(providerForm.extraBodyText).status;

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
      setProviderForm(defaultProviderForm);
      setShowApiKey(false);
      setProviderDirty(false);
      setProviderAutoSaveStatus('idle');
      setAdvancedOpen(false);
      return;
    }
    const nextForm = providerToForm(activeProvider);
    const serialized = serializeProviderForm(nextForm);
    setProviderForm(nextForm);
    setShowApiKey(false);
    setProviderDirty(false);
    setProviderAutoSaveStatus('idle');
    setAdvancedOpen(Boolean(nextForm.extraBodyText.trim()));
    providerLatestSerializedByIdRef.current.set(activeProvider.id, serialized);
    providerPersistedSerializedByIdRef.current.set(activeProvider.id, serialized);
  }, [activeProvider, activeProviderId]);

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
    if (providerSaveTimerRef.current !== undefined) {
      window.clearTimeout(providerSaveTimerRef.current);
      providerSaveTimerRef.current = undefined;
      void flushProviderSaveQueue();
    }
    onProviderChange(id);
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
            </div>
          </div>
        )}
        main={(
          <div className="main-pane config-main">
          <PaneHeader
            className="config-page-header"
            leading={<Button className="main-back-button" variant="ghost" type="button" onClick={onBackToList} icon={<ArrowLeft size={17} />}>{t('common.back')}</Button>}
            eyebrow={t('settings.areaTitle')}
            title={activeSection === 'runtime'
              ? t('settings.title')
              : activeSection === 'agent'
                ? t('settings.agentTitle')
                : (
                  <span className="provider-page-title">
                    {t('provider.title')}
                    <span className="mini-status-badge provider-title-count">
                      {t('provider.configuredCount', { count: providers.length })}
                    </span>
                  </span>
                )}
            description={activeSection === 'runtime'
              ? t('settings.description')
              : activeSection === 'agent'
                ? t('settings.agentDescription')
                : t('provider.description')}
            actions={activeSection === 'providers'
              ? <AutoSaveIndicator status={providerAutoSaveStatus} t={t} onRetry={retryProviderSaveNow} />
              : <AutoSaveIndicator status={autoSaveStatus} t={t} onRetry={retryRuntimeSaveNow} />}
          />

          {activeSection === 'runtime' ? (
            <div className="form-card entity-form narrow-form">
              <div className="form-grid two-column">
                <FormField label={t('settings.host')} htmlFor="settings-host" required>
                  <select
                    id="settings-host"
                    value={settingsForm.host}
                    onChange={(e) => patchRuntimeSettings({ host: e.target.value })}
                    required
                  >
                    <option value="127.0.0.1">127.0.0.1</option>
                    <option value="::1">::1</option>
                  </select>
                </FormField>
                <FormField label={t('settings.port')} htmlFor="settings-port" required>
                  <input
                    id="settings-port"
                    type="number"
                    min={1}
                    max={65535}
                    value={settingsForm.port}
                    onChange={(e) => patchRuntimeSettings({ port: Number(e.target.value) })}
                    required
                  />
                </FormField>
                <FormField label={t('settings.logLevel')} htmlFor="settings-log" required>
                  <select
                    id="settings-log"
                    value={settingsForm.log_level}
                    onChange={(e) => patchRuntimeSettings({ log_level: e.target.value })}
                    required
                  >
                    {['trace', 'debug', 'info', 'warn', 'error'].map((level) => <option key={level} value={level}>{level}</option>)}
                  </select>
                </FormField>
              </div>
              <p className="hint-text">{t('settings.restartHint')}</p>
            </div>
          ) : activeSection === 'agent' ? (
            <div className="form-card entity-form narrow-form">
              <div className="form-grid two-column">
                <FormField label={t('settings.requestTimeout')} htmlFor="settings-timeout" required>
                  <input
                    id="settings-timeout"
                    type="number"
                    min={1}
                    max={600}
                    value={settingsForm.request_timeout_seconds}
                    onChange={(e) => patchRuntimeSettings({ request_timeout_seconds: Number(e.target.value) })}
                    required
                  />
                </FormField>
                <FormField label={t('settings.requestRetries')} htmlFor="settings-retries" required>
                  <input
                    id="settings-retries"
                    type="number"
                    min={1}
                    max={10}
                    value={settingsForm.request_retry_attempts}
                    onChange={(e) => patchRuntimeSettings({ request_retry_attempts: Number(e.target.value) })}
                    required
                  />
                </FormField>
              </div>
              <p className="hint-text small-hint">{t('settings.agentRetryHint')}</p>
            </div>
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
                    {providers.map((provider) => (
                      <option key={provider.id} value={provider.id}>
                        {provider.name}
                      </option>
                    ))}
                  </select>
                </FormField>
                <div className="provider-picker-actions">
                  <Button variant="secondary" type="button" onClick={() => handleProviderChange('')} icon={<Plus size={16} />}>
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

              <form className="form-card entity-form provider-editor" onSubmit={handleProviderSubmit}>
                <p className="provider-editor-kicker">
                  {activeProvider ? t('provider.editing') : t('provider.creating')}
                </p>

                <section className="provider-form-section">
                  <PaneHeader
                    compact
                    title={t('provider.connection')}
                    actions={(
                      <span className={`mini-status-badge${
                        activeProvider?.has_api_key || providerForm.api_key ? '' : ' is-required'
                      }`}>
                        {activeProvider?.has_api_key || providerForm.api_key
                          ? t('provider.keyConfigured')
                          : t('provider.keyRequired')}
                      </span>
                    )}
                  />
                  <div className="form-grid two-column provider-connection-grid">
                    <FormField label={t('provider.name')} htmlFor="provider-name" required>
                      <input id="provider-name" value={providerForm.name} onChange={(e) => changeProviderForm((current) => ({ ...current, name: e.target.value }))} disabled={isSubmittingProvider} required />
                    </FormField>
                    <FormField label={t('provider.apiMode')} htmlFor="provider-api-mode" required>
                      <select
                        id="provider-api-mode"
                        value={providerForm.provider_type}
                        onChange={(event) => handleProtocolChange(event.target.value as ProviderType)}
                        disabled={isSubmittingProvider}
                        required
                      >
                        {PROTOCOL_PRESETS.map((preset) => (
                          <option key={preset.type} value={preset.type}>
                            {t(preset.labelKey)}
                          </option>
                        ))}
                      </select>
                    </FormField>
                    <FormField label={t('provider.baseUrl')} htmlFor="provider-base-url" required>
                      <input id="provider-base-url" value={providerForm.base_url} onChange={(e) => changeProviderForm((current) => ({ ...current, base_url: e.target.value }))} disabled={isSubmittingProvider} required />
                    </FormField>
                    <FormField label={t('provider.apiKey')} htmlFor="provider-api-key" required={!activeProvider}>
                      <div className="provider-secret-input">
                        <input
                          id="provider-api-key"
                          type={showApiKey ? 'text' : 'password'}
                          value={providerForm.api_key}
                          onChange={(e) => changeProviderForm((current) => ({ ...current, api_key: e.target.value }))}
                          placeholder={activeProvider?.has_api_key ? activeProvider.masked_api_key : ''}
                          disabled={isSubmittingProvider}
                          required={!activeProvider}
                        />
                        <IconButton
                          className="provider-secret-toggle"
                          variant="ghost"
                          type="button"
                          onClick={() => setShowApiKey((visible) => !visible)}
                          label={showApiKey ? t('provider.hideKey') : t('provider.showKey')}
                        >
                          {showApiKey ? <EyeOff size={16} /> : <Eye size={16} />}
                        </IconButton>
                      </div>
                    </FormField>
                  </div>
                </section>

                <section className="provider-form-section">
                  <div className="provider-model-section-heading">
                    <div>
                      <h3>{t('provider.models')}</h3>
                      <p>{t('provider.modelsDescription')}</p>
                    </div>
                    <div className="provider-field-actions">
                      <span className={`mini-status-badge${modelCount === 0 ? ' is-required' : ''}`}>
                        {modelCount === 0
                          ? t('provider.modelsRequired')
                          : t('provider.modelCount', { count: modelCount })}
                      </span>
                      <Button
                        className="inline-action"
                        variant="ghost"
                        type="button"
                        disabled={fetchingModels}
                        onClick={handleFetchModels}
                        icon={<Download size={14} />}
                      >
                        {fetchingModels ? t('provider.fetchingModels') : t('provider.fetchModels')}
                      </Button>
                    </div>
                  </div>

                  <div className="provider-model-workspace">
                    <FormField
                      label={t('provider.modelIds')}
                      htmlFor="provider-models"
                      required
                    >
                      <textarea
                        id="provider-models"
                        value={providerForm.modelsText}
                        onChange={(event) => handleModelsTextChange(event.target.value)}
                        rows={Math.max(8, Math.min(14, modelCount + 3))}
                        required
                        aria-required="true"
                        placeholder={t('provider.modelsPlaceholder')}
                        disabled={isSubmittingProvider || fetchingModels}
                      />
                    </FormField>

                    <div className="provider-model-config">
                      <div className="provider-model-config__label">
                        {t('provider.modelConfiguration')}
                        <span>（{t('provider.priceUnit')}）</span>
                      </div>
                      <div className="model-config-list">
                        {providerForm.modelConfigs.length === 0 ? (
                          <div className="model-config-empty">{t('provider.modelConfigurationEmpty')}</div>
                        ) : (
                          <>
                            <div className="model-config-header" aria-hidden="true">
                              <span>{t('provider.model')}</span>
                              <span>{t('provider.capabilities')}</span>
                              <span>{t('provider.contextSize')}</span>
                              <span>{t('provider.inputPrice')}</span>
                              <span>{t('provider.outputPrice')}</span>
                              <span>{t('provider.cacheWritePrice')}</span>
                              <span>{t('provider.cacheReadPrice')}</span>
                            </div>
                            {providerForm.modelConfigs.map((config, index) => (
                              <div className="model-config-row" key={config.model_id}>
                                <strong className="model-config-name" title={config.model_id}>
                                  {config.model_id}
                                </strong>
                                <div className="model-capabilities">
                                  {([
                                    ['multimodal', 'provider.multimodal'],
                                    ['thinking', 'provider.thinking'],
                                  ] as const).map(([field, labelKey]) => (
                                    <label className="model-capability-toggle" key={field}>
                                      <span>{t(labelKey)}</span>
                                      <input
                                        type="checkbox"
                                        checked={config[field]}
                                        onChange={(event) => patchModelConfig(config.model_id, {
                                          [field]: event.target.checked,
                                        })}
                                        disabled={isSubmittingProvider}
                                        aria-label={`${config.model_id} ${t(labelKey)}`}
                                      />
                                      <span className="model-capability-toggle__track" aria-hidden="true" />
                                    </label>
                                  ))}
                                </div>
                                <label className="model-context-field">
                                  <span className="model-price-field__mobile-label">{t('provider.contextSize')}</span>
                                  <span className="model-context-field__input">
                                    <input
                                      id={`provider-model-${index}-context_size`}
                                      type="number"
                                      min="1"
                                      step="1"
                                      inputMode="numeric"
                                      value={config.context_size}
                                      placeholder="128000"
                                      onChange={(event) => patchModelConfig(config.model_id, {
                                        context_size: parseContextSize(event.target.value),
                                      })}
                                      disabled={isSubmittingProvider}
                                      aria-label={`${config.model_id} ${t('provider.contextSize')}`}
                                    />
                                  </span>
                                </label>
                                {([
                                  ['input_price', 'provider.inputPrice'],
                                  ['output_price', 'provider.outputPrice'],
                                  ['cache_write_price', 'provider.cacheWritePrice'],
                                  ['cache_read_price', 'provider.cacheReadPrice'],
                                ] as const).map(([field, labelKey]) => (
                                  <label className="model-price-field" key={field}>
                                    <span className="model-price-field__mobile-label">{t(labelKey)}</span>
                                    <span className="model-price-field__input">
                                      <span aria-hidden="true">$</span>
                                      <input
                                        id={`provider-model-${index}-${field}`}
                                        type="number"
                                        min="0"
                                        step="0.000001"
                                        inputMode="decimal"
                                        value={config[field] ?? ''}
                                        placeholder="0.00"
                                        onChange={(event) => patchModelConfig(config.model_id, {
                                          [field]: parseOptionalPrice(event.target.value),
                                        })}
                                        disabled={isSubmittingProvider}
                                        aria-label={`${config.model_id} ${t(labelKey)}`}
                                      />
                                    </span>
                                  </label>
                                ))}
                              </div>
                            ))}
                          </>
                        )}
                      </div>
                    </div>
                  </div>
                </section>

                <details
                  className="provider-advanced"
                  open={advancedOpen}
                  onToggle={(event: SyntheticEvent<HTMLDetailsElement>) => {
                    setAdvancedOpen(event.currentTarget.open);
                  }}
                >
                  <summary>
                    <span className="provider-advanced__icon" aria-hidden="true">
                      <Code2 size={17} />
                    </span>
                    <span className="provider-advanced__content">
                      <strong>{t('provider.advanced')}</strong>
                      <small>{t('provider.extraBody')}</small>
                    </span>
                    {extraBodyStatus === 'valid' ? (
                      <span className="mini-status-badge">{t('provider.advancedConfigured')}</span>
                    ) : extraBodyStatus === 'invalid' ? (
                      <span className="mini-status-badge is-required">{t('provider.advancedInvalid')}</span>
                    ) : null}
                    <ChevronDown className="provider-advanced__chevron" size={17} aria-hidden="true" />
                  </summary>
                  <FormField label={t('provider.extraBody')} htmlFor="provider-extra-body">
                    <textarea
                      id="provider-extra-body"
                      value={providerForm.extraBodyText}
                      onChange={(e) => {
                        const nextValue = e.target.value;
                        changeProviderForm((current) => ({ ...current, extraBodyText: nextValue }));
                        if (nextValue.trim()) {
                          setAdvancedOpen(true);
                        }
                      }}
                      rows={5}
                      placeholder='{"temperature": 0.7}'
                      disabled={isSubmittingProvider}
                      aria-invalid={extraBodyStatus === 'invalid'}
                    />
                    {extraBodyStatus === 'invalid' ? (
                      <p className="hint-text provider-extra-body-error" role="alert">
                        {t('error.invalidJson')}
                      </p>
                    ) : null}
                  </FormField>
                </details>

                {!activeProvider ? (
                  <div className="form-actions provider-form-actions">
                    <Button variant="primary" type="submit" disabled={isSubmittingProvider} icon={<Save size={17} />}>
                      {t('provider.create')}
                    </Button>
                  </div>
                ) : null}
              </form>
            </div>
          )}
          </div>
        )}
      />
    </>
  );
}


