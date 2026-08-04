import type {
  ApiKeyStrategy,
  AppSettings,
  ImageModelConfig,
  ModelConfig,
  ModelProvider,
  ModelProviderPayload,
  ProviderType,
} from '../../../types';
import { protocolPresetByType } from './providerTemplates';
import type { ProviderTemplate } from './providerTemplates';

export type RuntimeAutoSaveStatus = 'idle' | 'pending' | 'saving' | 'saved' | 'error';
export type PendingRuntimeSave = {
  serialized: string;
  payload: AppSettings;
};
export type ProviderAutoSaveStatus = 'idle' | 'pending' | 'saving' | 'saved' | 'error';
export type PendingProviderSave = {
  providerId: string;
  serialized: string;
  payload: ModelProviderPayload;
};
export type ExtraBodyStatus = 'empty' | 'invalid' | 'valid';

export type ProviderForm = {
  provider_type: ProviderType;
  name: string;
  base_url: string;
  image_generation_url: string;
  image_edit_url: string;
  /** Masked entries reference stored keys; other entries are new plaintext keys. */
  apiKeys: string[];
  /** Text sitting in the add-key input, not yet committed to `apiKeys`. */
  newKeyText: string;
  key_strategy: ApiKeyStrategy;
  modelsText: string;
  modelConfigs: ModelConfig[];
  extraBodyText: string;
  proxy_url: string | null;
};

export const defaultProviderForm: ProviderForm = {
  provider_type: 'openai_compatible',
  name: '',
  base_url: protocolPresetByType.openai_compatible.defaultBaseUrl,
  image_generation_url: '',
  image_edit_url: '',
  apiKeys: [],
  newKeyText: '',
  key_strategy: 'failover',
  modelsText: '',
  modelConfigs: [],
  extraBodyText: '',
  proxy_url: null,
};

/** Fold the pending add-key input into the committed key rows. */
export function commitPendingApiKeys(form: ProviderForm): ProviderForm {
  const pending = parseApiKeyInput(form.newKeyText);
  if (pending.length === 0) {
    return form.newKeyText ? { ...form, newKeyText: '' } : form;
  }
  return {
    ...form,
    apiKeys: normalizeFormApiKeys([...form.apiKeys, ...pending]),
    newKeyText: '',
  };
}

export function providerTemplateToForm(template: ProviderTemplate, name: string): ProviderForm {
  if (!template.fixed || !template.providerType || !template.baseUrl) {
    return { ...defaultProviderForm, modelConfigs: [] };
  }
  return {
    ...defaultProviderForm,
    provider_type: template.providerType,
    name,
    base_url: template.baseUrl,
    image_generation_url: '',
    image_edit_url: '',
    modelConfigs: [],
  };
}

export const VALID_HOSTS = new Set(['127.0.0.1', '::1']);
export const VALID_LOG_LEVELS = new Set(['trace', 'debug', 'info', 'warn', 'error']);
export const RUNTIME_AUTO_SAVE_DELAY_MS = 600;
export const RUNTIME_AUTO_SAVE_RETRY_DELAY_MS = 1200;
export const RUNTIME_AUTO_SAVE_MAX_RETRY_DELAY_MS = 30000;
export const PROVIDER_AUTO_SAVE_DELAY_MS = 600;
export const PROVIDER_AUTO_SAVE_RETRY_DELAY_MS = 1200;
export const PROVIDER_AUTO_SAVE_MAX_RETRY_DELAY_MS = 30000;

export function parseModels(value: string): string[] {
  return Array.from(new Set(value.split(/\r?\n|,/).map((model) => model.trim()).filter(Boolean)));
}

/** Mirrors the backend convention: masked keys render as **** plus at most 4 tail chars. */
export function isMaskedApiKey(value: string): boolean {
  const trimmed = value.trim();
  return trimmed.startsWith('****') && trimmed.length <= 8;
}

/** Split pasted text into individual keys (newlines, commas, or blanks separate keys). */
export function parseApiKeyInput(value: string): string[] {
  return Array.from(new Set(value.split(/[\s,]+/).map((key) => key.trim()).filter(Boolean)));
}

export function normalizeFormApiKeys(apiKeys: string[]): string[] {
  return Array.from(new Set(apiKeys.map((key) => key.trim()).filter(Boolean)));
}

export function defaultModelConfig(modelId: string): ModelConfig {
  return {
    model_id: modelId,
    multimodal: false,
    thinking: false,
    context_size: 128000,
  };
}

export function normalizeModelConfig(modelId: string, config: ModelConfig): ModelConfig {
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
    cache_read_price: config.cache_read_price,
    image_options: normalizeImageModelConfig(config.image_options),
  };
}

export function normalizeImageModelConfig(
  config: ImageModelConfig | undefined,
): ImageModelConfig | undefined {
  if (!config) return undefined;
  const normalized: ImageModelConfig = {
    size: normalizeOptionalString(config.size),
    quality: normalizeOptionalString(config.quality),
    n: normalizeOptionalInteger(config.n, 1, 10),
    output_format: normalizeOptionalString(config.output_format),
    output_compression: normalizeOptionalInteger(config.output_compression, 0, 100),
    background: normalizeOptionalString(config.background),
    moderation: normalizeOptionalString(config.moderation),
    style: normalizeOptionalString(config.style),
    response_format: normalizeOptionalString(config.response_format),
    input_fidelity: normalizeOptionalString(config.input_fidelity),
  };
  return Object.values(normalized).some((value) => value !== undefined) ? normalized : undefined;
}

function normalizeOptionalString(value: string | undefined): string | undefined {
  const trimmed = value?.trim();
  return trimmed || undefined;
}

function normalizeOptionalInteger(
  value: number | undefined,
  minimum: number,
  maximum: number,
): number | undefined {
  if (!Number.isFinite(value) || value === undefined) return undefined;
  const integer = Math.floor(value);
  return integer >= minimum && integer <= maximum ? integer : undefined;
}

export function syncModelConfigs(models: string[], current: ModelConfig[]): ModelConfig[] {
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

export function parseOptionalPrice(value: string): number | undefined {
  if (!value.trim()) return undefined;
  const parsed = Number(value);
  return Number.isFinite(parsed) ? parsed : undefined;
}

export function parseContextSize(value: string): number {
  if (!value.trim()) return 128000;
  const parsed = Number(value);
  if (!Number.isFinite(parsed) || parsed <= 0) return 128000;
  return Math.floor(parsed);
}

export function isValidRuntimeSettings(settings: AppSettings): boolean {
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
  if (
    !Number.isInteger(settings.context_compress_threshold_percent)
    || settings.context_compress_threshold_percent < 1
    || settings.context_compress_threshold_percent > 99
  ) {
    return false;
  }
  return true;
}

export function serializeRuntimeSettings(settings: AppSettings): string {
  return JSON.stringify({
    host: settings.host,
    port: settings.port,
    log_level: settings.log_level,
    request_timeout_seconds: settings.request_timeout_seconds,
    request_retry_attempts: settings.request_retry_attempts,
    context_compress_threshold_percent: settings.context_compress_threshold_percent,
    llm_log_enabled: settings.llm_log_enabled,
    conversation_title_provider_id: settings.conversation_title_provider_id,
    conversation_title_model_id: settings.conversation_title_model_id,
    // Keep avatar/language in the payload for saves, but treat them as owned elsewhere
    // so external Sidebar updates do not thrash auto-save status.
    avatar_icon: settings.avatar_icon,
    avatar_color: settings.avatar_color,
    language: settings.language,
  });
}

export function formatExtraBodyText(extraBody: unknown): string {
  if (extraBody == null || typeof extraBody !== 'object' || Array.isArray(extraBody)) {
    return '';
  }
  if (Object.keys(extraBody as Record<string, unknown>).length === 0) {
    return '';
  }
  return JSON.stringify(extraBody, null, 2);
}

export function providerToForm(provider: ModelProvider): ProviderForm {
  return {
    provider_type: provider.provider_type,
    name: provider.name,
    base_url: provider.base_url,
    image_generation_url: provider.image_generation_url ?? '',
    image_edit_url: provider.image_edit_url ?? '',
    apiKeys: [...(provider.masked_api_keys ?? [])],
    newKeyText: '',
    key_strategy: provider.key_strategy ?? 'failover',
    modelsText: provider.models.join('\n'),
    modelConfigs: syncModelConfigs(provider.models, provider.model_configs ?? []),
    extraBodyText: formatExtraBodyText(provider.extra_body),
    proxy_url: provider.proxy_url ?? null,
  };
}

export function parseExtraBodyText(text: string): {
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

export function buildProviderPayload(form: ProviderForm): ModelProviderPayload | null {
  const parsedExtraBody = parseExtraBodyText(form.extraBodyText);
  if (parsedExtraBody.status === 'invalid') {
    return null;
  }

  const models = parseModels(form.modelsText);
  const payload: ModelProviderPayload = {
    provider_type: form.provider_type,
    name: form.name.trim(),
    base_url: form.base_url.trim(),
    image_generation_url: form.image_generation_url.trim() || null,
    image_edit_url: form.image_edit_url.trim() || null,
    api_keys: normalizeFormApiKeys(form.apiKeys),
    key_strategy: form.key_strategy,
    models,
    model_configs: syncModelConfigs(models, form.modelConfigs),
    // Always send an object so PUT updates apply (JSON null becomes Option::None
    // on the backend and would leave the previous extra_body unchanged).
    extra_body: parsedExtraBody.status === 'valid'
      ? parsedExtraBody.value
      : {},
    // Explicit null clears the proxy on PUT (missing field would leave it unchanged).
    proxy_url: form.proxy_url?.trim() || null,
  };

  if (
    !payload.name
    || !payload.base_url
    || payload.models.length === 0
  ) {
    return null;
  }
  return payload;
}

export function serializeProviderForm(form: ProviderForm): string {
  const models = parseModels(form.modelsText);
  return JSON.stringify({
    provider_type: form.provider_type,
    name: form.name.trim(),
    base_url: form.base_url.trim(),
    image_generation_url: form.image_generation_url.trim(),
    image_edit_url: form.image_edit_url.trim(),
    api_keys: normalizeFormApiKeys(form.apiKeys),
    key_strategy: form.key_strategy,
    models,
    model_configs: syncModelConfigs(models, form.modelConfigs),
    extra_body_text: form.extraBodyText.trim(),
    proxy_url: form.proxy_url?.trim() || null,
  });
}

/** Rebuild editor state from a serializeProviderForm snapshot. */
export function formFromSerializedSnapshot(serialized: string): ProviderForm {
  const snap = JSON.parse(serialized) as {
    provider_type: ProviderType;
    name: string;
    base_url: string;
    image_generation_url?: string;
    image_edit_url?: string;
    api_keys?: string[];
    key_strategy?: ApiKeyStrategy;
    models: string[];
    model_configs: ModelConfig[];
    extra_body_text: string;
    proxy_url?: string | null;
  };
  return {
    provider_type: snap.provider_type,
    name: snap.name,
    base_url: snap.base_url,
    image_generation_url: snap.image_generation_url ?? '',
    image_edit_url: snap.image_edit_url ?? '',
    apiKeys: snap.api_keys ?? [],
    newKeyText: '',
    key_strategy: snap.key_strategy ?? 'failover',
    modelsText: snap.models.join('\n'),
    modelConfigs: snap.model_configs ?? [],
    // Keep the exact extra-body text the user saved (not server pretty-print).
    extraBodyText: snap.extra_body_text ?? '',
    proxy_url: snap.proxy_url ?? null,
  };
}

export function clampCompressThreshold(value: number): number {
  if (!Number.isFinite(value)) return 90;
  return Math.min(99, Math.max(1, Math.round(value)));
}
