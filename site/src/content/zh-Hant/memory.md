# 記憶

Kura 的記憶平面遵循分層的
[TencentDB-Agent-Memory](https://github.com/TencentCloud/TencentDB-Agent-Memory)
模型：一座由受治理、可歸因、可撤銷的資產組成的金字塔。

## 各層

| 層 | 內容 | 規則 |
|----|------|------|
| **L0** | 情節引用——指向真實對話/執行記錄的有界摘錄 | 真相留在來源連結背後 |
| **L1** | 原子：事實、偏好、約束、決定 | **必須帶來源連結**——沒有證據就沒有原子 |
| **L2** | 場景摘要 | 透過成員資產下鑽 |
| **L3** | 人設蒸餾 | 金字塔頂端 |

所有資產共享一個信封：kind（`chat_memory`/`skill`/`wiki`/`code_graph`）、
所有者、租戶、可見性（`private`/`team`/`agent`/`restricted`）、狀態、版本、
取代鏈、撤銷墓碑、保留期。

## 記憶如何寫入

五條捕獲路徑自動餵給 L0：

1. **對話輪次**（query + stream）——透過 memory 外掛的 `chat/turn-end` hook
2. **IM 閘道器流量**——同一個 hook（`sourceKind: channel`，帶訊息/執行緒/
   dispatch 連結）
3. **HTTP 入站**——被接受的入站訊息
4. **工作流終態**——任務結果
5. **工作階段逐出**——從上下文視窗中省略的片段在離開前被捕獲（絕不直接丟棄）

一個由 LLM 支撐的**整合器**在回覆路徑之外把 L0 → L1 原子 → L2 場景 → L3
人設逐層蒸餾（輪次觸發 + 60 秒空閒節拍）。提取出的原子必須引用可驗證的
證據——**編造的引用會被丟棄**。

## 治理

- 寫策略失敗即關閉：agent 撰寫的寫入需要操作員審批（Web 操作檯的稽核
  佇列）；擴大可見性需要審批。
- 每個資產都是白盒：Ready 狀態的 L2/L3 資產會投影為 `<data_dir>/memory/`
  下的 Markdown，可直接檢視。
- 撤銷墓碑與保留期清理是一等公民。

## API

```bash
GET  /v1/memory/assets?layer=l1&status=ready
POST /v1/memory/assets                       # 受治理的建立
GET  /v1/memory/assets/{id}/drilldown        # 通往證據的確定性路徑
POST /v1/memory/assets/{id}/approve|reject|revoke|visibility
POST /v1/memory/consolidate                  # 手動觸發
```

SDK：`listMemoryAssets`、`createMemoryAsset`、`getMemoryDrilldown`、
`approveMemoryAsset`、`consolidateMemory`……
