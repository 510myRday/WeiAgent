import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, expect, test, vi } from 'vitest';

import type { Agent } from '../../types';
import { I18nProvider, useI18n } from '../../i18n';
import { FeedbackProvider } from '../../components/ui';
import { AgentsPanel } from './AgentsPanel';

vi.mock('../../api', () => ({
  readAgentWorkspace: vi.fn().mockResolvedValue({
    personality: '',
    memory: '',
    instructions: '',
  }),
}));

const builtinAgent: Agent = {
  id: 'builtin-master',
  name: '总管',
  description: '负责统筹任务、协调各智能体并推进复杂工作。',
  model_provider_id: 'provider-1',
  model_id: 'model-1',
  avatar_icon: 'bot',
  avatar_color: '#07c160',
  is_builtin: true,
  context_compress_threshold_percent: 90,
  created_at: '2026-01-01T00:00:00Z',
  updated_at: '2026-01-01T00:00:00Z',
};

function LocaleSwitch() {
  const { setLocale } = useI18n();
  return (
    <button type="button" onClick={() => setLocale('en')}>
      English
    </button>
  );
}

beforeEach(() => {
  localStorage.setItem('weiagent-locale', 'zh');
});

test('built-in agent fields follow the current locale without changing the stored values', async () => {
  const user = userEvent.setup();
  const { container } = render(
    <I18nProvider>
      <FeedbackProvider>
        <LocaleSwitch />
        <AgentsPanel
          agents={[builtinAgent]}
          providers={[{
            id: 'provider-1',
            provider_type: 'openai_compatible',
            name: 'Provider',
            base_url: 'https://example.com',
            has_api_key: true,
            masked_api_key: '****',
            models: ['model-1'],
            model_configs: [],
            extra_body: {},
            created_at: '2026-01-01T00:00:00Z',
            updated_at: '2026-01-01T00:00:00Z',
          }]}
          activeAgentId={builtinAgent.id}
          isLoadingAgents={false}
          onAgentChange={() => {}}
          onAgentAdd={vi.fn()}
          onAgentProfileSave={vi.fn()}
          onAgentRemove={vi.fn()}
          onOpenProviderSettings={() => {}}
          onOpenMain={() => {}}
          onBackToList={() => {}}
        />
      </FeedbackProvider>
    </I18nProvider>,
  );

  const nameInput = container.querySelector<HTMLInputElement>('#agent-name');
  const descriptionInput = container.querySelector<HTMLTextAreaElement>('#agent-description');

  expect(nameInput).toHaveValue('总管');
  expect(descriptionInput).toHaveValue('负责统筹任务、协调各智能体并推进复杂工作。');

  await user.click(screen.getByRole('button', { name: 'English' }));

  expect(nameInput).toHaveValue('Master');
  expect(descriptionInput).toHaveValue(
    'Coordinates tasks, delegates work, and drives complex jobs to completion.',
  );
});
