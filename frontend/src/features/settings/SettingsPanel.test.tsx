import { render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, expect, test, vi } from 'vitest';

import type { AppSettings } from '../../types';
import { I18nProvider } from '../../i18n';
import { FeedbackProvider } from '../../components/ui';
import { SettingsPanel } from './SettingsPanel';

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
});

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
  expect(templateGrid!.querySelectorAll('img')).toHaveLength(14);
  expect(screen.getByRole('button', { name: /Google Gemini API/ })).toBeInTheDocument();
  expect(screen.getByRole('button', { name: /Anthropic 官方 API/ })).toBeInTheDocument();
  expect(screen.getByRole('button', { name: /Kimi 开放平台/ })).toBeInTheDocument();
  expect(screen.getByRole('button', { name: /智谱 AI 开放平台/ })).toBeInTheDocument();
  const newTemplates = [
    ['NVIDIA NIM', '/model-providers/nvidia.svg', 'https://integrate.api.nvidia.com/v1'],
    ['阿里云百炼', '/model-providers/alibaba-bailian.svg', 'https://dashscope.aliyuncs.com/compatible-mode/v1'],
    ['OpenRouter', '/model-providers/openrouter.svg', 'https://openrouter.ai/api/v1'],
    ['硅基流动', '/model-providers/siliconflow.svg', 'https://api.siliconflow.cn/v1'],
    ['阶跃星辰', '/model-providers/stepfun.svg', 'https://api.stepfun.com/v1'],
    ['xAI Grok', '/model-providers/grok.svg', 'https://api.x.ai/v1'],
    ['MiniMax', '/model-providers/minimax.svg', 'https://api.minimaxi.com/v1'],
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
