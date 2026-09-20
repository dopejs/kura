# Usage

## Chat over HTTP

One-shot query:

```bash
curl -s http://127.0.0.1:19192/v1/chat/query \
  -H 'content-type: application/json' \
  -d '{
    "query": "summarize my day",
    "provider": "claude_code_cli",
    "threadId": "thr_personal"
  }'
```

Streaming (SSE):

```bash
curl -N http://127.0.0.1:19192/v1/chat/query/stream \
  -H 'content-type: application/json' \
  -d '{"query": "hello", "provider": "echo"}'
```

Every turn runs the full pipeline: skills + persona compile into the
prompt, thread continuity injects prior turns, the **context plugin**
injects memory bootstrap and query-time recall, the **session-strategy
plugin** shapes the window under budget — and whatever the model sees is
exactly what is persisted on the dispatch record
(*model-visible = logged*).

## Tools

A turn can call tools. What the model is offered is resolved per turn and
per tenant:

- **MCP tools** — everything the servers you connected publish, authorized
  under the `chat` surface's exposure rules; a tool marked
  `approval_required` waits for a person to answer instead of failing.
- **`memory.lookup`** — recall over your own Ready memory, the same fused
  ranking as `POST /v1/retrieval/queries`; hits cite `Memory[<layer> <id>]`.
- **`web.search` / `image.generate` / `video.generate`** — one tool per
  configured tool profile. Kura does not pick a vendor: the `mcp_backed`
  family forwards to an MCP server you chose (the model sees that tool's
  real argument schema), and every call runs through the quota and egress
  gates. `builtin_stub` proves the path without a vendor account.

```bash
curl -s http://127.0.0.1:19192/v1/tools/profiles -H 'content-type: application/json' -d '{
  "title": "my search", "capability": "web.search", "family": "mcp_backed",
  "authMode": "none", "mcpServerId": "mcp_search", "mcpToolName": "search", "isDefault": true
}'
```

Every call is a `chat.tool.called` event and a `kura_chat_tool_calls_total`
metric, and the `chat/tool-call` hook can rewrite or veto it.

## Browser

`computer-use` sessions drive a real browser when the plugin is configured
with `driver: "subprocess"`: a supervised Node worker
(`capabilities/browser/worker.mjs`, Playwright + Chromium) speaks a
line-JSON protocol to the daemon; hangs and crashes are reported to the
capability supervisor and the worker is respawned.

## TypeScript SDK

```ts
import { createKuraClient } from "@kura/client";

const client = createKuraClient({ baseURL: "http://127.0.0.1:19192" });

const result = await client.queryChat({ query: "hello", provider: "echo" });

// Streaming
for await (const chunk of client.streamChatQuery({ query: "hi" })) {
  process.stdout.write(chunk.delta);
}

// Memory, plugins, retrieval…
const plugins = await client.listPlugins();
const recall  = await client.queryRetrieval({ query: "package manager" });
const assets  = await client.listMemoryAssets({ layer: "l1", status: "ready" });
```

## Terminal UI

`kura tui` launches the full-screen terminal client: conversations,
thread continuity, and a live daemon event stream (`/events`).

## Web operator shell

`kura web` serves the installed web shell locally and opens your
browser. It is the operator console: memory
assets + review queue, **Plugins** (assembly report, enable/disable,
hooks), channels, routines, providers, quota, diagnostics, evaluation,
and support surfaces.

## Threads and continuity

Pass a `threadId` to keep continuity: prior turns are stored as governed
continuity records and re-rendered into each dispatch (bounded, redaction
safe). IM channels get **one context per thread** automatically.

## Events

Everything observable is an event (`llm.*`, `memory.*`, `context.*`,
`chat.*`, `improvement.*`, `system.*`), persisted in the store ledger and
published on the live bus. The TUI `/events` view and the sessions API
expose them.
