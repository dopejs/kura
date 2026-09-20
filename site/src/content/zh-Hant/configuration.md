# 設定

守護程序從三層載入設定，優先順序依次升高：

1. 內建的、區分環境的預設值
2. `<data_dir>/config.json`
3. `KURA_*` 環境變數

test 環境（`KURA_ENV=test`）使用 `~/.kura-test` 和 `127.0.0.1:19192`；
production 使用 `~/.kura` 和 `127.0.0.1:19191`。

## config.json 結構

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

`store.readers` 在單一寫連線旁開啟只讀的讀連線，讓讀操作在 WAL 下與寫操作
併發；`0`（預設）即單連線行為。每種角色的鎖等待時間會匯出到 `/metrics`。

## LLM 提供方

- **echo**——確定性的程序內兜底；當 `llm.defaultProvider` 為 `echo` 時列出，
  因此守護程序零設定即可工作。
- **claude_code_cli / codex_cli**——託管 CLI 提供方：守護程序驅動本地安裝
  的 CLI（`cliPath`、`workDir`）。
- **openai_compatible**——任意 OpenAI 相容的 HTTP 端點（`baseURL`、
  `apiKey`/`apiKeyEnv`、`model`、流式超時）。只有設定了 `baseURL` 才會註冊；
  支援工具呼叫。
- **accounts**——使用者登入的訂閱賬號（`llm.accounts`，或以 JSON 形式透過
  `KURA_LLM_ACCOUNTS` 傳入），每個賬號一條並註明協議（`openai_compatible`、
  `openai_responses`、`anthropic_messages`）；憑據可在守護程序執行中替換。

`llm.defaultProvider` 選擇預設提供方；chat API 接受按請求覆蓋。

## 金鑰

優先使用 `*Env` 欄位（如 `botTokenEnv`），讓憑據來自環境變數而不是寫進設定
檔案。租戶級金鑰放在守護程序的金鑰平面（資料目錄下的
`tenant-secret-values/`），在支援的地方用 `secretRef` 引用（如
`slack.botTokenSecretRef`）。

## 外掛 profile

執行時的組合在 `<data_dir>/plugins.json` 裡單獨設定——哪些外掛啟用，以及
各外掛的設定。見**外掛**。工具與部署相關工作新增的條目：

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

工具提供方 profile（網頁搜尋、圖片與影片生成）不是設定檔案條目：它們透過
`POST /v1/tools/profiles` 建立，只儲存 `secretRef`（從不儲存憑據值），而
`mcp_backed` family 指向你自己接入的 MCP 伺服器。

## 常用環境變數

| 變數 | 作用 |
|------|------|
| `KURA_ENV` | 選擇 `test` / `prod` 環境 |
| `KURA_CONNECTORS_DISCORD_ENABLED` | 啟用實時 Discord 聯結器 |
| `KURA_STORE_READERS` | 寫連線旁的讀連線數（見 `store.readers`） |
| `KURA_PROJECT_ROOT` | 守護程序服務的專案；限定 `project_tools` 沙箱 profile 的範圍 |
| `KURA_*` | 每個設定欄位都有對應的環境變數覆蓋 |
