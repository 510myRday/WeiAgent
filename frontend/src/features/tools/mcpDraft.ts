import type { McpServer, McpServerDraft, McpTransport } from '../../toolTypes';
import { isRecord } from '../../utils/isRecord';

export const EMPTY_MCP_DRAFT: McpServerDraft = {
  name: '',
  description: '',
  enabled: true,
  supports_parallel_tool_calls: true,
  provider: null,
  provider_url: null,
  logo_url: null,
  tags: [],
  transport: { type: 'stdio', command: '', args: [], cwd: null, env: {}, inherit_env: false, env_allowlist: [], long_running: true, timeout_ms: 60000 },
};

export function redactTransport(transport: McpTransport): McpTransport {
  if (transport.type === 'stdio') {
    return { ...transport, env: Object.fromEntries(Object.entries(transport.env).map(([key, value]) => [key, value ? '__preserve__' : value])) };
  }
  const oauth = transport.oauth
    ? { ...transport.oauth, client_secret: transport.oauth.client_secret ? '__preserve__' : transport.oauth.client_secret }
    : transport.oauth;
  const identity_header = transport.identity_header
    ? { ...transport.identity_header, value: transport.identity_header.value ? '__preserve__' : transport.identity_header.value }
    : transport.identity_header;
  return {
    ...transport,
    bearer_token: transport.bearer_token ? '__preserve__' : transport.bearer_token,
    headers: Object.fromEntries(Object.entries(transport.headers).map(([key, value]) => [key, value ? '__preserve__' : value])),
    identity_header,
    oauth,
  };
}

export function argsToText(args: string[]): string {
  return args.map((arg) => (arg.includes(' ') ? `"${arg}"` : arg)).join(' ');
}

export function textToArgs(value: string): string[] {
  return value.match(/(?:[^\s"]+|"[^"]*")+/g)?.map((arg) => arg.replace(/^"|"$/g, '')) ?? [];
}

export function normalizeStdioArgs(args: string[]): string[] {
  if (args.length !== 1 || !args[0].trim().startsWith('-') || !/[\s]/.test(args[0])) return args;
  return textToArgs(args[0]);
}

export function draftFromServer(server?: McpServer, secrets?: McpTransport): McpServerDraft {
  if (!server) return structuredClone(EMPTY_MCP_DRAFT);
  const transport = secrets ?? redactTransport(server.transport);
  return {
    id: server.id,
    name: server.name,
    description: server.description,
    enabled: server.enabled,
    supports_parallel_tool_calls: server.supports_parallel_tool_calls ?? true,
    provider: server.provider ?? null,
    provider_url: server.provider_url ?? null,
    logo_url: server.logo_url ?? null,
    tags: server.tags ?? [],
    transport: transport.type === 'stdio'
      ? { ...transport, args: normalizeStdioArgs(transport.args) }
      : transport,
  };
}

export function parseLines(value: string): string[] {
  return value.split(/\r?\n/).map((line) => line.trim()).filter(Boolean);
}

export function parseTags(value: string): string[] {
  return value.split(/[,，]/).map((tag) => tag.trim()).filter(Boolean);
}

export function mapEntries(value: string): Record<string, string> {
  return Object.fromEntries(parseLines(value).map((line) => {
    const separator = line.indexOf('=');
    return separator < 0 ? [line, ''] : [line.slice(0, separator).trim(), line.slice(separator + 1).trim()];
  }).filter(([key]) => key));
}

export function entriesText(entries: Record<string, string>): string {
  return Object.entries(entries).map(([key, value]) => `${key}=${value}`).join('\n');
}

export function serializeJsonDraft(draft: McpServerDraft): string {
  const jsonDraft = { ...draft };
  Reflect.deleteProperty(jsonDraft, 'id');
  jsonDraft.transport = redactTransport(draft.transport);
  return JSON.stringify(jsonDraft, null, 2);
}

export function getStdioArgsText(transport: McpTransport): string {
  return transport.type === 'stdio' ? argsToText(normalizeStdioArgs(transport.args ?? [])) : '';
}

export function validateMcpDraft(value: unknown, t: (key: string) => string): string {
  if (!isRecord(value) || typeof value.name !== 'string' || !value.name.trim()) return t('tools.mcp.jsonNameError');
  if (!isRecord(value.transport) || typeof value.transport.type !== 'string') return t('tools.mcp.jsonTransportError');

  const transport = value.transport;
  const transportType = typeof transport.type === 'string' ? transport.type : '';
  if (!['stdio', 'streamable_http', 'sse'].includes(transportType)) return t('tools.mcp.jsonTransportError');
  if (value.tags !== undefined && (!Array.isArray(value.tags) || value.tags.some((item) => typeof item !== 'string'))) return t('tools.mcp.jsonTagsError');
  if (transportType === 'stdio') {
    if (typeof transport.command !== 'string' || !transport.command.trim()) return t('tools.mcp.jsonCommandError');
    if (transport.args !== undefined && (!Array.isArray(transport.args) || transport.args.some((item) => typeof item !== 'string'))) return t('tools.mcp.jsonArrayError');
    if (transport.timeout_ms !== undefined && (typeof transport.timeout_ms !== 'number' || !Number.isFinite(transport.timeout_ms))) return t('tools.mcp.jsonTimeoutError');
  } else {
    if (typeof transport.url !== 'string' || !transport.url.trim()) return t('tools.mcp.jsonUrlError');
    if (transport.timeout_ms !== undefined && (typeof transport.timeout_ms !== 'number' || !Number.isFinite(transport.timeout_ms))) return t('tools.mcp.jsonTimeoutError');
  }
  return '';
}

