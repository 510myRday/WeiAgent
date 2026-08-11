import { render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { useState } from 'react';
import { beforeEach, expect, test, vi } from 'vitest';

import { readAgentWorkspace } from '../../api';
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
  id: 'builtin-wei-sir',
  name: 'Wei Sir',
  description: '负责统筹任务、协调各智能体并推进复杂工作。',
  model_provider_id: 'provider-1',
  model_id: 'model-1',
  avatar_icon: 'bot',
  avatar_color: '#07c160',
  is_builtin: true,
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
            masked_api_keys: ['****'],
            key_strategy: 'failover',
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

  expect(nameInput).toHaveValue('喂先生');
  expect(descriptionInput).toHaveValue('负责统筹任务、协调各智能体并推进复杂工作。');

  await user.click(screen.getByRole('button', { name: 'English' }));

  expect(nameInput).toHaveValue('Wei Sir');
  expect(descriptionInput).toHaveValue(
    'Coordinates tasks, delegates work, and drives complex jobs to completion.',
  );
});

test('keeps the workspace editor focused when saving updates the agent list', async () => {
  const user = userEvent.setup();
  vi.mocked(readAgentWorkspace)
    .mockReset()
    .mockResolvedValueOnce({ personality: '', memory: '', instructions: '' })
    .mockResolvedValueOnce({ personality: 'server reload', memory: '', instructions: '' });
  const onAgentProfileSave = vi.fn().mockResolvedValue({ agent: builtinAgent, workspace: {
    personality: '',
    memory: '',
    instructions: '',
  } });

  function Harness() {
    const [agent, setAgent] = useState(builtinAgent);

    return (
      <I18nProvider>
        <FeedbackProvider>
          <AgentsPanel
            agents={[agent]}
            providers={[{
              id: 'provider-1',
              provider_type: 'openai_compatible',
              name: 'Provider',
              base_url: 'https://example.com',
              has_api_key: true,
              masked_api_keys: ['****'],
              key_strategy: 'failover',
              models: ['model-1'],
              model_configs: [],
              extra_body: {},
              created_at: '2026-01-01T00:00:00Z',
              updated_at: '2026-01-01T00:00:00Z',
            }]}
            activeAgentId={agent.id}
            isLoadingAgents={false}
            onAgentChange={() => {}}
            onAgentAdd={vi.fn()}
            onAgentProfileSave={async (...args) => {
              const result = await onAgentProfileSave(...args);
              setAgent({ ...agent, updated_at: '2026-08-11T00:00:00Z' });
              return result;
            }}
            onAgentRemove={vi.fn()}
            onOpenProviderSettings={() => {}}
            onOpenMain={() => {}}
            onBackToList={() => {}}
          />
        </FeedbackProvider>
      </I18nProvider>
    );
  }

  render(<Harness />);

  await user.click(screen.getByRole('tab', { name: /人格与指令|Personality & instructions/ }));
  const personality = await screen.findByRole('textbox', { name: /个性 \/ 人设|Personality/ });
  await user.click(personality);
  await user.type(personality, ' keep editing');

  expect(personality).toHaveFocus();
  await waitFor(() => expect(onAgentProfileSave).toHaveBeenCalledTimes(1), { timeout: 2000 });
  expect(personality).toHaveValue(' keep editing');
  expect(personality).toHaveFocus();
});
