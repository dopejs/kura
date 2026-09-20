# 上下文與工作階段

對個人 agent 來說，**上下文管理就是工作階段管理**——在 Kura 裡兩者都是
`chat/pre-dispatch` 瀑布上的外掛，所以任何外掛（內建或外部）都能修改或
替換這套策略。

## 預設的 context 外掛

在瀑布中最先執行。確定性、有預算、始終帶引用：

1. **記憶引導**——租戶的 Ready **L3 人設**，然後是 **L2 場景**（最新優先），
   在 `memoryBudgetChars`（預設 4000）預算內作為系統框架訊息注入。每條注入
   的內容都內聯引用：`Memory[l3 mem_xxx] persona: …`——召回的記憶是證據，
   而不是裸文字。L1 原子**絕不批次注入**。
2. **按需召回**——本輪最後一條使用者訊息透過三路排序融合召回 Ready 的 L1
   原子：BM25 + 時近性 + 字元三元組向量，以倒數排名融合（RRF）合併。沒有
   詞法或向量關聯的原子絕不會被召回。預算：`retrievalBudgetChars`（預設
   2000）。以 `Memory[l1 …] (recalled): …` 形式注入。
3. **符號壓縮**——過大的非框架訊息（預設 >8000 字元）外接為一條全文的 L0
   記憶引用；視窗裡保留 200 字元預覽加引用。token 開銷下降，證據路徑仍在
   （`GET /v1/memory/assets/{id}`）。
4. **繫結感知裝載**——`agent` 可見性的資產只為繫結了它的當前 agent profile
   注入。

每次裝配都會發出一條 `context.assembled` 事件，攜帶 **AssemblyRecord**：
每一項納入（資產、層、字元數、來源）和每一項排除及其原因
（`over_budget`/`empty_content`/`visibility`）。沒有任何內容會悄悄進入或
錯過上下文。

## session-strategy 外掛

在 context 之後執行。保留框架的視窗整形：

- 系統訊息（人設、技能、安全、注入的記憶）是**框架**——絕不省略。
- 最近的 `keepRecent` 條訊息始終保留。
- 超出預算時，最早的歷史坍縮為一個標記——而被省略的片段會**先捕獲到記憶**
  （執行緒關聯的 L0），標記引用它：`…elided span captured as Memory[l0_ref mem_x]`。
  模型可以回溯。逐出絕不銷燬上下文。

兩個預算，按來源區分：**個人**長工作階段（48k 字元）和 **IM 執行緒**
（`sourceKind: channel`，16k）——每執行緒一個上下文。

## 按需檢索 API

同一套融合排序，開放給客戶端和工具：

```bash
curl -s http://127.0.0.1:19192/v1/retrieval/queries \
  -H 'content-type: application/json' \
  -d '{"query": "which package manager do we use", "limit": 5}'
```

命中項帶有排名、來源連結和可下鑽的成員 id。
