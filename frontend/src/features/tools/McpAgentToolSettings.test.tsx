import { render, screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, expect, test, vi } from 'vitest';

import type { McpAgentSettings } from '../../api';
import { readAgentMcpConfig, updateAgentMcpConfig } from '../../api';
import { FeedbackProvider } from '../../components/ui';
import { I18nProvider } from '../../i18n';
import { McpAgentToolSettings } from './McpAgentToolSettings';

vi.mock('../../api', () => ({
  readAgentMcpConfig: vi.fn(),
  updateAgentMcpConfig: vi.fn(),
}));

const settings: McpAgentSettings = {
  defaults: [
    {
      id: 'server-1',
      name: 'Local MCP',
      description: 'Local tools',
      enabled: true,
      provider: null,
      provider_url: null,
      logo_url: null,
      tags: [],
      transport: { type: 'stdio', command: 'local-mcp', args: [], env: {} },
      tools: [
        {
          name: 'read_file',
          qualified_name: 'mcp__server1__read_file',
          title: 'Read file',
          description: 'Read a local file',
          input_schema: {},
          enabled: true,
        },
        {
          name: 'write_file',
          qualified_name: 'mcp__server1__write_file',
          title: 'Write file',
          description: 'Write a local file',
          input_schema: {},
          enabled: false,
        },
      ],
      status: 'ready',
    },
  ],
  overrides: {
    servers: [],
    tools: [{ server_id: 'server-1', tool_name: 'write_file', enabled: false }],
  },
  effective: [],
};

settings.effective = structuredClone(settings.defaults);

function renderSettings(props: React.ComponentProps<typeof McpAgentToolSettings>) {
  return render(
    <I18nProvider>
      <FeedbackProvider>
        <McpAgentToolSettings {...props} />
      </FeedbackProvider>
    </I18nProvider>,
  );
}

beforeEach(() => {
  vi.clearAllMocks();
  localStorage.setItem('weiagent-locale', 'zh');
  vi.mocked(readAgentMcpConfig).mockResolvedValue(structuredClone(settings));
  vi.mocked(updateAgentMcpConfig).mockResolvedValue(structuredClone(settings));
});

test('summary renders each MCP tool as an independent agent tool row', async () => {
  renderSettings({ agentId: 'agent-1', view: 'summary', onEdit: vi.fn() });

  expect(await screen.findByText('Read file')).toBeVisible();
  expect(screen.getByText('Write file')).toBeVisible();
  expect(screen.queryByText('MCP 服务设置')).not.toBeInTheDocument();
  expect(screen.getAllByText('MCP')).toHaveLength(2);
  expect(screen.getAllByRole('button', { name: '编辑工具配置' })).toHaveLength(2);
});

test('editing an MCP tool targets its server and tool name', async () => {
  const user = userEvent.setup();
  const onEdit = vi.fn();
  renderSettings({ agentId: 'agent-1', view: 'summary', onEdit });

  const row = (await screen.findByText('Read file')).closest('.agent-tool-row');
  expect(row).not.toBeNull();
  const editButton = within(row as HTMLElement).getByRole('button', { name: '编辑工具配置' });
  await user.click(editButton);

  expect(onEdit).toHaveBeenCalledWith({ serverId: 'server-1', toolName: 'read_file' });
});

test('single MCP editor only shows the selected tool', async () => {
  renderSettings({
    agentId: 'agent-1',
    view: 'edit',
    selection: { serverId: 'server-1', toolName: 'write_file' },
  });

  expect(await screen.findByRole('heading', { name: 'Write file' })).toBeVisible();
  expect(screen.queryByRole('heading', { name: 'Read file' })).not.toBeInTheDocument();
  expect(screen.getByText('跟随全局默认')).toBeVisible();
});
