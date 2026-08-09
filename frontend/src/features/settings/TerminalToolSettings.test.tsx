import { render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, expect, test, vi } from 'vitest';

import {
  listToolProcesses,
  pickDirectory,
  readEnvironmentRuntime,
  readToolDefaults,
} from '../../api';
import { FeedbackProvider } from '../../components/ui';
import { I18nProvider } from '../../i18n';
import { TerminalToolSettings } from './TerminalToolSettings';

vi.mock('../../api', () => ({
  installEnvironmentComponent: vi.fn(),
  listToolProcesses: vi.fn(),
  pickDirectory: vi.fn(),
  readAgentToolsConfig: vi.fn(),
  readEnvironmentRuntime: vi.fn(),
  readToolDefaults: vi.fn(),
  refreshEnvironmentComponent: vi.fn(),
  stopToolProcess: vi.fn(),
  updateAgentToolsConfig: vi.fn(),
  updateToolDefaults: vi.fn(),
}));

vi.mock('./WebSearchToolSettings', () => ({
  WebSearchToolSettings: () => null,
}));

const toolDefaults = {
  max_llm_iterations: 99,
  terminal: {
    enabled: true,
    permission_mode: 'request_approval' as const,
    approval_grants: [],
    default_timeout_ms: 30_000,
    allowed_cwd_roots: [],
  },
  web_search: {
    enabled: false,
    max_results: 5,
    timeout_ms: 15_000,
    key_selection: 'sequential' as const,
    extraction: {
      remote_enabled: true,
      max_remote_attempts_per_url: 3,
      allow_grok_fallback: true,
    },
    sources: [],
  },
  evidence_storage: {
    enabled: true,
    retention_days: 30,
    max_total_mb: 200,
  },
  task_planning: {
    enabled: true,
    progress_interval: 5,
  },
};

function renderSettings() {
  return render(
    <I18nProvider>
      <FeedbackProvider>
        <TerminalToolSettings scope="defaults" />
      </FeedbackProvider>
    </I18nProvider>,
  );
}

beforeEach(() => {
  vi.clearAllMocks();
  localStorage.setItem('weiagent-locale', 'zh');
  vi.mocked(readToolDefaults).mockResolvedValue(structuredClone(toolDefaults));
  vi.mocked(readEnvironmentRuntime).mockResolvedValue({
    supported: true,
    available: true,
    managed_installed: true,
    installing: false,
    target_version: '0.145.0',
    update_available: false,
    components: [],
  });
  vi.mocked(listToolProcesses).mockResolvedValue([]);
  vi.mocked(pickDirectory).mockResolvedValue({ path: 'D:\\Code_Save\\rust\\WeiAgent' });
});

test('browsing fills the path input and keeps add as the confirmation step', async () => {
  const user = userEvent.setup();
  renderSettings();

  const browseButton = await screen.findByRole('button', { name: '浏览' });
  const pathInput = screen.getByPlaceholderText('输入文件夹的完整路径');
  const addButton = screen.getByRole('button', { name: '添加' });

  expect(addButton).toBeDisabled();
  await user.click(browseButton);

  await waitFor(() => expect(pathInput).toHaveValue('D:\\Code_Save\\rust\\WeiAgent'));
  expect(pickDirectory).toHaveBeenCalledWith(undefined);
  expect(addButton).toBeEnabled();

  await user.click(addButton);
  const selectedPath = screen.getByText('D:\\Code_Save\\rust\\WeiAgent');
  expect(selectedPath).toBeVisible();
  expect(selectedPath.closest('li')).toHaveClass('allowed-root-item');
  expect(pathInput).toHaveValue('');
});

test('manual path entry still uses the existing add flow', async () => {
  const user = userEvent.setup();
  renderSettings();

  const pathInput = await screen.findByPlaceholderText('输入文件夹的完整路径');
  await user.type(pathInput, 'D:\\Projects\\Manual');
  await user.click(screen.getByRole('button', { name: '添加' }));

  expect(screen.getByText('D:\\Projects\\Manual')).toBeVisible();
  expect(pickDirectory).not.toHaveBeenCalled();
});
