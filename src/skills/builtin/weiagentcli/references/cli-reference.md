# CLI Reference and Troubleshooting

## Startup modes

```bash
weiagent
weiagent serve
weiagent dev
```

The bare command and `serve` start the production service. `dev` starts the development service and frontend. Server options are available on `serve` and `dev`:

```text
--host HOST
--port PORT
--foreground
```

Client commands automatically connect to the default `http://127.0.0.1:18283` or start a local service. Override connection settings with global options:

```text
--server URL
--token TOKEN
--no-start
--data-dir PATH
--timeout SECONDS
```

The token can also come from `WEIAGENT_API_TOKEN`; the server URL can come from `WEIAGENT_SERVER_URL`.

## Output options

```text
--format human|json
--quiet
--verbose
--no-color
```

Use `--format json` whenever a result will be parsed or passed to another command. `--quiet` suppresses nonessential client messages. Errors go to standard error and the process exits nonzero.

## File-based writes

CRUD commands accept `--file PATH` where the command declares it. The file must contain valid JSON. Prefer a complete document for nested settings and preserve fields not being changed.

Typical sequence:

```bash
weiagent --format json RESOURCE show ID
# create or update REQUEST.json
weiagent --format json RESOURCE edit ID --file REQUEST.json
weiagent --format json RESOURCE show ID
```

Use a temporary file for generated JSON. Do not put long JSON or credentials directly in command-line arguments.

## Structured completion contract

Mutations that return an empty HTTP response are represented as `{"ok":true}`. Streamed Agent runs use one JSON object per line, for example:

```json
{"event":"delta","text":"TEXT"}
{"event":"completed","run_id":"RUN_ID"}
```

Treat an explicit `error` event or nonzero process exit as failure.

## Common recovery loop

1. Re-run the narrowest `show` or `test` command with `--format json`.
2. Read `status`, `last_error`, diagnostics, and IDs from the result.
3. Correct only the invalid field in the source JSON.
4. Apply the edit or reload.
5. Verify again.

Use `--verbose` for client connection diagnostics. Use `--no-start` to distinguish a service startup problem from a command or API problem.

## Important path rules

- Skill content paths are relative to the Skill directory, such as `references/agents.md`.
- JSON file paths are resolved by the local shell and may be absolute or relative to the current working directory.
- `--data-dir PATH` selects the service data directory for server startup and client token discovery.
