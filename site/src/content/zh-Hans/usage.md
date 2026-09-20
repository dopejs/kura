# 使用

## 通过 HTTP 对话

一次性查询：

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

每一轮都走完整流水线：技能与人设编译进提示词，线程连续性注入先前的对话，
**context 插件**注入记忆引导与按需召回，**session-strategy 插件**在预算内
整形窗口——而模型看到的内容，和 dispatch 记录里持久化的内容完全一致
（*模型可见 = 已记录*）。

## 工具

一轮对话可以调用工具。模型能看到什么，按轮次、按租户解析：

- **MCP 工具**——你接入的服务器发布的所有工具，在 `chat` surface 的暴露
  规则下授权；标记为 `approval_required` 的工具会等待人来回答，而不是直接
  失败。
- **`memory.lookup`**——在你自己的 Ready 记忆上召回，和
  `POST /v1/retrieval/queries` 用同一套融合排序；命中项以
  `Memory[<layer> <id>]` 形式引用。
- **`web.search` / `image.generate` / `video.generate`**——每个已配置的工具
  profile 对应一个工具。Kura 不替你选供应商：`mcp_backed` family 转发到你
  自己选定的 MCP 服务器（模型看到的是该工具真实的参数 schema），每次调用
  都经过配额与出站策略门。`builtin_stub` 用于在没有供应商账号时验证整条
  路径。

```bash
curl -s http://127.0.0.1:19192/v1/tools/profiles -H 'content-type: application/json' -d '{
  "title": "my search", "capability": "web.search", "family": "mcp_backed",
  "authMode": "none", "mcpServerId": "mcp_search", "mcpToolName": "search", "isDefault": true
}'
```

每次调用都是一条 `chat.tool.called` 事件和一个 `kura_chat_tool_calls_total`
指标；`chat/tool-call` hook 可以改写参数或否决调用。

## 浏览器

当 `computer-use` 插件配置为 `driver: "subprocess"` 时，computer-use 会话
会驱动真实浏览器：一个受监管的 Node worker（`capabilities/browser/worker.mjs`，
Playwright + Chromium）通过按行 JSON 协议和守护进程通信；挂起和崩溃会上报
给能力监管器，worker 会被重新拉起。

## TypeScript SDK

```ts
import { createKuraClient } from "@kura/client";

const client = createKuraClient({ baseURL: "http://127.0.0.1:19192" });

const result = await client.queryChat({ query: "hello", provider: "echo" });

// 流式
for await (const chunk of client.streamChatQuery({ query: "hi" })) {
  process.stdout.write(chunk.delta);
}

// 记忆、插件、检索……
const plugins = await client.listPlugins();
const recall  = await client.queryRetrieval({ query: "package manager" });
const assets  = await client.listMemoryAssets({ layer: "l1", status: "ready" });
```

## 终端界面

`kura tui` 启动全屏终端客户端：对话、线程连续性，以及实时的守护进程事件
流（`/events`）。

## Web 操作台

`kura web` 在本地启动已安装的 Web 操作台并打开浏览器。它是运维控制台：
记忆资产 + 审核队列、**插件**（装配报告、启用/禁用、hook）、频道、例程、
提供方、配额、诊断、评估以及支持类界面。

## 线程与连续性

传入 `threadId` 以保持连续性：先前的轮次以受治理的连续性记录保存，并在每次
dispatch 时重新渲染进上下文（有界、脱敏安全）。IM 频道自动获得**每线程一个
上下文**。

## 事件

所有可观测的东西都是事件（`llm.*`、`memory.*`、`context.*`、`chat.*`、
`improvement.*`、`system.*`），持久化在存储账本中并发布到实时总线。TUI 的
`/events` 视图和 sessions API 都能查看。
