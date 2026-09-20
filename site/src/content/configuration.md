# Configuration

The daemon loads configuration from three layers, in increasing
precedence:

1. Built-in environment-aware defaults
2. `<data_dir>/config.json`
3. `KURA_*` environment variables

Test env (`KURA_ENV=test`) targets `~/.kura-test` and `127.0.0.1:19192`;
production targets `~/.kura` and `127.0.0.1:19191`.

## config.json shape

```json
{
  "logLevel": "info",
  "llm": {
    "defaultProvider": "claude_code_cli",
    "defaultModel": "",
    "defaultTimeoutMs": 30000,
    "openaiCompatible": {
      "baseURL": "https://api.example.com/v1",
      "apiKeyEnv": "MY_API_KEY",
      "model": "some-model"
    },
    "claude": { "cliPath": "", "defaultModel": "", "workDir": "~" },
    "codex":  { "cliPath": "", "defaultModel": "", "workDir": "~" }
  },
  "connectors": {
    "discord":  { "enabled": false, "botTokenEnv": "DISCORD_BOT_TOKEN" },
    "telegram": { "enabled": false, "botTokenEnv": "TELEGRAM_BOT_TOKEN" },
    "slack":    { "enabled": false },
    "matrix":   { "enabled": false }
  },
  "store": { "readers": 4 }
}
```

`store.readers` opens query-only reader connections beside the single
writer so reads run concurrently with writes under WAL; `0` (the default)
is the single-connection behaviour. Lock wait per role is exported at
`/metrics`.

## LLM providers

- **echo** — deterministic in-process fallback, listed when
  `llm.defaultProvider` is `echo`, so the daemon works with zero
  configuration.
- **claude_code_cli / codex_cli** — managed CLI providers: the daemon
  drives the locally installed CLI (`cliPath`, `workDir`).
- **openai_compatible** — any OpenAI-compatible HTTP endpoint
  (`baseURL`, `apiKey`/`apiKeyEnv`, `model`, stream timeouts). Registered
  only when `baseURL` is set; supports tool calling.
- **accounts** — subscriptions the user signed into (`llm.accounts`, or
  `KURA_LLM_ACCOUNTS` as JSON), one entry per account with its protocol
  (`openai_compatible`, `openai_responses`, `anthropic_messages`); the
  credential can be replaced while the daemon runs.

`llm.defaultProvider` picks the default; per-request overrides are
accepted on the chat APIs.

## Secrets

Prefer the `*Env` fields (e.g. `botTokenEnv`) so credentials come from the
environment instead of being written into config files. Tenant-scoped
secrets live behind the daemon's secrets plane (`tenant-secret-values/`
under the data dir) and are referenced by `secretRef` where supported
(e.g. `slack.botTokenSecretRef`).

## The plugin profile

Runtime composition is configured separately in `<data_dir>/plugins.json`
— which plugins are enabled and their per-plugin config. See **Plugins**.
Entries added by the tool and deployment work:

```json
{
  "entries": {
    "chat": { "config": { "toolMaxRounds": 16 } },
    "computer-use": { "config": {
      "driver": "subprocess",
      "command": "node",
      "args": ["/path/to/kura/capabilities/browser/worker.mjs"],
      "env": { "KURA_PLAYWRIGHT_PATH": "/path/to/node_modules/playwright" },
      "timeoutMs": 30000
    } },
    "swarm": { "config": { "enabled": true, "maxConcurrentChildren": 2 } }
  }
}
```

Tool provider profiles (web search, image and video generation) are not
config-file entries: they are created through `POST /v1/tools/profiles`,
hold only a `secretRef` (never a credential value), and the `mcp_backed`
family points at an MCP server you connected yourself.

## Useful environment variables

| Variable | Effect |
|----------|--------|
| `KURA_ENV` | `test` / `prod` environment selection |
| `KURA_CONNECTORS_DISCORD_ENABLED` | opt into the live Discord connector |
| `KURA_STORE_READERS` | reader connections beside the writer (see `store.readers`) |
| `KURA_PROJECT_ROOT` | the project a daemon serves; scopes the `project_tools` sandbox profile |
| `KURA_*` | every config field has an env override |
