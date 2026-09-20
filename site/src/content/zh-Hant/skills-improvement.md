# 技能與自我改進

兩者遵循同一條產品規則：**agent 提議，操作員治理**。提議/審批迴圈之外
沒有自主行為。

## Agent 管理的技能

一個技能提案*就是*一個受治理的記憶資產（`kind: skill`）：

1. **提議**——`POST /v1/skills/proposals`，帶名稱、描述、正文和**動機證據
   連結**（必填——校驗器拒絕無證據的提案）。agent 作者身份會被寫策略強制
   置為 `Pending`，進入標準的記憶稽核佇列。
2. **稽核**——操作員透過既有的記憶稽核（`/v1/memory/assets/{id}/approve`）
   批准或拒絕。
3. **釋出**——`POST /v1/skills/proposals/{id}/publish`（僅限已批准的提案；
   其他一律拒絕）。釋出會把 `SKILL.md` 包寫入 `<data_dir>/skills/`，註冊一
   個 Community 目錄項，其版本來源永久記錄 `memory:<assetId>`（溯源鏈），
   並重新載入登錄檔。

執行時的守衛是結構性的：技能登錄檔只掃描技能目錄，因此待審或被拒的提案
**永遠不可載入**。

## 經審計的自我改進

操作員可審計、可否決的閉環。當前的目標類別：外掛 profile 設定值（如工作階段
預算、上下文預算）。

```bash
POST /v1/improvement/proposals
{
  "targetPlugin": "session-strategy",
  "configKey": "personalBudgetChars",
  "currentValue": 48000,
  "proposedValue": 64000,
  "predictedEffect": "fewer elisions in long sessions",
  "evidenceLinks": [{ "kind": "event", "id": "evt_ctx_123" }],
  "proposedBy": "agent"
}
```

程式碼中強制的硬規則：

- **必須有證據**——沒有動機證據的提案會被拒絕。
- **有速率上限**——每個目標每 24 小時的提案數上限；上限是操作員設定
  （`self-improve` 外掛設定），刻意不允許 agent 調整。
- **沒有回滾路徑就不改**——應用前把*完整的先前 profile* 快照進提案，再原子
  地重寫 `plugins.json`；`POST …/{id}/rollback` 精確恢復。
- **完整審計鏈**——`improvement.proposed → applied → kept/rolled_back` 都是
  事件；提案以白盒 JSON 持久化在 `<data_dir>/improvement/` 下，重啟後仍在。

帶回歸觸發自動回滾的後續評估是路線圖上的下一片。
