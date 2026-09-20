# 使用

## 透過 HTTP 對話

一次性查詢：

```bash
curl -s http://127.0.0.1:19192/v1/chat/query \
  -H 'content-type: application/json' \
  -d '{
    "query": "summarize my day",
    "provider": "claude_code_cli",
    "threadId": "thr_personal"
  }'
```

流式（SSE）：

```bash
curl -N http://127.0.0.1:19192/v1/chat/query/stream \
  -H 'content-type: application/json' \
  -d '{"query": "hello", "provider": "echo"}'
```

每一輪都走完整流水線：技能與人設編譯進提示詞，執行緒連續性注入先前的對話，
**context 外掛**注入記憶引導與按需召回，**session-strategy 外掛**在預算內
整形視窗——而模型看到的內容，和 dispatch 記錄裡持久化的內容完全一致
（*模型可見 = 已記錄*）。

## 工具

一輪對話可以呼叫工具。模型能看到什麼，按輪次、按租戶解析：

- **MCP 工具**——你接入的伺服器釋出的所有工具，在 `chat` surface 的暴露
  規則下授權；標記為 `approval_required` 的工具會等待人來回答，而不是直接
  失敗。
- **`memory.lookup`**——在你自己的 Ready 記憶上召回，和
  `POST /v1/retrieval/queries` 用同一套融合排序；命中項以
  `Memory[<layer> <id>]` 形式引用。
- **`web.search` / `image.generate` / `video.generate`**——每個已設定的工具
  profile 對應一個工具。Kura 不替你選供應商：`mcp_backed` family 轉發到你
  自己選定的 MCP 伺服器（模型看到的是該工具真實的引數 schema），每次呼叫
  都經過配額與出站策略門。`builtin_stub` 用於在沒有供應商賬號時驗證整條
  路徑。

```bash
curl -s http://127.0.0.1:19192/v1/tools/profiles -H 'content-type: application/json' -d '{
  "title": "my search", "capability": "web.search", "family": "mcp_backed",
  "authMode": "none", "mcpServerId": "mcp_search", "mcpToolName": "search", "isDefault": true
}'
```

每次呼叫都是一條 `chat.tool.called` 事件和一個 `kura_chat_tool_calls_total`
指標；`chat/tool-call` hook 可以改寫引數或否決呼叫。

## 瀏覽器

當 `computer-use` 外掛設定為 `driver: "subprocess"` 時，computer-use 工作階段
會驅動真實瀏覽器：一個受監管的 Node worker（`capabilities/browser/worker.mjs`，
Playwright + Chromium）透過按行 JSON 協議和守護程序通訊；掛起和崩潰會上報
給能力監管器，worker 會被重新拉起。

## TypeScript SDK

```ts
import { createKuraClient } from "@kura/client";

const client = createKuraClient({ baseURL: "http://127.0.0.1:19192" });

const result = await client.queryChat({ query: "hello", provider: "echo" });

// 流式
for await (const chunk of client.streamChatQuery({ query: "hi" })) {
  process.stdout.write(chunk.delta);
}

// 記憶、外掛、檢索……
const plugins = await client.listPlugins();
const recall  = await client.queryRetrieval({ query: "package manager" });
const assets  = await client.listMemoryAssets({ layer: "l1", status: "ready" });
```

## 終端介面

`kura tui` 啟動全屏終端客戶端：對話、執行緒連續性，以及實時的守護程序事件
流（`/events`）。

## Web 操作檯

`kura web` 在本地啟動已安裝的 Web 操作檯並開啟瀏覽器。它是運維控制檯：
記憶資產 + 稽核佇列、**外掛**（裝配報告、啟用/禁用、hook）、頻道、例程、
提供方、配額、診斷、評估以及支援類介面。

## 執行緒與連續性

傳入 `threadId` 以保持連續性：先前的輪次以受治理的連續性記錄儲存，並在每次
dispatch 時重新渲染進上下文（有界、脫敏安全）。IM 頻道自動獲得**每執行緒一個
上下文**。

## 事件

所有可觀測的東西都是事件（`llm.*`、`memory.*`、`context.*`、`chat.*`、
`improvement.*`、`system.*`），持久化在儲存賬本中併發布到實時匯流排。TUI 的
`/events` 檢視和 sessions API 都能檢視。
