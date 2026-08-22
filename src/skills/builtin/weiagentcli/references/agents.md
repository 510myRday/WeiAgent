# Agent Operations

Use this reference for Agent lifecycle, behavior, tool settings, MCP access, Skill access, and Agent runs.

## Discovery and inspection

```bash
weiagent --format json agent list
weiagent --format json agent show AGENT_ID
```

The normal `show` response contains compact Agent metadata. Read only the section needed for the current task:

```bash
weiagent --format json agent profile show AGENT_ID
weiagent --format json agent tools show AGENT_ID
weiagent --format json agent mcp show AGENT_ID
weiagent --format json agent skill show AGENT_ID
```

Use the aggregate form only for a complete snapshot or final cross-section verification:

```bash
weiagent --format json agent show AGENT_ID --full
```

The aggregate response includes `agent`, `workspace`, `tools`, `mcp`, and `skills` sections and is intentionally larger than the section-specific reads.

Resolve an ID from the list by exact name when possible. Do not guess UUIDs.

## Create and edit an Agent

Small metadata changes can use flags:

```bash
weiagent agent create --name "NAME" --description "DESCRIPTION" \
  --provider-id PROVIDER_ID --model MODEL_ID
weiagent agent edit AGENT_ID --name "NEW_NAME" --model MODEL_ID
# Add --full to agent edit when the complete post-edit document is needed.
weiagent agent edit AGENT_ID --model MODEL_ID --full
```

For a complete document, use `--file`:

```json
{
  "agent": {
    "name": "NAME",
    "description": "DESCRIPTION",
    "model_provider_id": "PROVIDER_ID",
    "model_id": "MODEL_ID",
    "avatar_icon": "bot",
    "avatar_color": "blue"
  },
  "workspace": {
    "personality": "PERSONALITY",
    "memory": "MEMORY",
    "instructions": "INSTRUCTIONS"
  },
  "tools": {},
  "mcp": {"servers": [], "tools": []},
  "skills": {"skills": [], "groups": []}
}
```

```bash
weiagent agent create --file AGENT.json
weiagent agent create --file AGENT.json --full
weiagent agent edit AGENT_ID --file AGENT.json
```

Creation returns the new Agent ID and changed sections by default. Add `--full` when the complete post-creation document is needed. Delete an Agent with:

```bash
weiagent agent delete AGENT_ID
```

Separate files are also supported: `--profile-file`, `--tools-file`, `--mcp-file`, and `--skills-file`. The complete document is merged by the CLI into the appropriate API requests.

## Profile and behavior

```bash
weiagent agent profile show AGENT_ID
weiagent agent profile edit AGENT_ID --file PROFILE.json
```

The profile file may contain either the workspace object directly or a wrapper:

```json
{
  "personality": "PERSONALITY",
  "memory": "MEMORY",
  "instructions": "INSTRUCTIONS"
}
```

When changing one field, first read the current profile and preserve the other two fields.

## Built-in tool settings

```bash
weiagent agent tools show AGENT_ID
weiagent agent tools edit AGENT_ID --file TOOLS.json
```

The override shape supports `max_llm_iterations`, `terminal`, `web_search`, and `task_planning`:

```json
{
  "version": 1,
  "max_llm_iterations": 12,
  "terminal": {
    "enabled": true,
    "permission_mode": "request_approval",
    "default_timeout_ms": 30000,
    "allowed_cwd_roots": ["PATH"]
  },
  "web_search": {
    "enabled": true,
    "max_results": 8,
    "source_overrides": []
  },
  "task_planning": {
    "enabled": true,
    "progress_interval": 3
  }
}
```

Read the existing settings before editing. Submit the complete override document so unrelated grants and source overrides remain intact.

## MCP access for one Agent

```bash
weiagent agent mcp show AGENT_ID
weiagent agent mcp enable AGENT_ID SERVER_ID
weiagent agent mcp disable AGENT_ID SERVER_ID
weiagent agent mcp tool-enable AGENT_ID SERVER_ID TOOL_NAME
weiagent agent mcp tool-disable AGENT_ID SERVER_ID TOOL_NAME
weiagent agent mcp edit AGENT_ID --file MCP_OVERRIDES.json
```

The file shape is:

```json
{
  "servers": [
    {"server_id": "SERVER_ID", "enabled": true, "customized": true}
  ],
  "tools": [
    {"server_id": "SERVER_ID", "tool_name": "TOOL_NAME", "enabled": true}
  ]
}
```

The convenience commands read the current overrides and preserve unrelated entries. Use `edit` for a multi-item change, keeping every existing item that is not part of the request.

## Skill access for one Agent

```bash
weiagent agent skill show AGENT_ID
weiagent agent skill enable AGENT_ID SKILL_NAME
weiagent agent skill disable AGENT_ID SKILL_NAME
weiagent agent skill group-enable AGENT_ID GROUP_ID
weiagent agent skill group-disable AGENT_ID GROUP_ID
weiagent agent skill edit AGENT_ID --file SKILLS_OVERRIDES.json
```

```json
{
  "skills": [
    {"name": "SKILL_NAME", "enabled": true}
  ],
  "groups": [
    {"group": "GROUP_ID", "group_id": "GROUP_ID", "enabled": true}
  ]
}
```

## Run an Agent

```bash
weiagent agent run AGENT_ID "PROMPT"
weiagent agent run AGENT_ID --stream "PROMPT"
weiagent agent run AGENT_ID --conversation CONVERSATION_ID "PROMPT"
weiagent agent run AGENT_ID --thinking-level off "PROMPT"
weiagent agent run AGENT_ID --thinking-level low "PROMPT"
weiagent agent run AGENT_ID --thinking-level medium "PROMPT"
weiagent agent run AGENT_ID --thinking-level high "PROMPT"
weiagent agent run AGENT_ID --thinking-level x-high "PROMPT"
```

If the prompt argument is omitted, read it from standard input. `--stream` emits incremental output. With `--format json`, streamed output is one JSON object per line, such as `delta`, `completed`, or `error` events.

## Verification

After a mutation, run the narrowest matching read command:

```bash
weiagent --format json agent show AGENT_ID
weiagent --format json agent profile show AGENT_ID
weiagent --format json agent mcp show AGENT_ID
weiagent --format json agent skill show AGENT_ID
weiagent --format json agent tools show AGENT_ID
```

Use `agent show AGENT_ID --full` only when several sections changed or the complete aggregate state must be checked.
