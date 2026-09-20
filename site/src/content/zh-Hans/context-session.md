# 上下文与会话

对个人 agent 来说，**上下文管理就是会话管理**——在 Kura 里两者都是
`chat/pre-dispatch` 瀑布上的插件，所以任何插件（内置或外部）都能修改或
替换这套策略。

## 默认的 context 插件

在瀑布中最先运行。确定性、有预算、始终带引用：

1. **记忆引导**——租户的 Ready **L3 人设**，然后是 **L2 场景**（最新优先），
   在 `memoryBudgetChars`（默认 4000）预算内作为系统框架消息注入。每条注入
   的内容都内联引用：`Memory[l3 mem_xxx] persona: …`——召回的记忆是证据，
   而不是裸文本。L1 原子**绝不批量注入**。
2. **按需召回**——本轮最后一条用户消息通过三路排序融合召回 Ready 的 L1
   原子：BM25 + 时近性 + 字符三元组向量，以倒数排名融合（RRF）合并。没有
   词法或向量关联的原子绝不会被召回。预算：`retrievalBudgetChars`（默认
   2000）。以 `Memory[l1 …] (recalled): …` 形式注入。
3. **符号压缩**——过大的非框架消息（默认 >8000 字符）外置为一条全文的 L0
   记忆引用；窗口里保留 200 字符预览加引用。token 开销下降，证据路径仍在
   （`GET /v1/memory/assets/{id}`）。
4. **绑定感知装载**——`agent` 可见性的资产只为绑定了它的当前 agent profile
   注入。

每次装配都会发出一条 `context.assembled` 事件，携带 **AssemblyRecord**：
每一项纳入（资产、层、字符数、来源）和每一项排除及其原因
（`over_budget`/`empty_content`/`visibility`）。没有任何内容会悄悄进入或
错过上下文。

## session-strategy 插件

在 context 之后运行。保留框架的窗口整形：

- 系统消息（人设、技能、安全、注入的记忆）是**框架**——绝不省略。
- 最近的 `keepRecent` 条消息始终保留。
- 超出预算时，最早的历史坍缩为一个标记——而被省略的片段会**先捕获到记忆**
  （线程关联的 L0），标记引用它：`…elided span captured as Memory[l0_ref mem_x]`。
  模型可以回溯。逐出绝不销毁上下文。

两个预算，按来源区分：**个人**长会话（48k 字符）和 **IM 线程**
（`sourceKind: channel`，16k）——每线程一个上下文。

## 按需检索 API

同一套融合排序，开放给客户端和工具：

```bash
curl -s http://127.0.0.1:19192/v1/retrieval/queries \
  -H 'content-type: application/json' \
  -d '{"query": "which package manager do we use", "limit": 5}'
```

命中项带有排名、来源链接和可下钻的成员 id。
