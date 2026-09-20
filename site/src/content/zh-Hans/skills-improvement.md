# 技能与自我改进

两者遵循同一条产品规则：**agent 提议，操作员治理**。提议/审批循环之外
没有自主行为。

## Agent 管理的技能

一个技能提案*就是*一个受治理的记忆资产（`kind: skill`）：

1. **提议**——`POST /v1/skills/proposals`，带名称、描述、正文和**动机证据
   链接**（必填——校验器拒绝无证据的提案）。agent 作者身份会被写策略强制
   置为 `Pending`，进入标准的记忆审核队列。
2. **审核**——操作员通过既有的记忆审核（`/v1/memory/assets/{id}/approve`）
   批准或拒绝。
3. **发布**——`POST /v1/skills/proposals/{id}/publish`（仅限已批准的提案；
   其他一律拒绝）。发布会把 `SKILL.md` 包写入 `<data_dir>/skills/`，注册一
   个 Community 目录项，其版本来源永久记录 `memory:<assetId>`（溯源链），
   并重新加载注册表。

运行时的守卫是结构性的：技能注册表只扫描技能目录，因此待审或被拒的提案
**永远不可加载**。

## 经审计的自我改进

操作员可审计、可否决的闭环。当前的目标类别：插件 profile 配置值（如会话
预算、上下文预算）。

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

代码中强制的硬规则：

- **必须有证据**——没有动机证据的提案会被拒绝。
- **有速率上限**——每个目标每 24 小时的提案数上限；上限是操作员配置
  （`self-improve` 插件配置），刻意不允许 agent 调整。
- **没有回滚路径就不改**——应用前把*完整的先前 profile* 快照进提案，再原子
  地重写 `plugins.json`；`POST …/{id}/rollback` 精确恢复。
- **完整审计链**——`improvement.proposed → applied → kept/rolled_back` 都是
  事件；提案以白盒 JSON 持久化在 `<data_dir>/improvement/` 下，重启后仍在。

带回归触发自动回滚的后续评估是路线图上的下一片。
