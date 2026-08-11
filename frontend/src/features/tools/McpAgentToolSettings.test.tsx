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
          description: 'Read a local file. '.repeat(20),
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
    tools: [],
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
  vi.mocked(updateAgentMcpConfig).mockImplementation(async (_agentId, overrides) => {
    const next = structuredClone(settings);
    next.overrides = structuredClone(overrides);
    next.effective = structuredClone(next.defaults);
    for (const serverOverride of overrides.servers) {
      const server = next.effective.find((item) => item.id === serverOverride.server_id);
      if (server && serverOverride.enabled !== undefined) server.enabled = serverOverride.enabled;
    }
    for (const toolOverride of overrides.tools) {
      const server = next.effective.find((item) => item.id === toolOverride.server_id);
      const tool = server?.tools.find((item) => item.name === toolOverride.tool_name);
      if (tool && toolOverride.enabled !== undefined) tool.enabled = toolOverride.enabled;
    }
    return next;
  });
});

test('summary renders configured MCP servers instead of individual tools', async () => {
  renderSettings({ agentId: 'agent-1', view: 'summary', onEdit: vi.fn() });

  expect(await screen.findByText('Local MCP')).toBeVisible();
  expect(screen.queryByText('Read file')).not.toBeInTheDocument();
  expect(screen.getAllByText('MCP')).toHaveLength(1);
  expect(screen.getByRole('checkbox', { name: 'Local MCP 继承全局默认' })).toBeChecked();
  expect(screen.getByRole('checkbox', { name: 'Local MCP 已启用' })).toBeChecked();
  expect(screen.getByRole('checkbox', { name: 'Local MCP 已启用' })).toBeDisabled();
  expect(screen.getByRole('button', { name: '修改服务工具' })).toBeDisabled();
});

test('unchecking inheritance enables editing the MCP server tools', async () => {
  const user = userEvent.setup();
  const onEdit = vi.fn();
  renderSettings({ agentId: 'agent-1', view: 'summary', onEdit });

  const checkbox = await screen.findByRole('checkbox', { name: 'Local MCP 继承全局默认' });
  await user.click(checkbox);

  expect(updateAgentMcpConfig).toHaveBeenCalledWith('agent-1', {
    servers: [{ server_id: 'server-1', enabled: true, customized: true }],
    tools: [],
  });
  const row = (await screen.findByText('Local MCP')).closest('.agent-tool-row');
  expect(row).not.toBeNull();
  const editButton = within(row as HTMLElement).getByRole('button', { name: '修改服务工具' });
  expect(editButton).toBeEnabled();
  expect(within(row as HTMLElement).getByRole('checkbox', { name: 'Local MCP 已启用' })).toBeEnabled();
  await user.click(editButton);

  expect(onEdit).toHaveBeenCalledWith({ serverId: 'server-1' });
});

test('MCP server editor shows all tools and edits individual switches', async () => {
  const user = userEvent.setup();
  renderSettings({
    agentId: 'agent-1',
    view: 'edit',
    selection: { serverId: 'server-1' },
  });

  expect(await screen.findByRole('heading', { name: 'Local MCP' })).toBeVisible();
  expect(screen.getByText('Read file')).toBeVisible();
  expect(screen.getByText('Write file')).toBeVisible();
  expect(screen.getAllByText('查看 Schema')).toHaveLength(2);
  const search = screen.getByRole('textbox', { name: '搜索工具' });
  await user.type(search, 'write');
  expect(screen.queryByText('Read file')).not.toBeInTheDocument();
  expect(screen.getByText('Write file')).toBeVisible();
  await user.clear(search);
  const expandDescription = screen.getByRole('button', { name: '展开描述' });
  await user.click(expandDescription);
  expect(screen.getByRole('button', { name: '收起描述' })).toBeVisible();

  await user.click(screen.getByRole('checkbox', { name: 'Read file 已启用' }));
  expect(updateAgentMcpConfig).toHaveBeenCalledWith('agent-1', {
    servers: [{ server_id: 'server-1', enabled: true, customized: true }],
    tools: [{ server_id: 'server-1', tool_name: 'read_file', enabled: false }],
  });
});
