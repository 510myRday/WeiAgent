import { fireEvent, render, screen } from '@testing-library/react';
import { beforeEach, expect, test, vi } from 'vitest';

import type { Agent } from '../../types';
import { I18nProvider } from '../../i18n';
import { ChatAgentSelect } from './ChatAgentSelect';

const agents: Agent[] = [
  {
    id: 'agent-master',
    name: 'Master',
    description: '',
    model_provider_id: 'provider-1',
    model_id: 'model-1',
    avatar_icon: 'smile',
    avatar_color: '#ff4fa3',
    is_builtin: true,
    created_at: '2026-07-17T00:00:00Z',
    updated_at: '2026-07-17T00:00:00Z',
  },
  {
    id: 'agent-writer',
    name: '文案助手',
    description: '',
    model_provider_id: 'provider-1',
    model_id: 'model-2',
    avatar_icon: 'sparkles',
    avatar_color: '#07c160',
    is_builtin: false,
    created_at: '2026-07-17T00:00:00Z',
    updated_at: '2026-07-17T00:00:00Z',
  },
];

beforeEach(() => {
  localStorage.setItem('weiagent-locale', 'zh');
});

test('lists conversation agents and reports the selected configuration target', () => {
  const onChange = vi.fn();
  render(
    <I18nProvider>
      <ChatAgentSelect agents={agents} value="agent-master" onChange={onChange} />
    </I18nProvider>,
  );

  const select = screen.getByRole('combobox', { name: '当前智能体' });
  expect(select).toHaveValue('agent-master');
  expect(screen.getByRole('option', { name: '总管' })).toBeInTheDocument();
  expect(screen.getByRole('option', { name: '文案助手' })).toBeInTheDocument();

  fireEvent.change(select, { target: { value: 'agent-writer' } });
  expect(onChange).toHaveBeenCalledWith('agent-writer');
});
