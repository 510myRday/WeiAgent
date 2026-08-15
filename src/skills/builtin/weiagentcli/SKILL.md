---
name: weiagentcli
description: Operate the current WeiAgent intelligent agent service through its built-in CLI and local service API. Use this Skill when the task asks to inspect, create, edit, run, enable, disable, test, reload, install, or remove WeiAgent Agents, Providers, Models, Skills, MCP servers, proxies, conversations, or runs.
---

# WeiAgent CLI Operating Guide

Treat `weiagent` as the operating interface for the current WeiAgent service. Execute the matching CLI command instead of only explaining a command. Use JSON output for discovery, parsing, and chained operations; summarize the resulting state after the command completes.

## Operating rules

- If the user does not name another target, operate on the current `weiagent` service.
- Treat `list` as discovery, `show` as detail inspection, and `test` as connectivity or health verification.
- For writes, use `create`/`add`, `edit`, `delete`/`remove`, `enable`, `disable`, `reload`, `install`, or `run` as appropriate.
- Resolve missing IDs with the relevant `list` command. Continue automatically when one target matches; ask only when multiple targets remain ambiguous.
- For Agent behavior or permissions, start with `weiagent agent show AGENT_ID --full`, preserve unrelated settings, edit only the requested section, and verify with the matching `show` command.
- Client commands connect to or start the local service automatically. Use `--no-start` only when the caller explicitly requires an already-running service.
- Commands launched by WeiAgent's built-in terminal inherit the service URL, data directory, and temporary API token. Run ordinary client commands directly; do not start a second service first.
- The built-in Skill name is exactly `weiagentcli`.

## Reference routing

Load only the reference needed for the current task:

- Agent creation, profile, tools, permissions, MCP access, Skill access, or Agent runs: `references/agents.md`
- Provider and model configuration or testing: `references/providers-models.md`
- Skill discovery, content loading, installation, reload, or Skill groups: `references/skills.md`
- MCP server creation, transport configuration, tools, auth, or refresh: `references/mcp.md`
- Proxy creation, testing, enablement, or provider assignment: `references/proxies.md`
- Startup, global options, JSON output, files, and troubleshooting: `references/cli-reference.md`

Use `weiagent skill content weiagentcli --path references/FILE.md` when the reference must be loaded through the service itself.

## Standard execution loop

1. Identify the resource domain and load its reference file.
2. Discover the target with `--format json ... list` when an ID or exact name is missing.
3. Inspect the target with `show` before changing nested configuration.
4. For file-based writes, construct a complete JSON document in a temporary file and pass `--file PATH`.
5. Execute the mutation or run command.
6. Re-read the affected resource and report the actual result, including any returned status or error.

## Fast command index

```text
weiagent agent list | show AGENT_ID [--full] | create | edit | delete | run
weiagent provider list | show PROVIDER_ID | create | edit | delete | test
weiagent model list PROVIDER_ID | test PROVIDER_ID MODEL_ID
weiagent skill list | show SKILL_NAME | content SKILL_NAME [--path PATH]
weiagent skill run SKILL_NAME | reload | remove | install SOURCE
weiagent mcp list | show SERVER_ID | add --file FILE | edit SERVER_ID --file FILE
weiagent mcp remove | enable | disable | test | reload [SERVER_ID]
weiagent proxy list | show PROXY_ID | add | edit | remove | test
```

All resource IDs are UUIDs unless a command explicitly takes a name or group ID. See the domain references for complete syntax and request shapes.
