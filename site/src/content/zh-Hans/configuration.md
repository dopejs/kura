# 配置

守护进程从三层加载配置，优先级依次升高：

1. 内置的、区分环境的默认值
2. `<data_dir>/config.json`
3. `KURA_*` 环境变量

test 环境（`KURA_ENV=test`）使用 `~/.kura-test` 和 `127.0.0.1:19192`；
production 使用 `~/.kura` 和 `127.0.0.1:19191`。

## config.json 结构

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

`store.readers` 在单一写连接旁打开只读的读连接，让读操作在 WAL 下与写操作
并发；`0`（默认）即单连接行为。每种角色的锁等待时间会导出到 `/metrics`。

## LLM 提供方

- **echo**——确定性的进程内兜底；当 `llm.defaultProvider` 为 `echo` 时列出，
  因此守护进程零配置即可工作。
- **claude_code_cli / codex_cli**——托管 CLI 提供方：守护进程驱动本地安装
  的 CLI（`cliPath`、`workDir`）。
- **openai_compatible**——任意 OpenAI 兼容的 HTTP 端点（`baseURL`、
  `apiKey`/`apiKeyEnv`、`model`、流式超时）。只有设置了 `baseURL` 才会注册；
  支持工具调用。
- **accounts**——用户登录的订阅账号（`llm.accounts`，或以 JSON 形式通过
  `KURA_LLM_ACCOUNTS` 传入），每个账号一条并注明协议（`openai_compatible`、
  `openai_responses`、`anthropic_messages`）；凭据可在守护进程运行中替换。

`llm.defaultProvider` 选择默认提供方；chat API 接受按请求覆盖。

## 密钥

优先使用 `*Env` 字段（如 `botTokenEnv`），让凭据来自环境变量而不是写进配置
文件。租户级密钥放在守护进程的密钥平面（数据目录下的
`tenant-secret-values/`），在支持的地方用 `secretRef` 引用（如
`slack.botTokenSecretRef`）。

## 插件 profile

运行时的组合在 `<data_dir>/plugins.json` 里单独配置——哪些插件启用，以及
各插件的配置。见**插件**。工具与部署相关工作新增的条目：

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

工具提供方 profile（网页搜索、图片与视频生成）不是配置文件条目：它们通过
`POST /v1/tools/profiles` 创建，只保存 `secretRef`（从不保存凭据值），而
`mcp_backed` family 指向你自己接入的 MCP 服务器。

## 常用环境变量

| 变量 | 作用 |
|------|------|
| `KURA_ENV` | 选择 `test` / `prod` 环境 |
| `KURA_CONNECTORS_DISCORD_ENABLED` | 启用实时 Discord 连接器 |
| `KURA_STORE_READERS` | 写连接旁的读连接数（见 `store.readers`） |
| `KURA_PROJECT_ROOT` | 守护进程服务的项目；限定 `project_tools` 沙箱 profile 的范围 |
| `KURA_*` | 每个配置字段都有对应的环境变量覆盖 |
