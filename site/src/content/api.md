# API Reference

The daemon exposes a single HTTP API (default `127.0.0.1:19192` in test,
`:19191` in prod). Everything is JSON; contracts live as JSON Schemas
under `schemas/` in the repo and the TypeScript SDK (`@kura/client`)
mirrors them.

## Authentication

`/healthz`, `/version`, `/v1/system/info`, the pairing entry points, and
signature-authenticated webhook ingress are open; **everything else runs
behind bearer-token auth** (pairing flow: `POST /v1/auth/pairings/start`).

## Route families

| Family | Highlights |
|--------|-----------|
| Chat | `POST /v1/chat/query`, `POST /v1/chat/query/stream` (SSE) — a turn may run tool rounds |
| Tools | `GET /v1/tools/capabilities`, `GET/POST /v1/tools/profiles`, `…/{id}/check` — search / image / video providers, `secretRef` only |
| Swarm | `POST /v1/swarm/runs` — bounded concurrent sub-agents (opt-in) |
| LLM | `GET /v1/llm/dispatches`, `…/stream` — every dispatch record, messages included |
| Memory | `GET/POST /v1/memory/assets`, drilldown, approve/reject/revoke, consolidate |
| Retrieval | `POST /v1/retrieval/queries` — fused recall with citations |
| Plugins | `GET /v1/plugins` (assembly report), `GET/PUT /v1/plugins/profile` |
| Skills | `GET /v1/skills`, `POST /v1/skills/proposals`, `…/publish` |
| Improvement | `POST /v1/improvement/proposals`, `…/apply\|reject\|rollback` |
| Sessions | `GET /v1/sessions`, reset, event history |
| Threads | lifecycle (reset/archive/reopen), handoffs, continuity previews |
| Connectors | registry, ingress/messages pipeline, delivery outcomes |
| Providers | provider registry, managed auth, models, health checks |
| Sandbox | `POST /v1/sandboxes/executions`, profiles, explain |
| MCP | server registry, attached executions, webhook ingress |
| Catalog | items/versions/trust tiers, enablements, rollback |
| Routines / Reminders / Webhooks / Calendar / Mail / Triage | automation surfaces |
| Policy | approvals queue, consumer-policy sync |
| Billing / Quota | plans, usage ledgers, quotas, denials |
| Evaluation | campaigns, fixtures, replay, live validation |
| Identity | tenants, memberships, secrets, pairing/tokens |
| Release | `POST /v1/release/launch-gate` — the evidence-gated ship decision |
| Observability | `/healthz`, `/version`, `/v1/system/info`, event streams, `GET /metrics` (Prometheus text, bearer-authenticated) |
| Config | `GET/PUT /v1/config/file` — the config file with compare-and-swap, dry run and inline-secret refusal |
| Memory ops | `GET /v1/memory/overview`, `POST /v1/memory/indexes/rebuild`, `GET /v1/context/assemblies` |

## Conventions

- **camelCase** wire fields everywhere; enums use stable snake-case wire
  values.
- Errors are `{ "error": message }` with meaningful status codes
  (400/401/403/404/409/422/5xx). Policy vetoes are 403.
- Mutations emit events; the event ledger is the audit trail.
- Boot-time inputs (the plugin profile) respond with
  `restartRequired: true` rather than pretending to hot-apply.
- Every response carries `x-request-id` (echoed when the client sent one);
  the daemon writes one `http.request` access-log line per request with
  that id, the tenant, the route template, status and latency.

## SDK

```bash
pnpm add @kura/client
```

`createKuraClient({ baseURL, token })` → typed methods for every family:
`queryChat`, `streamChatQuery`, `listMemoryAssets`, `queryRetrieval`,
`listPlugins`, `updatePluginProfile`, `listThreads`, and 100+ more.
