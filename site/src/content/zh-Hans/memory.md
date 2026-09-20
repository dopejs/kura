# 记忆

Kura 的记忆平面遵循分层的
[TencentDB-Agent-Memory](https://github.com/TencentCloud/TencentDB-Agent-Memory)
模型：一座由受治理、可归因、可撤销的资产组成的金字塔。

## 各层

| 层 | 内容 | 规则 |
|----|------|------|
| **L0** | 情节引用——指向真实对话/运行记录的有界摘录 | 真相留在来源链接背后 |
| **L1** | 原子：事实、偏好、约束、决定 | **必须带来源链接**——没有证据就没有原子 |
| **L2** | 场景摘要 | 通过成员资产下钻 |
| **L3** | 人设蒸馏 | 金字塔顶端 |

所有资产共享一个信封：kind（`chat_memory`/`skill`/`wiki`/`code_graph`）、
所有者、租户、可见性（`private`/`team`/`agent`/`restricted`）、状态、版本、
取代链、撤销墓碑、保留期。

## 记忆如何写入

五条捕获路径自动喂给 L0：

1. **对话轮次**（query + stream）——通过 memory 插件的 `chat/turn-end` hook
2. **IM 网关流量**——同一个 hook（`sourceKind: channel`，带消息/线程/
   dispatch 链接）
3. **HTTP 入站**——被接受的入站消息
4. **工作流终态**——任务结果
5. **会话逐出**——从上下文窗口中省略的片段在离开前被捕获（绝不直接丢弃）

一个由 LLM 支撑的**整合器**在回复路径之外把 L0 → L1 原子 → L2 场景 → L3
人设逐层蒸馏（轮次触发 + 60 秒空闲节拍）。提取出的原子必须引用可验证的
证据——**编造的引用会被丢弃**。

## 治理

- 写策略失败即关闭：agent 撰写的写入需要操作员审批（Web 操作台的审核
  队列）；扩大可见性需要审批。
- 每个资产都是白盒：Ready 状态的 L2/L3 资产会投影为 `<data_dir>/memory/`
  下的 Markdown，可直接查看。
- 撤销墓碑与保留期清理是一等公民。

## API

```bash
GET  /v1/memory/assets?layer=l1&status=ready
POST /v1/memory/assets                       # 受治理的创建
GET  /v1/memory/assets/{id}/drilldown        # 通往证据的确定性路径
POST /v1/memory/assets/{id}/approve|reject|revoke|visibility
POST /v1/memory/consolidate                  # 手动触发
```

SDK：`listMemoryAssets`、`createMemoryAsset`、`getMemoryDrilldown`、
`approveMemoryAsset`、`consolidateMemory`……
