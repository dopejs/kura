# 外掛

**信任邊界核心之外的一切都是外掛。** 核心（儲存、事件匯流排、身份、認證、
策略、金鑰、審計）不能被禁用；其餘一切——LLM 派發、對話、記憶、上下文、
工作階段策略、頻道、計費、排程器，以及 20 多個其他子系統——都作為宣告瞭依賴的
具名外掛裝配起來。

## profile：`<data_dir>/plugins.json`

啟動時讀取。檔案缺失表示"全部啟用"；檔案格式錯誤會讓啟動明確失敗。

```json
{
  "disabled": ["channel-discord"],
  "entries": {
    "session-strategy": {
      "config": { "personalBudgetChars": 64000, "keepRecent": 4 }
    },
    "context": {
      "config": { "memoryBudgetChars": 4000, "retrievalBudgetChars": 2000 }
    },
    "self-improve": {
      "config": { "maxPerTargetPerDay": 3 }
    }
  }
}
```

- 禁用一個外掛會讓它的管理器不被裝配（API 回答"未設定"），並且**傳遞性地
  禁用依賴它的外掛**——例如禁用 `billing` 會禁用 `webhooks`（它的配額門由
  計費支撐且失敗即關閉；絕不會半裝配地執行）。
- 頻道外掛門控它們的聯結器執行時：profile 優先於設定開關，禁用時不會觸碰
  任何網路或憑據。

## 自省

```bash
curl -s http://127.0.0.1:19192/v1/plugins           # 裝配報告
curl -s http://127.0.0.1:19192/v1/plugins/profile    # 磁碟上的 profile
```

`GET /v1/plugins` 是外掛平面的"匯出設定"：按構建順序列出每個外掛的啟用
狀態、禁用原因、提供的 seam、依賴邊、hook 註冊和 profile 警告。
`PUT /v1/plugins/profile` 校驗並原子替換 profile（`restartRequired: true`——
profile 是啟動時輸入）。Web 操作檯的外掛頁提供啟用/禁用開關。

## Hook 點（瀑布）

外掛把行為掛在具名的 hook 點上；處理器按註冊順序執行，可以**修改
payload**，或**中止**（否決）：

| 點位 | Payload | 能力 |
|------|---------|------|
| `chat/turn-start` | `tenantId, threadId, query, sourceKind` | 改寫查詢、否決本輪 |
| `chat/pre-dispatch` | `+ agentProfileId, provider, model, messages[]` | 改寫上下文/提供方/模型、否決 |
| `chat/turn-end` | `+ dispatchId, output, status, turn ids, skills` | 觀察已結束的輪次 |
| `chat/tool-call` | `tenantId, threadId, agentProfileId, callId, name, arguments` | 改寫引數、否決呼叫（模型會被告知） |

`chat/pre-dispatch` 在 **dispatch 記錄持久化之前**執行，所以持久化的訊息
與提供方收到的位元組完全一致——這就是*模型可見 = 已記錄*不變數。否決返回
HTTP 403 並記錄為 `chat.hook.vetoed` 事件。

## 工具呼叫

一輪對話是一個 agent 迴圈，而不是單次 dispatch：模型被提供工具，當它請求
某個工具時，守護程序執行它、附加結果、再次派發——受 `toolMaxRounds`
（`entries.chat.config`，預設 16）限制。每一輪都是獨立持久化的 dispatch
（含它被提供的工具與發出的呼叫），每次呼叫都是一條 `chat.tool.called` 事件和
一個指標，並且 `chat/tool-call` hook 在每次呼叫前執行。模型看到什麼由裝配
層**按輪次、按租戶**解析：已接入 MCP 伺服器釋出的工具（在 `chat` surface
的暴露規則下授權；需要審批的工具會等待人來回答）、租戶自己記憶上的
`memory.lookup`，以及每個已設定工具 profile 對應的一個工具（`web.search`、
`image.generate`、`video.generate`）——見**使用 → 工具**。

## 預設外掛陣容（節選）

| 外掛 | 角色 |
|------|------|
| `context` | 記憶引導 + 按需召回（瀑布的第一個） |
| `session-strategy` | 視窗整形：個人 48k / IM 執行緒 16k 預算 |
| `memory` | L0–L3 記憶平面 + 捕獲 hook + 60 秒整合節拍 |
| `self-improve` | 經審計的設定調優提案 |
| `tools` | 工具提供方 profile（搜尋 / 圖片 / 影片），受配額與出站門保護 |
| `computer-use` | 瀏覽器工作階段；`driver: "subprocess"` 執行受監管的 Playwright worker |
| `swarm` | 有界的併發子 agent（需顯式啟用） |
| `channel-*` | Discord / Telegram / Slack / Matrix 執行時 |
