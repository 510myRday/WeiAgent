import { fireEvent, render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { expect, test, vi } from 'vitest';

import { I18nProvider } from '../../i18n';
import { McpServerDialog } from './McpServerDialog';

test('MCP editor can return to visual mode while JSON is incomplete', async () => {
  const user = userEvent.setup();

  render(
    <I18nProvider>
      <McpServerDialog
        open
        saving={false}
        testing={false}
        onClose={vi.fn()}
        onSave={vi.fn()}
        onTest={vi.fn()}
      />
    </I18nProvider>,
  );

  const visualTab = screen.getByRole('tab', { name: '可视化配置' });
  const jsonTab = screen.getByRole('tab', { name: 'JSON 配置' });

  await user.click(jsonTab);
  await user.clear(screen.getByRole('textbox', { name: 'MCP JSON 配置' }));
  await user.click(visualTab);

  expect(screen.getByRole('textbox', { name: '服务名称' })).toBeVisible();
  expect(visualTab).not.toBeDisabled();
});

test('MCP editor exposes the enabled switch beside the editor mode controls', async () => {
  const user = userEvent.setup();

  render(
    <I18nProvider>
      <McpServerDialog
        open
        saving={false}
        testing={false}
        onClose={vi.fn()}
        onSave={vi.fn()}
        onTest={vi.fn()}
      />
    </I18nProvider>,
  );

  const enabledSwitch = screen.getByRole('checkbox', { name: '启用' });
  expect(enabledSwitch).toBeChecked();

  await user.click(enabledSwitch);

  expect(enabledSwitch).not.toBeChecked();
});

test('MCP editor places working directory and request timeout in advanced settings', async () => {
  const user = userEvent.setup();

  render(
    <I18nProvider>
      <McpServerDialog
        open
        saving={false}
        testing={false}
        onClose={vi.fn()}
        onSave={vi.fn()}
        onTest={vi.fn()}
      />
    </I18nProvider>,
  );

  expect(screen.queryByRole('textbox', { name: '工作目录' })).not.toBeInTheDocument();
  expect(screen.queryByRole('spinbutton', { name: '请求超时（秒）' })).not.toBeInTheDocument();

  await user.click(screen.getByRole('button', { name: /高级设置/ }));

  expect(screen.getByRole('textbox', { name: '工作目录' })).toBeVisible();
  expect(screen.getByRole('spinbutton', { name: '请求超时（秒）' })).toBeVisible();
});

test('MCP HTTP transports use a full-width token and fill advanced settings without a cwd gap', async () => {
  const user = userEvent.setup();

  render(
    <I18nProvider>
      <McpServerDialog
        open
        saving={false}
        testing={false}
        onClose={vi.fn()}
        onSave={vi.fn()}
        onTest={vi.fn()}
      />
    </I18nProvider>,
  );

  const transport = screen.getByRole('combobox', { name: '传输方式' });
  for (const type of ['streamable_http', 'sse']) {
    await user.selectOptions(transport, type);

    const token = screen.getByLabelText('Bearer Token');
    expect(token.closest('.form-field')?.parentElement).not.toHaveClass('form-grid', 'two-column');

    if (!screen.getByRole('button', { name: /高级设置/ }).getAttribute('aria-expanded')?.includes('true')) {
      await user.click(screen.getByRole('button', { name: /高级设置/ }));
    }

    expect(screen.queryByRole('textbox', { name: '工作目录' })).not.toBeInTheDocument();
    const timeout = screen.getByRole('spinbutton', { name: '请求超时（秒）' });
    expect(timeout.closest('.form-field')).toHaveClass('mcp-advanced-timeout-wide');
    expect(screen.getByRole('textbox', { name: '提供者' })).toBeVisible();
    expect(screen.getByRole('textbox', { name: '提供者网址' })).toBeVisible();
  }
});

test('MCP editor accepts both comma separators for tags and removes cancel action', async () => {
  const user = userEvent.setup();
  const onSave = vi.fn(async () => {});

  render(
    <I18nProvider>
      <McpServerDialog
        open
        saving={false}
        testing={false}
        onClose={vi.fn()}
        onSave={onSave}
        onTest={vi.fn()}
      />
    </I18nProvider>,
  );

  expect(screen.getByRole('option', { name: 'SSE' })).toBeInTheDocument();
  expect(screen.queryByRole('button', { name: '取消' })).not.toBeInTheDocument();

  await user.type(screen.getByRole('textbox', { name: '服务名称' }), 'Test MCP');
  await user.type(screen.getByRole('textbox', { name: '启动命令' }), 'npx');
  await user.click(screen.getByRole('button', { name: /高级设置/ }));
  fireEvent.change(screen.getByRole('textbox', { name: '标签' }), { target: { value: '文件，搜索,本地' } });
  await user.click(screen.getByRole('button', { name: '保存' }));

  expect(onSave).toHaveBeenCalledWith(expect.objectContaining({ tags: ['文件', '搜索', '本地'] }));
});

test('MCP editor blocks a duplicate server name before saving', async () => {
  const user = userEvent.setup();
  const onSave = vi.fn(async () => {});
  const existingServer = {
    id: 'server-1',
    name: 'Local MCP',
    description: '',
    enabled: true,
    provider: null,
    provider_url: null,
    logo_url: null,
    tags: [],
    transport: { type: 'stdio' as const, command: 'npx', args: [], cwd: null, env: {}, long_running: false, timeout_ms: 60000 },
    tools: [],
    status: 'unknown' as const,
  };

  render(
    <I18nProvider>
      <McpServerDialog
        open
        existingServers={[existingServer]}
        saving={false}
        testing={false}
        onClose={vi.fn()}
        onSave={onSave}
        onTest={vi.fn()}
      />
    </I18nProvider>,
  );

  await user.type(screen.getByRole('textbox', { name: '服务名称' }), ' local mcp ');

  expect(screen.getByText('服务名称已存在，请修改后再保存。')).toBeVisible();
  expect(screen.getByRole('button', { name: '保存' })).toBeDisabled();
  expect(onSave).not.toHaveBeenCalled();
});

test('MCP visual command arguments use one command-line input', () => {
  render(
    <I18nProvider>
      <McpServerDialog
        open
        saving={false}
        testing={false}
        onClose={vi.fn()}
        onSave={vi.fn()}
        onTest={vi.fn()}
      />
    </I18nProvider>,
  );

  const args = screen.getByRole('textbox', { name: '命令参数' });

  fireEvent.change(args, { target: { value: '--from "D:/Code Save/project" wecom-mail-mcp' } });

  expect(args).toHaveValue('--from "D:/Code Save/project" wecom-mail-mcp');
});

test('MCP JSON view keeps the server id out of the editable document', async () => {
  const user = userEvent.setup();
  const server = {
    id: 'server-1',
    name: 'Existing MCP',
    description: '',
    enabled: true,
    provider: null,
    provider_url: null,
    logo_url: null,
    tags: [],
    transport: { type: 'stdio' as const, command: 'npx', args: [], cwd: null, env: {}, long_running: false, timeout_ms: 60000 },
    tools: [],
    status: 'unknown' as const,
  };

  render(
    <I18nProvider>
      <McpServerDialog
        open
        server={server}
        secrets={{ type: 'stdio', command: 'npx', args: [], cwd: null, env: { API_KEY: 'real-secret' }, long_running: false, timeout_ms: 60000 }}
        saving={false}
        testing={false}
        onClose={vi.fn()}
        onSave={vi.fn()}
        onTest={vi.fn()}
      />
    </I18nProvider>,
  );

  await user.click(screen.getByRole('tab', { name: 'JSON 配置' }));

  const json = screen.getByRole('textbox', { name: 'MCP JSON 配置' }) as HTMLTextAreaElement;
  expect(json.value).not.toContain('"id"');
});

test('MCP environment variables use editable rows and preserve secrets in JSON data', async () => {
  const user = userEvent.setup();
  const onSave = vi.fn(async () => {});

  render(
    <I18nProvider>
      <McpServerDialog
        open
        saving={false}
        testing={false}
        onClose={vi.fn()}
        onSave={onSave}
        onTest={vi.fn()}
      />
    </I18nProvider>,
  );

  await user.click(screen.getByRole('button', { name: /添加环境变量/ }));
  const key = screen.getByRole('textbox', { name: '变量名' });
  const value = screen.getByLabelText('变量值');

  expect(value).toHaveAttribute('type', 'password');
  await user.type(key, 'API_KEY');
  await user.type(value, 'secret');
  expect(value).toHaveValue('secret');

  await user.click(screen.getByRole('button', { name: '显示变量值' }));
  expect(value).toHaveAttribute('type', 'text');

  await user.type(screen.getByRole('textbox', { name: '服务名称' }), 'Test MCP');
  await user.type(screen.getByRole('textbox', { name: '启动命令' }), 'npx');
  await user.click(screen.getByRole('button', { name: '保存' }));
  expect(onSave).toHaveBeenCalledWith(expect.objectContaining({
    transport: expect.objectContaining({ env: { API_KEY: 'secret' } }),
  }));
});

test('MCP visual values reveal the real secret while JSON keeps the preserve marker', async () => {
  const user = userEvent.setup();
  const server = {
    id: 'server-1',
    name: 'Existing MCP',
    description: '',
    enabled: true,
    provider: null,
    provider_url: null,
    logo_url: null,
    tags: [],
    transport: { type: 'stdio' as const, command: 'npx', args: [], cwd: null, env: { API_KEY: '__preserve__' }, long_running: false, timeout_ms: 60000 },
    tools: [],
    status: 'unknown' as const,
  };

  render(
    <I18nProvider>
      <McpServerDialog
        open
        server={server}
        secrets={{ type: 'stdio', command: 'npx', args: [], cwd: null, env: { API_KEY: 'real-secret' }, long_running: false, timeout_ms: 60000 }}
        saving={false}
        testing={false}
        onClose={vi.fn()}
        onSave={vi.fn()}
        onTest={vi.fn()}
      />
    </I18nProvider>,
  );

  const value = screen.getByLabelText('变量值');
  expect(value).toHaveAttribute('type', 'password');
  expect(value).toHaveValue('real-secret');

  await user.click(screen.getByRole('button', { name: '显示变量值' }));

  expect(value).toHaveAttribute('type', 'text');
  expect(value).toHaveValue('real-secret');

  await user.click(screen.getByRole('tab', { name: 'JSON 配置' }));
  expect((screen.getByRole('textbox', { name: 'MCP JSON 配置' }) as HTMLTextAreaElement).value).toContain('"API_KEY": "__preserve__"');
});
