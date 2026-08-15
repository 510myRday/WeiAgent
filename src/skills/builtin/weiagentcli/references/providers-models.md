# Providers and Models

Use this reference for LLM provider lifecycle, API key pools, model registration, model tests, and provider-level proxy configuration.

## Inspect

```bash
weiagent --format json provider list
weiagent --format json provider show PROVIDER_ID
weiagent --format json model list PROVIDER_ID
```

`provider list` is intentionally summarized. Use `show` for base URL, provider type, masked key state, model list, model configs, cache settings, and proxy assignment. API keys are never returned in plaintext by normal reads.

## Provider types

The `provider_type` values are:

- `openai_compatible`
- `openai_responses`
- `anthropic_messages`
- `google_gemini`
- `openai_images`

## Create and edit with flags

```bash
weiagent provider create --name "NAME" --base-url "https://HOST" \
  --provider-type openai_compatible --models MODEL_A,MODEL_B \
  --api-key-env ENV_VAR
weiagent provider edit PROVIDER_ID --models MODEL_A,MODEL_B
weiagent provider test PROVIDER_ID
```

For secrets or advanced fields, use a JSON file:

```json
{
  "name": "NAME",
  "provider_type": "openai_compatible",
  "base_url": "https://HOST",
  "api_keys": ["TOKEN"],
  "models": ["MODEL_ID"],
  "model_configs": [
    {
      "model_id": "MODEL_ID",
      "multimodal": false,
      "thinking": true,
      "context_size": 128000,
      "input_price": null,
      "output_price": null
    }
  ],
  "key_strategy": "failover",
  "extra_body": {},
  "proxy_url": null
}
```

```bash
weiagent provider create --file PROVIDER.json
weiagent provider edit PROVIDER_ID --file PROVIDER.json
```

Supported key strategies are `failover`, `round_robin`, and `random`. Prefer `--api-key-env ENV_VAR` or a file over placing secrets directly in shell history.

When editing API keys, preserve masked entries returned by `show` if they refer to existing keys. Plaintext entries add or replace keys; stale masked entries are discarded by the service.

## Model operations

```bash
weiagent model list PROVIDER_ID
weiagent model test PROVIDER_ID MODEL_ID
weiagent model test PROVIDER_ID MODEL_ID --prompt "PROMPT"
```

The model ID is a provider-local string. A model must be present in the provider's `models` list before assigning it to an Agent. After changing a provider, run `provider test` and then `model test` for the exact model an Agent will use.

## Assign a provider and model to an Agent

1. Run `provider list` and choose `PROVIDER_ID`.
2. Run `model list PROVIDER_ID` and choose `MODEL_ID`.
3. Run `agent edit AGENT_ID --provider-id PROVIDER_ID --model MODEL_ID`.
4. Verify with `agent show AGENT_ID`.

Provider configuration is global. The Agent's provider/model selection is per Agent.

