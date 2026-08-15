# Skill Operations

Use this reference for Skill discovery, loading, installation, reloads, removal, groups, and Agent Skill access.

## Discovery and selective loading

```bash
weiagent --format json skill list
weiagent --format json skill show SKILL_NAME
weiagent --format json skill content SKILL_NAME
weiagent --format json skill content SKILL_NAME --path references/FILE.md
```

`skill list` returns names, versions, groups, enabled state, and built-in state. `skill show` returns the manifest and file inventories. `skill content` reads `SKILL.md` by default; pass a relative path to load a reference or another bundled file.

For AI use, prefer this progressive-disclosure sequence:

1. `skill list` to find a Skill.
2. `skill show SKILL_NAME` to inspect its references.
3. `skill content SKILL_NAME --path references/RELEVANT_FILE.md` to load only the needed detail.

## Install and reload

```bash
weiagent skill install SOURCE
weiagent skill install SOURCE --group-id GROUP_ID
weiagent skill install SOURCE --proxy-id PROXY_ID
weiagent skill reload
```

`SOURCE` can be a local Skill directory, archive, or supported URL. A valid Skill directory contains a regular `SKILL.md` with `name` and `description` frontmatter. Network imports can use a configured proxy.

Installation is asynchronous. The CLI waits for the import task and prints the final result. If the result reports an error, inspect the source layout and retry with the narrowest supported source path.

Reload rescans the Skill root and is useful after external files were changed. The built-in `weiagentcli` Skill is materialized by the service at startup and belongs to the protected `builtin` group.

## Remove and enablement

```bash
weiagent skill remove SKILL_NAME
```

Global enablement is managed by the service API and UI. Per-Agent enablement uses the commands in `references/agents.md`:

```bash
weiagent agent skill show AGENT_ID
weiagent agent skill enable AGENT_ID SKILL_NAME
weiagent agent skill disable AGENT_ID SKILL_NAME
weiagent agent skill group-enable AGENT_ID GROUP_ID
weiagent agent skill group-disable AGENT_ID GROUP_ID
```

Do not delete or replace the built-in `weiagentcli` Skill. It is restored from the executable on service startup.

## Skill groups

Group IDs are returned by `skill list` in the `groups` section. Use the exact group ID when changing Agent-level group access. Preserve individual Skill overrides when editing the full `skills` document.
