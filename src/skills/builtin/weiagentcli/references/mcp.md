# MCP Operations

Use this reference for MCP server lifecycle, transport configuration, tool enablement, authentication, refresh, and per-Agent overrides.

## Inspect and test

```bash
weiagent --format json mcp list
weiagent --format json mcp show
weiagent --format json mcp show SERVER_ID
weiagent --format json mcp tools SERVER_ID
weiagent --format json mcp tool show SERVER_ID TOOL_NAME
weiagent mcp test SERVER_ID
weiagent mcp reload SERVER_ID
weiagent mcp reload
```

Use the MCP commands in progressive-disclosure order:

1. `mcp show` lists configured servers and their current status.
2. `mcp show SERVER_ID` reads one server without expanding its tool inventory.
3. `mcp tools SERVER_ID` lists the tools exposed by that server.
4. `mcp tool show SERVER_ID TOOL_NAME` reads one tool's description and input schema.

`mcp list` remains a compatibility alias for the first step. `test` and `reload` reconnect and refresh the server's tool inventory.

## Create or edit a server

MCP create and edit use a complete JSON document:

```json
{
  "name": "SERVER_NAME",
  "description": "DESCRIPTION",
  "enabled": true,
  "supports_parallel_tool_calls": true,
  "provider": "PROVIDER_NAME",
  "provider_url": "https://HOST",
  "logo_url": null,
  "tags": ["TAG"],
  "transport": {
    "type": "stdio",
    "command": "MCP_COMMAND",
    "args": ["ARG"],
    "cwd": "PATH",
    "env": {"TOKEN_NAME": "TOKEN"},
    "inherit_env": false,
    "env_allowlist": [],
    "long_running": true,
    "timeout_ms": 30000
  }
}
```

```bash
weiagent mcp add --file MCP.json
weiagent mcp edit SERVER_ID --file MCP.json
```

The transport type is one of `stdio`, `streamable_http`, or `sse`.

Streamable HTTP example:

```json
{
  "name": "SERVER_NAME",
  "enabled": true,
  "transport": {
    "type": "streamable_http",
    "url": "https://HOST/mcp",
    "bearer_token": "TOKEN",
    "headers": {"X-Custom-Header": "VALUE"},
    "timeout_ms": 30000
  }
}
```

SSE uses the same HTTP fields with `"type": "sse"`. OAuth-capable servers may also include `oauth` with `authorize_url`, `token_url`, `client_id`, optional `client_secret`, `scopes`, `redirect_port`, and `redirect_uri`. Client certificates use `client_cert`, `client_key`, and an optional `identity_header`.

Secrets are redacted by `show`. Keep them in a protected file or environment-backed generation step rather than shell arguments.

## Lifecycle

```bash
weiagent mcp enable SERVER_ID
weiagent mcp disable SERVER_ID
weiagent mcp remove SERVER_ID
weiagent mcp test SERVER_ID
weiagent mcp reload SERVER_ID
```

Changing a server configuration triggers a background refresh when it remains enabled. Use `test` or `reload SERVER_ID` when the next operation needs confirmed tool discovery.

## Tool enablement

Global tool enablement is represented in the server document returned by the server operations. Use the exact tool name from `mcp tools SERVER_ID` when configuring an Agent. Per-Agent access is separate:

```bash
weiagent agent mcp show AGENT_ID
weiagent agent mcp enable AGENT_ID SERVER_ID
weiagent agent mcp disable AGENT_ID SERVER_ID
weiagent agent mcp tool-enable AGENT_ID SERVER_ID TOOL_NAME
weiagent agent mcp tool-disable AGENT_ID SERVER_ID TOOL_NAME
```

The Agent-level command preserves unrelated server and tool overrides. For a batch change, use the complete `mcp` override document from `references/agents.md`.

## Auth troubleshooting

Inspect `status`, `last_error`, and `diagnostics` in `mcp show SERVER_ID`. OAuth state is available from the service API even though the compact CLI surface does not expose separate auth subcommands. For a failed connection, verify the transport JSON, run `mcp test`, then inspect the refreshed server response before changing Agent access.
