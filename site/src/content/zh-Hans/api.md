# API 参考

守护进程暴露单一的 HTTP API（test 环境默认 `127.0.0.1:19192`，prod 为
`:19191`）。一切都是 JSON；契约以 JSON Schema 形式放在仓库的 `schemas/`
下，TypeScript SDK（`@kura/client`）与之保持一致。

## 认证

`/healthz`、`/version`、`/v1/system/info`、配对入口以及签名认证的 webhook
入站是开放的；**其余一切都在 bearer 令牌认证之后**（配对流程：
`POST /v1/auth/pairings/start`）。

## 路由族

| 族 | 要点 |
|----|------|
| Chat | `POST /v1/chat/query`、`POST /v1/chat/query/stream`（SSE）——一轮可能包含工具轮次 |
| Tools | `GET /v1/tools/capabilities`、`GET/POST /v1/tools/profiles`、`…/{id}/check`——搜索 / 图片 / 视频提供方，只保存 `secretRef` |
| Swarm | `POST /v1/swarm/runs`——有界的并发子 agent（需显式启用） |
| LLM | `GET /v1/llm/dispatches`、`…/stream`——每条 dispatch 记录，含消息 |
| Memory | `GET/POST /v1/memory/assets`、下钻、approve/reject/revoke、consolidate |
| Retrieval | `POST /v1/retrieval/queries`——带引用的融合召回 |
| Plugins | `GET /v1/plugins`（装配报告）、`GET/PUT /v1/plugins/profile` |
| Skills | `GET /v1/skills`、`POST /v1/skills/proposals`、`…/publish` |
| Improvement | `POST /v1/improvement/proposals`、`…/apply\|reject\|rollback` |
| Sessions | `GET /v1/sessions`、重置、事件历史 |
| Threads | 生命周期（reset/archive/reopen）、交接、连续性预览 |
| Connectors | 注册表、入站/消息流水线、投递结果 |
| Providers | 提供方注册表、托管认证、模型、健康检查 |
| Sandbox | `POST /v1/sandboxes/executions`、profile、explain |
| MCP | 服务器注册表、附属执行、webhook 入站 |
| Catalog | 条目/版本/信任层级、启用、回滚 |
| Routines / Reminders / Webhooks / Calendar / Mail / Triage | 自动化界面 |
| Policy | 审批队列、消费者策略同步 |
| Billing / Quota | 套餐、用量账本、配额、拒绝记录 |
| Evaluation | 活动、fixture、回放、实时验证 |
| Identity | 租户、成员、密钥、配对/令牌 |
| Release | `POST /v1/release/launch-gate`——以证据把关的发布决策 |
| Observability | `/healthz`、`/version`、`/v1/system/info`、事件流、`GET /metrics`（Prometheus 文本，需 bearer 认证） |
| Config | `GET/PUT /v1/config/file`——带比较交换、试运行和内联密钥拒绝的配置文件 |
| Memory ops | `GET /v1/memory/overview`、`POST /v1/memory/indexes/rebuild`、`GET /v1/context/assemblies` |

## 约定

- 线上字段一律 **camelCase**；枚举使用稳定的 snake-case 线上值。
- 错误为 `{ "error": message }`，并带有意义的状态码
  （400/401/403/404/409/422/5xx）。策略否决为 403。
- 变更都会发出事件；事件账本就是审计轨迹。
- 启动时输入（插件 profile）回应 `restartRequired: true`，而不是假装热应用。
- 每个响应都带 `x-request-id`（客户端提供时原样回显）；守护进程为每个请求
  写一行 `http.request` 访问日志，包含该 id、租户、路由模板、状态和时延。

## SDK

```bash
pnpm add @kura/client
```

`createKuraClient({ baseURL, token })` → 每个路由族的类型化方法：
`queryChat`、`streamChatQuery`、`listMemoryAssets`、`queryRetrieval`、
`listPlugins`、`updatePluginProfile`、`listThreads` 等 100 多个。
