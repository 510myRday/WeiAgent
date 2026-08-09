import type { ComponentProps } from 'react';
import { render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, expect, test, vi } from 'vitest';

import type { AppSettings, ModelProvider } from '../../types';
import { I18nProvider } from '../../i18n';
import { FeedbackProvider } from '../../components/ui';
import { SettingsPanel } from './SettingsPanel';
import { clearNewProviderDraft } from './settings-panel/newProviderDraft';

vi.mock('../../api', () => ({
  fetchModels: vi.fn().mockResolvedValue([]),
  fetchModelsForProvider: vi.fn().mockResolvedValue([]),
  listIpProxies: vi.fn().mockResolvedValue([]),
}));

const settings: AppSettings = {
  host: '127.0.0.1',
  port: 3000,
  log_level: 'info',
  avatar_icon: 'bot',
  avatar_color: '#07c160',
  language: 'zh',
  request_timeout_seconds: 60,
  request_retry_attempts: 5,
  context_compress_threshold_percent: 90,
  llm_log_enabled: false,
  conversation_title_provider_id: null,
  conversation_title_model_id: null,
};

beforeEach(() => {
  localStorage.setItem('weiagent-locale', 'zh');
  clearNewProviderDraft();
});

const existingProvider: ModelProvider = {
  id: 'provider-1',
  provider_type: 'openai_compatible',
  name: '已有提供商',
  base_url: 'https://api.example.com/v1',
  has_api_key: true,
  masked_api_keys: ['****-key'],
  key_strategy: 'failover',
  models: ['gpt-test'],
  model_configs: [],
  extra_body: null,
  created_at: '2026-01-01T00:00:00Z',
  updated_at: '2026-01-01T00:00:00Z',
};

function panelTree(overrides: Partial<ComponentProps<typeof SettingsPanel>> = {}) {
  return (
    <I18nProvider>
      <FeedbackProvider>
        <SettingsPanel
          settingsForm={settings}
          onSettingsFormChange={() => {}}
          onSettingsSave={vi.fn().mockResolvedValue(settings)}
          providers={[]}
          activeProviderId=""
          initialSection="providers"
          onProviderChange={() => {}}
          onProviderAdd={vi.fn()}
          onProviderEdit={vi.fn()}
          onProviderRemove={vi.fn()}
          onOpenMain={() => {}}
          onBackToList={() => {}}
          {...overrides}
        />
      </FeedbackProvider>
    </I18nProvider>
  );
}

test('provider templates populate and lock fixed connection fields while custom stays editable', async () => {
  const user = userEvent.setup();
  const { container } = render(
    <I18nProvider>
      <FeedbackProvider>
        <SettingsPanel
          settingsForm={settings}
          onSettingsFormChange={() => {}}
          onSettingsSave={vi.fn().mockResolvedValue(settings)}
          providers={[]}
          activeProviderId=""
          initialSection="providers"
          onProviderChange={() => {}}
          onProviderAdd={vi.fn()}
          onProviderEdit={vi.fn()}
          onProviderRemove={vi.fn()}
          onOpenMain={() => {}}
          onBackToList={() => {}}
        />
      </FeedbackProvider>
    </I18nProvider>,
  );

  const pickerActions = container.querySelector<HTMLElement>('.provider-picker-actions');
  expect(pickerActions).not.toBeNull();

  await user.click(within(pickerActions!).getByRole('button', { name: '创建提供商' }));

  expect(screen.getByRole('heading', { name: '选择提供商模板' })).toBeInTheDocument();
  const featuredTemplate = document.querySelector<HTMLElement>('.provider-template-featured');
  expect(featuredTemplate).not.toBeNull();
  const templateGrid = document.querySelector<HTMLElement>('.provider-template-grid');
  expect(templateGrid).not.toBeNull();
  const customTemplate = within(featuredTemplate!).getByRole('button');
  expect(customTemplate).toHaveAccessibleName(/自定义连接/);
  expect(customTemplate).toHaveTextContent('适用于私有部署、代理网关及其他兼容模型服务。');
  expect(customTemplate).toHaveTextContent('灵活配置');
  const openAiTemplate = screen.getByRole('button', { name: /OpenAI 官方 API/ });
  expect(openAiTemplate).toBeInTheDocument();
  expect(openAiTemplate.querySelector('img')).toHaveAttribute('src', '/model-providers/openai.svg');
  expect(templateGrid!.querySelectorAll('img')).toHaveLength(21);
  expect(screen.getByRole('button', { name: /Google Gemini API/ })).toBeInTheDocument();
  expect(screen.getByRole('button', { name: /Anthropic 官方 API/ })).toBeInTheDocument();
  expect(screen.getByRole('button', { name: /Kimi 开放平台/ })).toBeInTheDocument();
  expect(screen.getByRole('button', { name: /智谱 AI 开放平台/ })).toBeInTheDocument();
  const newTemplates = [
    ['NVIDIA NIM', '/model-providers/nvidia.svg', 'https://integrate.api.nvidia.com/v1'],
    ['阿里云百炼', '/model-providers/alibaba-bailian.svg', 'https://dashscope.aliyuncs.com/compatible-mode/v1'],
    ['OpenRouter', '/model-providers/openrouter.svg', 'https://openrouter.ai/api/v1'],
    ['硅基流动', '/model-providers/siliconflow.svg', 'https://api.siliconflow.cn/v1'],
    ['魔搭社区', '/model-providers/modelscope.svg', 'https://api-inference.modelscope.cn/v1'],
    ['火山引擎', '/model-providers/volcengine.svg', 'https://ark.cn-beijing.volces.com/api/v3'],
    ['腾讯混元', '/model-providers/hunyuan.svg', 'https://api.hunyuan.cloud.tencent.com/v1'],
    ['百度千帆', '/model-providers/baidu-qianfan.svg', 'https://qianfan.baidubce.com/v2'],
    ['美团龙猫', '/model-providers/longcat.svg', 'https://api.longcat.chat/openai'],
    ['小米 MiMo', '/model-providers/xiaomi-mimo.svg', 'https://api.xiaomimimo.com/v1'],
    ['阶跃星辰', '/model-providers/stepfun.svg', 'https://api.stepfun.com/v1'],
    ['xAI Grok', '/model-providers/grok.svg', 'https://api.x.ai/v1'],
    ['MiniMax', '/model-providers/minimax.svg', 'https://api.minimaxi.com/v1'],
    ['Z\\.ai', '/model-providers/zai.svg', 'https://api.z.ai/api/paas/v4'],
    ['OpenCode Zen', '/model-providers/opencode.svg', 'https://opencode.ai/zen/v1'],
  ] as const;
  for (const [name, iconPath, url] of newTemplates) {
    const template = screen.getByRole('button', { name: new RegExp(name) });
    expect(template.querySelector('img')).toHaveAttribute('src', iconPath);
    expect(template).toHaveTextContent(url);
  }

  const templateSearch = screen.getByPlaceholderText('搜索提供商、接口或服务地址');
  await user.type(templateSearch, 'api.x.ai');
  expect(screen.getByRole('button', { name: /xAI Grok/ })).toBeInTheDocument();
  expect(screen.queryByRole('button', { name: /OpenAI 官方 API/ })).not.toBeInTheDocument();

  await user.clear(templateSearch);
  await user.type(templateSearch, '没有这个服务');
  expect(screen.getByRole('status')).toHaveTextContent('没有匹配的提供商');
  await user.click(screen.getByRole('button', { name: '清除搜索' }));
  expect(templateSearch).toHaveValue('');

  await user.click(screen.getByRole('button', { name: /DeepSeek 官方 API/ }));

  const providerName = screen.getByLabelText(/^名称/);
  const apiMode = screen.getByLabelText(/^接口模式/);
  const baseUrl = screen.getByLabelText(/^Base URL/);
  await waitFor(() => expect(providerName).toHaveValue('DeepSeek 官方 API'));
  expect(apiMode).toHaveValue('openai_compatible');
  expect(apiMode).toBeDisabled();
  expect(baseUrl).toHaveValue('https://api.deepseek.com/v1');
  expect(baseUrl).toBeDisabled();

  await user.click(within(pickerActions!).getByRole('button', { name: '创建提供商' }));
  await user.click(screen.getByRole('button', { name: /自定义连接/ }));

  await waitFor(() => expect(providerName).toHaveValue(''));
  expect(apiMode).toHaveValue('openai_compatible');
  expect(apiMode).toBeEnabled();
  expect(baseUrl).toHaveValue('https://api.openai.com/v1');
  expect(baseUrl).toBeEnabled();
});

test('unsaved new-provider draft is restored after the panel unmounts and remounts', async () => {
  const user = userEvent.setup();
  const first = render(panelTree());

  await user.type(screen.getByLabelText(/^名称/), '草稿提供商');
  await user.type(screen.getByLabelText(/^API Key/), 'sk-draft-key');
  await user.type(screen.getByLabelText(/^模型 ID/), 'model-a,model-b');

  first.unmount();
  render(panelTree());

  expect(screen.getByLabelText(/^名称/)).toHaveValue('草稿提供商');
  expect(screen.getByLabelText(/^API Key/)).toHaveValue('sk-draft-key');
  expect(screen.getByLabelText(/^模型 ID/)).toHaveValue('model-a,model-b');
  expect(screen.getByText('model-a')).toBeInTheDocument();
});

test('creating a provider keeps the saved API key in the editor', async () => {
  const user = userEvent.setup();
  const createdProvider: ModelProvider = {
    ...existingProvider,
    id: 'provider-created',
    name: '新提供商',
    masked_api_keys: ['****-key'],
    models: ['model-a'],
  };
  const onProviderAdd = vi.fn().mockResolvedValue(createdProvider);
  const { container } = render(panelTree({ onProviderAdd }));

  await user.type(screen.getByLabelText(/^名称/), '新提供商');
  await user.type(screen.getByLabelText(/^API Key/), 'sk-first-key');
  await user.type(screen.getByLabelText(/^模型 ID/), 'model-a');
  await user.click(within(container.querySelector('.provider-editor')!).getByRole('button', { name: '创建提供商' }));

  await waitFor(() => expect(onProviderAdd).toHaveBeenCalledWith(expect.objectContaining({
    api_keys: ['sk-first-key'],
  })));
  expect(screen.getByText('****-key')).toBeInTheDocument();
});

test('switching to an existing provider discards the new-provider draft', async () => {
  const user = userEvent.setup();
  const onProviderChange = vi.fn();
  const view = render(panelTree({ providers: [existingProvider], onProviderChange }));

  await user.type(screen.getByLabelText(/^名称/), '草稿提供商');
  await user.selectOptions(screen.getByLabelText('当前提供商'), 'provider-1');
  expect(onProviderChange).toHaveBeenCalledWith('provider-1');

  view.rerender(panelTree({
    providers: [existingProvider],
    onProviderChange,
    activeProviderId: 'provider-1',
  }));
  await waitFor(() => expect(screen.getByLabelText(/^名称/)).toHaveValue('已有提供商'));

  view.unmount();
  render(panelTree({ providers: [existingProvider], onProviderChange }));

  expect(screen.getByLabelText(/^名称/)).toHaveValue('');
});

test('image providers show image request fields instead of text model pricing', async () => {
  const user = userEvent.setup();
  render(panelTree());

  await user.click(screen.getAllByRole('button', { name: '创建提供商' })[0]);
  await user.click(screen.getByRole('button', { name: /自定义连接/ }));
  await user.selectOptions(screen.getByLabelText(/^接口模式/), 'openai_images');
  await user.type(screen.getByLabelText(/^模型 ID/), 'gpt-image-2');

  const size = screen.getByLabelText('gpt-image-2 尺寸');
  const count = screen.getByLabelText('gpt-image-2 数量');
  expect(size).toBeInTheDocument();
  expect(screen.getByLabelText('gpt-image-2 质量')).toBeInTheDocument();
  expect(count).toBeInTheDocument();
  expect(within(count).getByRole('option', { name: '无' })).toBeInTheDocument();
  expect(within(size).getByRole('option', { name: '无' })).toBeInTheDocument();
  expect(within(size).getByRole('option', { name: '自定义' })).toBeInTheDocument();
  await user.selectOptions(size, '');
  await user.selectOptions(count, '');
  expect(size).toHaveValue('');
  expect(count).toHaveValue('');
  expect(screen.getByLabelText('gpt-image-2 格式')).toBeInTheDocument();
  expect(screen.getByLabelText('gpt-image-2 背景')).toBeInTheDocument();
  expect(screen.getByLabelText('gpt-image-2 风格')).not.toBeDisabled();
  expect(screen.getByLabelText('gpt-image-2 响应格式')).not.toBeDisabled();
  expect(screen.getByLabelText('gpt-image-2 编辑精度')).not.toBeDisabled();
  expect(within(screen.getByLabelText('gpt-image-2 背景')).getByRole('option', { name: 'transparent' })).toBeInTheDocument();
  expect(screen.queryByLabelText('gpt-image-2 上下文')).not.toBeInTheDocument();
  expect(screen.queryByLabelText('gpt-image-2 输入')).not.toBeInTheDocument();
});
