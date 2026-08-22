# Heartbeat and Cron Scheduling

Use the existing `execute_terminal` tool to run `weiagent` CLI commands. There is no dedicated schedule function call.

## Context and binding

The WeiAgent Service Context includes:

```text
Current agent ID: AGENT_ID
Current conversation ID: CONVERSATION_ID
```

The service context IDs are context, not user confirmation. Always enumerate the available Agents first:

```text
weiagent --format json agent list
```

Show the user every available Agent name, together with its ID when needed to disambiguate duplicate names. Wait for the user to select one exact Agent. Never select the current Agent, the first Agent, the built-in Agent, or any other Agent as a default.

Heartbeat belongs to one existing conversation and uses that conversation's current Agent configuration. After the user selects an Agent, verify that `CONVERSATION_ID` belongs to that Agent before using it. If it belongs to another Agent, ask the user to identify an existing conversation for the selected Agent or switch to that Agent's conversation, then re-read the service context. Do not silently move the Heartbeat to another Agent or create a replacement conversation for it.

Cron always binds to the exact `agent_id` selected by the user. A Cron task does not accept task-level model, provider, tool, MCP, or Skill overrides. Each execution creates a new conversation and loads the bound Agent's current configuration at execution time.

Cron output is stored in the execution conversation, not in the conversation that created the task. Use `cron executions CRON_ID` to find execution conversation IDs.

## Standard workflow

### Confirmation gate

Before any create or set command, complete these steps:

1. Determine whether the request means a **Heartbeat** or a recurring **Cron** task. If the user has not clearly specified the type, ask and wait.
2. Run `weiagent --format json agent list` once and present all existing Agent names to the user.
3. Ask the user to select one exact Agent. The answer must identify the Agent by name or ID. Do not infer it from the current conversation or the wording of the prompt.
4. Collect the remaining fields in the same clarification when possible:
   - Heartbeat: selected Agent, target existing conversation, interval, and prompt.
   - Cron: selected Agent, five-field schedule, prompt, and optional source conversation/timezone.
5. Repeat the selected type and Agent back to the user and wait for explicit confirmation when the request was initially ambiguous or omitted either field.
6. Only after the type and Agent are confirmed, run the single mutation command through `execute_terminal` with `--format json`.
7. Verify with one narrow `show` command. Report the returned ID, selected Agent, enabled state, and next run time.

Recommended clarification format:

```text
当前已有智能体：
1. AGENT_NAME_1 (AGENT_ID_1)
2. AGENT_NAME_2 (AGENT_ID_2)

请确认两项：
1. 创建 Heartbeat 还是 Cron？
2. 使用哪个智能体？请回复名称或 ID。

然后请提供执行提示词；Cron 还需要五段表达式，Heartbeat 还需要间隔。
```

This is one combined clarification round. Do not run `agent show` for every Agent; the list result already supplies the names and IDs needed for selection. Load the selected Agent's detail only when the user explicitly asks about its configuration.

Do not claim a task has completed merely because the `run` request was accepted. It queues the next scheduler tick; inspect the task and its executions afterward.

## Cron commands

```text
weiagent --format json cron list
weiagent --format json cron show CRON_ID
weiagent --format json cron create --agent AGENT_ID --schedule "0 9 * * *" "PROMPT"
weiagent --format json cron create --agent AGENT_ID --schedule "*/15 * * * *" --conversation CONVERSATION_ID "PROMPT"
weiagent --format json cron edit CRON_ID --schedule "0 9 * * *" "PROMPT"
weiagent --format json cron pause CRON_ID
weiagent --format json cron resume CRON_ID
weiagent --format json cron run CRON_ID
weiagent --format json cron executions CRON_ID
weiagent --format json cron remove CRON_ID
```

The schedule uses exactly five standard fields: minute, hour, day-of-month, month, and day-of-week. Examples:

```text
0 9 * * *
*/15 * * * *
0 0 * * 1
30 8 1 * *
```

The task stores the service's local UTC offset at creation time, such as `+08:00`. It does not use an IANA timezone database.

For long prompts, use `--prompt-file PATH` instead of putting the prompt in the command line:

```text
weiagent --format json cron create --agent AGENT_ID --schedule "0 9 * * *" --prompt-file PATH
```

## Heartbeat commands

```text
weiagent --format json heartbeat show --conversation CONVERSATION_ID
weiagent --format json heartbeat set --conversation CONVERSATION_ID --interval 10m "PROMPT"
weiagent --format json heartbeat pause --conversation CONVERSATION_ID
weiagent --format json heartbeat resume --conversation CONVERSATION_ID
weiagent --format json heartbeat run --conversation CONVERSATION_ID
weiagent --format json heartbeat remove --conversation CONVERSATION_ID
```

Heartbeat has at most one configuration per conversation. `set` replaces the existing interval and prompt. The minimum interval is enforced by the service. If the conversation is already running when a Heartbeat fires, it is marked pending and runs once after the conversation becomes idle.

When the user asks for a schedule without an explicit Agent, stop at the confirmation gate, list the existing Agents, and ask the user to select one. The current Agent is never a fallback. Do not create a new Agent and do not add tool, MCP, Skill, provider, or model configuration to the schedule.
