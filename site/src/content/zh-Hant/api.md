# API 參考

守護程序暴露單一的 HTTP API（test 環境預設 `127.0.0.1:19192`，prod 為
`:19191`）。一切都是 JSON；契約以 JSON Schema 形式放在倉庫的 `schemas/`
下，TypeScript SDK（`@kura/client`）與之保持一致。

## 認證

`/healthz`、`/version`、`/v1/system/info`、配對入口以及簽名認證的 webhook
入站是開放的；**其餘一切都在 bearer 權杖認證之後**（配對流程：
`POST /v1/auth/pairings/start`）。

## 路由族

| 族 | 要點 |
|----|------|
| Chat | `POST /v1/chat/query`、`POST /v1/chat/query/stream`（SSE）——一輪可能包含工具輪次 |
| Tools | `GET /v1/tools/capabilities`、`GET/POST /v1/tools/profiles`、`…/{id}/check`——搜尋 / 圖片 / 影片提供方，只儲存 `secretRef` |
| Swarm | `POST /v1/swarm/runs`——有界的併發子 agent（需顯式啟用） |
| LLM | `GET /v1/llm/dispatches`、`…/stream`——每條 dispatch 記錄，含訊息 |
| Memory | `GET/POST /v1/memory/assets`、下鑽、approve/reject/revoke、consolidate |
| Retrieval | `POST /v1/retrieval/queries`——帶引用的融合召回 |
| Plugins | `GET /v1/plugins`（裝配報告）、`GET/PUT /v1/plugins/profile` |
| Skills | `GET /v1/skills`、`POST /v1/skills/proposals`、`…/publish` |
| Improvement | `POST /v1/improvement/proposals`、`…/apply\|reject\|rollback` |
| Sessions | `GET /v1/sessions`、重置、事件歷史 |
| Threads | 生命週期（reset/archive/reopen）、交接、連續性預覽 |
| Connectors | 登錄檔、入站/訊息流水線、投遞結果 |
| Providers | 提供方登錄檔、託管認證、模型、健康檢查 |
| Sandbox | `POST /v1/sandboxes/executions`、profile、explain |
| MCP | 伺服器登錄檔、附屬執行、webhook 入站 |
| Catalog | 條目/版本/信任層級、啟用、回滾 |
| Routines / Reminders / Webhooks / Calendar / Mail / Triage | 自動化介面 |
| Policy | 審批佇列、消費者策略同步 |
| Billing / Quota | 套餐、用量賬本、配額、拒絕記錄 |
| Evaluation | 活動、fixture、回放、實時驗證 |
| Identity | 租戶、成員、金鑰、配對/權杖 |
| Release | `POST /v1/release/launch-gate`——以證據把關的釋出決策 |
| Observability | `/healthz`、`/version`、`/v1/system/info`、事件流、`GET /metrics`（Prometheus 文字，需 bearer 認證） |
| Config | `GET/PUT /v1/config/file`——帶比較交換、試執行和內聯金鑰拒絕的設定檔案 |
| Memory ops | `GET /v1/memory/overview`、`POST /v1/memory/indexes/rebuild`、`GET /v1/context/assemblies` |

## 約定

- 線上欄位一律 **camelCase**；列舉使用穩定的 snake-case 線上值。
- 錯誤為 `{ "error": message }`，並帶有意義的狀態碼
  （400/401/403/404/409/422/5xx）。策略否決為 403。
- 變更都會發出事件；事件賬本就是審計軌跡。
- 啟動時輸入（外掛 profile）回應 `restartRequired: true`，而不是假裝熱應用。
- 每個響應都帶 `x-request-id`（客戶端提供時原樣回顯）；守護程序為每個請求
  寫一行 `http.request` 訪問日誌，包含該 id、租戶、路由模板、狀態和時延。

## SDK

```bash
pnpm add @kura/client
```

`createKuraClient({ baseURL, token })` → 每個路由族的型別化方法：
`queryChat`、`streamChatQuery`、`listMemoryAssets`、`queryRetrieval`、
`listPlugins`、`updatePluginProfile`、`listThreads` 等 100 多個。
