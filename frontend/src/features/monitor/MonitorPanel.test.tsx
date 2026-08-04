import { render, screen, waitFor } from '@testing-library/react';
import { beforeEach, expect, test, vi } from 'vitest';

import type { AppSettings } from '../../types';
import { FeedbackProvider } from '../../components/ui';
import { I18nProvider } from '../../i18n';
import { runA11yCheck } from '../../test/a11y';
import { MonitorPanel } from './MonitorPanel';

const { listLlmLogs } = vi.hoisted(() => ({ listLlmLogs: vi.fn() }));

vi.mock('../../api', () => ({
  getLlmLog: vi.fn(),
  listLlmLogs,
}));

const settings: AppSettings = {
  host: '127.0.0.1',
  port: 4311,
  log_level: 'info',
  avatar_icon: 'bot',
  avatar_color: '#07c160',
  language: 'zh',
  request_timeout_seconds: 30,
  request_retry_attempts: 2,
  context_compress_threshold_percent: 80,
  llm_log_enabled: true,
  conversation_title_provider_id: null,
  conversation_title_model_id: null,
};

beforeEach(() => {
  listLlmLogs.mockResolvedValue({
    items: [],
    total: 0,
    offset: 0,
    limit: 10,
    agent_stats: [],
    model_stats: [{
      model_key: 'provider:model',
      provider_name: 'Provider',
      model_id: 'model',
      request_count: 1,
      latest_at: '2026-08-04T00:00:00.000Z',
    }],
    summary: {
      request_count: 0,
      success_count: 0,
      error_count: 0,
      input_tokens: 0,
      output_tokens: 0,
      cache_read_tokens: 0,
      cache_write_tokens: 0,
      cost_usd: 0,
    },
  });
});

test('Monitor model scope keeps button semantics and has no axe violations', async () => {
  const { container } = render(
    <I18nProvider>
      <FeedbackProvider>
        <MonitorPanel
          agents={[]}
          providers={[]}
          settings={settings}
          onSettingsSave={async (next) => next}
          onOpenMain={() => undefined}
          onBackToList={() => undefined}
        />
      </FeedbackProvider>
    </I18nProvider>,
  );

  const modelButton = await screen.findByRole('button', { name: /Provider.*model/ });
  expect(modelButton).toHaveAttribute('aria-pressed', 'false');
  expect(modelButton).not.toHaveAttribute('role', 'listitem');
  await waitFor(() => expect(modelButton).toBeInTheDocument());
  const a11y = await runA11yCheck(container);
  expect(a11y.violations).toHaveLength(0);
});
