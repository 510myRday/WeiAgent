import {
  BookMarked,
  BookOpen,
  Cable,
  FileSearch,
  FilePenLine,
  ListChecks,
  List,
  MonitorPlay,
  Search,
  SquareTerminal,
  Wrench,
} from 'lucide-react';

import type { McpServer } from '../../toolTypes';
import { McpServerIcon } from '../tools/McpServerIcon';

const TOOL_ICON_SIZE = 14;

function builtinToolIcon(name: string) {
  switch (name.trim().toLowerCase()) {
    case 'execute_terminal':
      return <SquareTerminal size={TOOL_ICON_SIZE} aria-hidden="true" />;
    case 'terminal_session':
      return <MonitorPlay size={TOOL_ICON_SIZE} aria-hidden="true" />;
    case 'multi_edit_file':
      return <FilePenLine size={TOOL_ICON_SIZE} aria-hidden="true" />;
    case 'skill_view':
      return <BookMarked size={TOOL_ICON_SIZE} aria-hidden="true" />;
    case 'web_search':
      return <Search size={TOOL_ICON_SIZE} aria-hidden="true" />;
    case 'web_search_sources':
      return <List size={TOOL_ICON_SIZE} aria-hidden="true" />;
    case 'web_extract':
      return <FileSearch size={TOOL_ICON_SIZE} aria-hidden="true" />;
    case 'task_planning':
      return <ListChecks size={TOOL_ICON_SIZE} aria-hidden="true" />;
    case 'builtin_skills':
      return <BookOpen size={TOOL_ICON_SIZE} aria-hidden="true" />;
    default:
      return null;
  }
}

function findMcpServer(toolName: string, mcpServers: McpServer[]) {
  const qualifiedMatches = mcpServers.filter((server) => server.tools.some((tool) => (
    tool.qualified_name === toolName
  )));
  if (qualifiedMatches.length === 1) return qualifiedMatches[0];

  // Keep support for older persisted calls that used the raw MCP tool name,
  // but only when that name identifies one service unambiguously.
  const rawMatches = mcpServers.filter((server) => server.tools.some((tool) => (
    tool.name === toolName
  )));
  return rawMatches.length === 1 ? rawMatches[0] : undefined;
}

function isMcpToolName(toolName: string): boolean {
  return toolName.startsWith('mcp_') || toolName.startsWith('mcp__');
}

export function ToolIcon({ toolName, mcpServers }: { toolName: string; mcpServers: McpServer[] }) {
  const builtinIcon = builtinToolIcon(toolName);
  if (builtinIcon) {
    return <span className="message-tool__tool-icon" aria-hidden="true">{builtinIcon}</span>;
  }

  const mcpServer = findMcpServer(toolName, mcpServers);
  if (mcpServer) {
    return (
      <span className="message-tool__tool-icon" aria-hidden="true">
        <McpServerIcon logoUrl={mcpServer.logo_url} size={TOOL_ICON_SIZE} />
      </span>
    );
  }

  return (
    <span className="message-tool__tool-icon" aria-hidden="true">
      {isMcpToolName(toolName)
        ? <Cable size={TOOL_ICON_SIZE} />
        : <Wrench size={TOOL_ICON_SIZE} />}
    </span>
  );
}
