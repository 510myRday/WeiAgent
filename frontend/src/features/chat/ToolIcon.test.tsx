import { render } from '@testing-library/react';
import { describe, expect, test } from 'vitest';

import type { McpServer } from '../../toolTypes';
import { ToolIcon } from './ToolIcon';

function serverWithTools(tools: Array<{ name: string; qualified_name: string }>): McpServer {
  return {
    id: 'server-id',
    name: 'Editor MCP',
    description: '',
    enabled: true,
    logo_url: 'https://example.com/editor-mcp.png',
    tags: [],
    transport: { type: 'stdio', command: 'editor-mcp', args: [], env: {} },
    tools: tools.map((tool) => ({
      ...tool,
      tool_id: `tool-${tool.name}`,
      description: '',
      input_schema: { type: 'object' },
      enabled: true,
    })),
    status: 'ready',
  };
}

describe('ToolIcon', () => {
  test('keeps the built-in icon when an MCP tool has the same raw name', () => {
    const mcpServer = serverWithTools([
      { name: 'multi_edit_file', qualified_name: 'mcp_Editor_MCP_multi_edit_file' },
    ]);
    const { container } = render(<ToolIcon toolName="multi_edit_file" mcpServers={[mcpServer]} />);

    expect(container.querySelector('svg.lucide-file-pen-line')).not.toBeNull();
    expect(container.querySelector('img')).toBeNull();
  });

  test('uses the configured MCP logo for the qualified MCP tool name', () => {
    const mcpServer = serverWithTools([
      { name: 'search', qualified_name: 'mcp_Editor_MCP_search' },
    ]);
    const { container } = render(
      <ToolIcon toolName="mcp_Editor_MCP_search" mcpServers={[mcpServer]} />,
    );

    expect(container.querySelector('img')).toHaveAttribute('src', 'https://example.com/editor-mcp.png');
  });
});
