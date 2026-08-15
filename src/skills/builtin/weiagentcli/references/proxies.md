# Proxy Operations

Use this reference for reusable HTTP, HTTPS, SOCKS4, and SOCKS5 proxy entries and their assignment to Providers or Skill downloads.

## Inspect and test

```bash
weiagent --format json proxy list
weiagent --format json proxy show PROXY_ID
weiagent proxy test PROXY_ID
```

The list identifies proxy IDs, names, protocol, enabled state, availability, latency, and quality probes. Use `show` for the complete redacted record.

## Create and edit

Flag form:

```bash
weiagent proxy add --name "NAME" --url "http://HOST:PORT"
weiagent proxy edit PROXY_ID --name "NEW_NAME" --url "socks5://HOST:PORT"
weiagent proxy edit PROXY_ID --enabled true
```

Complete file form:

```json
{
  "name": "NAME",
  "url": "http://HOST:PORT",
  "enabled": true
}
```

```bash
weiagent proxy add --file PROXY.json
weiagent proxy edit PROXY_ID --file PROXY.json
```

Supported protocols are `http`, `https`, `socks4`, and `socks5`. The service normalizes SOCKS URLs for remote DNS where applicable.

## Remove and use

```bash
weiagent proxy remove PROXY_ID
weiagent proxy test PROXY_ID
weiagent provider edit PROVIDER_ID --file PROVIDER.json
weiagent skill install SOURCE --proxy-id PROXY_ID
```

Provider assignment is stored in the provider's `proxy_url` field. Skill installation uses `--proxy-id` only for that download/import operation. Test a proxy before assigning it, and verify the provider or import result afterward.

