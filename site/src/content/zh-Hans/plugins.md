# 插件

**信任边界内核之外的一切都是插件。** 内核（存储、事件总线、身份、认证、
策略、密钥、审计）不能被禁用；其余一切——LLM 派发、对话、记忆、上下文、
会话策略、频道、计费、调度器，以及 20 多个其他子系统——都作为声明了依赖的
具名插件装配起来。

## profile：`<data_dir>/plugins.json`

启动时读取。文件缺失表示"全部启用"；文件格式错误会让启动明确失败。

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

- 禁用一个插件会让它的管理器不被装配（API 回答"未配置"），并且**传递性地
  禁用依赖它的插件**——例如禁用 `billing` 会禁用 `webhooks`（它的配额门由
  计费支撑且失败即关闭；绝不会半装配地运行）。
- 频道插件门控它们的连接器运行时：profile 优先于配置开关，禁用时不会触碰
  任何网络或凭据。

## 自省

```bash
curl -s http://127.0.0.1:19192/v1/plugins           # 装配报告
curl -s http://127.0.0.1:19192/v1/plugins/profile    # 磁盘上的 profile
```

`GET /v1/plugins` 是插件平面的"导出配置"：按构建顺序列出每个插件的启用
状态、禁用原因、提供的 seam、依赖边、hook 注册和 profile 警告。
`PUT /v1/plugins/profile` 校验并原子替换 profile（`restartRequired: true`——
profile 是启动时输入）。Web 操作台的插件页提供启用/禁用开关。

## Hook 点（瀑布）

插件把行为挂在具名的 hook 点上；处理器按注册顺序运行，可以**修改
payload**，或**中止**（否决）：

| 点位 | Payload | 能力 |
|------|---------|------|
| `chat/turn-start` | `tenantId, threadId, query, sourceKind` | 改写查询、否决本轮 |
| `chat/pre-dispatch` | `+ agentProfileId, provider, model, messages[]` | 改写上下文/提供方/模型、否决 |
| `chat/turn-end` | `+ dispatchId, output, status, turn ids, skills` | 观察已结束的轮次 |
| `chat/tool-call` | `tenantId, threadId, agentProfileId, callId, name, arguments` | 改写参数、否决调用（模型会被告知） |

`chat/pre-dispatch` 在 **dispatch 记录持久化之前**运行，所以持久化的消息
与提供方收到的字节完全一致——这就是*模型可见 = 已记录*不变量。否决返回
HTTP 403 并记录为 `chat.hook.vetoed` 事件。

## 工具调用

一轮对话是一个 agent 循环，而不是单次 dispatch：模型被提供工具，当它请求
某个工具时，守护进程执行它、附加结果、再次派发——受 `toolMaxRounds`
（`entries.chat.config`，默认 16）限制。每一轮都是独立持久化的 dispatch
（含它被提供的工具与发出的调用），每次调用都是一条 `chat.tool.called` 事件和
一个指标，并且 `chat/tool-call` hook 在每次调用前运行。模型看到什么由装配
层**按轮次、按租户**解析：已接入 MCP 服务器发布的工具（在 `chat` surface
的暴露规则下授权；需要审批的工具会等待人来回答）、租户自己记忆上的
`memory.lookup`，以及每个已配置工具 profile 对应的一个工具（`web.search`、
`image.generate`、`video.generate`）——见**使用 → 工具**。

## 默认插件阵容（节选）

| 插件 | 角色 |
|------|------|
| `context` | 记忆引导 + 按需召回（瀑布的第一个） |
| `session-strategy` | 窗口整形：个人 48k / IM 线程 16k 预算 |
| `memory` | L0–L3 记忆平面 + 捕获 hook + 60 秒整合节拍 |
| `self-improve` | 经审计的配置调优提案 |
| `tools` | 工具提供方 profile（搜索 / 图片 / 视频），受配额与出站门保护 |
| `computer-use` | 浏览器会话；`driver: "subprocess"` 运行受监管的 Playwright worker |
| `swarm` | 有界的并发子 agent（需显式启用） |
| `channel-*` | Discord / Telegram / Slack / Matrix 运行时 |
