import type { ProviderType } from '../../../types';

export type ProtocolPreset = {
  type: ProviderType;
  labelKey: string;
  shortKey: string;
  endpoint: string;
  defaultBaseUrl: string;
};

export type ProviderTemplateId =
  | 'openai'
  | 'deepseek'
  | 'google'
  | 'anthropic'
  | 'kimi'
  | 'zhipu'
  | 'nvidia'
  | 'bailian'
  | 'openrouter'
  | 'siliconflow'
  | 'modelscope'
  | 'volcengine'
  | 'hunyuan'
  | 'qianfan'
  | 'longcat'
  | 'mimo'
  | 'zai'
  | 'stepfun'
  | 'grok'
  | 'minimax'
  | 'opencode'
  | 'custom';

export type ProviderTemplate = {
  id: ProviderTemplateId;
  labelKey: string;
  descriptionKey: string;
  providerType?: ProviderType;
  baseUrl?: string;
  iconPath?: string;
  fixed: boolean;
};

export const PROTOCOL_PRESETS: ProtocolPreset[] = [
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

export const protocolPresetByType = Object.fromEntries(
  PROTOCOL_PRESETS.map((preset) => [preset.type, preset]),
) as Record<ProviderType, ProtocolPreset>;

export const PROVIDER_TEMPLATES: ProviderTemplate[] = [
  {
    id: 'custom',
    labelKey: 'provider.template.custom.name',
    descriptionKey: 'provider.template.custom.description',
    fixed: false,
  },
  {
    id: 'openai',
    labelKey: 'provider.template.openai.name',
    descriptionKey: 'provider.template.openai.description',
    providerType: 'openai_compatible',
    baseUrl: 'https://api.openai.com/v1',
    iconPath: '/model-providers/openai.svg',
    fixed: true,
  },
  {
    id: 'deepseek',
    labelKey: 'provider.template.deepseek.name',
    descriptionKey: 'provider.template.deepseek.description',
    providerType: 'openai_compatible',
    baseUrl: 'https://api.deepseek.com/v1',
    iconPath: '/model-providers/deepseek.svg',
    fixed: true,
  },
  {
    id: 'google',
    labelKey: 'provider.template.google.name',
    descriptionKey: 'provider.template.google.description',
    providerType: 'google_gemini',
    baseUrl: 'https://generativelanguage.googleapis.com/v1beta',
    iconPath: '/model-providers/google-gemini.svg',
    fixed: true,
  },
  {
    id: 'anthropic',
    labelKey: 'provider.template.anthropic.name',
    descriptionKey: 'provider.template.anthropic.description',
    providerType: 'anthropic_messages',
    baseUrl: 'https://api.anthropic.com/v1',
    iconPath: '/model-providers/anthropic.svg',
    fixed: true,
  },
  {
    id: 'nvidia',
    labelKey: 'provider.template.nvidia.name',
    descriptionKey: 'provider.template.nvidia.description',
    providerType: 'openai_compatible',
    baseUrl: 'https://integrate.api.nvidia.com/v1',
    iconPath: '/model-providers/nvidia.svg',
    fixed: true,
  },
  {
    id: 'bailian',
    labelKey: 'provider.template.bailian.name',
    descriptionKey: 'provider.template.bailian.description',
    providerType: 'openai_compatible',
    baseUrl: 'https://dashscope.aliyuncs.com/compatible-mode/v1',
    iconPath: '/model-providers/alibaba-bailian.svg',
    fixed: true,
  },
  {
    id: 'openrouter',
    labelKey: 'provider.template.openrouter.name',
    descriptionKey: 'provider.template.openrouter.description',
    providerType: 'openai_compatible',
    baseUrl: 'https://openrouter.ai/api/v1',
    iconPath: '/model-providers/openrouter.svg',
    fixed: true,
  },
  {
    id: 'siliconflow',
    labelKey: 'provider.template.siliconflow.name',
    descriptionKey: 'provider.template.siliconflow.description',
    providerType: 'openai_compatible',
    baseUrl: 'https://api.siliconflow.cn/v1',
    iconPath: '/model-providers/siliconflow.svg',
    fixed: true,
  },
  {
    id: 'modelscope',
    labelKey: 'provider.template.modelscope.name',
    descriptionKey: 'provider.template.modelscope.description',
    providerType: 'openai_compatible',
    baseUrl: 'https://api-inference.modelscope.cn/v1',
    iconPath: '/model-providers/modelscope.svg',
    fixed: true,
  },
  {
    id: 'volcengine',
    labelKey: 'provider.template.volcengine.name',
    descriptionKey: 'provider.template.volcengine.description',
    providerType: 'openai_compatible',
    baseUrl: 'https://ark.cn-beijing.volces.com/api/v3',
    iconPath: '/model-providers/volcengine.svg',
    fixed: true,
  },
  {
    id: 'hunyuan',
    labelKey: 'provider.template.hunyuan.name',
    descriptionKey: 'provider.template.hunyuan.description',
    providerType: 'openai_compatible',
    baseUrl: 'https://api.hunyuan.cloud.tencent.com/v1',
    iconPath: '/model-providers/hunyuan.svg',
    fixed: true,
  },
  {
    id: 'qianfan',
    labelKey: 'provider.template.qianfan.name',
    descriptionKey: 'provider.template.qianfan.description',
    providerType: 'openai_compatible',
    baseUrl: 'https://qianfan.baidubce.com/v2',
    iconPath: '/model-providers/baidu-qianfan.svg',
    fixed: true,
  },
  {
    id: 'longcat',
    labelKey: 'provider.template.longcat.name',
    descriptionKey: 'provider.template.longcat.description',
    providerType: 'openai_compatible',
    baseUrl: 'https://api.longcat.chat/openai',
    iconPath: '/model-providers/longcat.svg',
    fixed: true,
  },
  {
    id: 'mimo',
    labelKey: 'provider.template.mimo.name',
    descriptionKey: 'provider.template.mimo.description',
    providerType: 'openai_compatible',
    baseUrl: 'https://api.xiaomimimo.com/v1',
    iconPath: '/model-providers/xiaomi-mimo.svg',
    fixed: true,
  },
  {
    id: 'stepfun',
    labelKey: 'provider.template.stepfun.name',
    descriptionKey: 'provider.template.stepfun.description',
    providerType: 'openai_compatible',
    baseUrl: 'https://api.stepfun.com/v1',
    iconPath: '/model-providers/stepfun.svg',
    fixed: true,
  },
  {
    id: 'grok',
    labelKey: 'provider.template.grok.name',
    descriptionKey: 'provider.template.grok.description',
    providerType: 'openai_compatible',
    baseUrl: 'https://api.x.ai/v1',
    iconPath: '/model-providers/grok.svg',
    fixed: true,
  },
  {
    id: 'minimax',
    labelKey: 'provider.template.minimax.name',
    descriptionKey: 'provider.template.minimax.description',
    providerType: 'openai_compatible',
    baseUrl: 'https://api.minimaxi.com/v1',
    iconPath: '/model-providers/minimax.svg',
    fixed: true,
  },
  {
    id: 'kimi',
    labelKey: 'provider.template.kimi.name',
    descriptionKey: 'provider.template.kimi.description',
    providerType: 'openai_compatible',
    baseUrl: 'https://api.moonshot.cn/v1',
    iconPath: '/model-providers/kimi.svg',
    fixed: true,
  },
  {
    id: 'zhipu',
    labelKey: 'provider.template.zhipu.name',
    descriptionKey: 'provider.template.zhipu.description',
    providerType: 'openai_compatible',
    baseUrl: 'https://open.bigmodel.cn/api/paas/v4',
    iconPath: '/model-providers/zhipu.svg',
    fixed: true,
  },
  {
    id: 'zai',
    labelKey: 'provider.template.zai.name',
    descriptionKey: 'provider.template.zai.description',
    providerType: 'openai_compatible',
    baseUrl: 'https://api.z.ai/api/paas/v4',
    iconPath: '/model-providers/zai.svg',
    fixed: true,
  },
  {
    id: 'opencode',
    labelKey: 'provider.template.opencode.name',
    descriptionKey: 'provider.template.opencode.description',
    providerType: 'openai_compatible',
    baseUrl: 'https://opencode.ai/zen/v1',
    iconPath: '/model-providers/opencode.svg',
    fixed: true,
  },
];

export const providerTemplateById = Object.fromEntries(
  PROVIDER_TEMPLATES.map((template) => [template.id, template]),
) as Record<ProviderTemplateId, ProviderTemplate>;
