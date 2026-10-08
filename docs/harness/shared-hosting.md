# Shared hosting: one daemon, many tenants

> Status: design; M1 boundary fixes (§4.5) done, shared mode (§4.1-4.4) in
> progress (2026-10-08). Owner: daemon.
> Replaces the "one daemon container per user" pilot in the private
> `kura-gateway` repo as the target architecture. That pilot stays as a
> dedicated tier; nothing here removes it.

## 1. Goal

One Kura daemon serves many end users who sign up on their own. An idle user
costs a few rows in SQLite, not a resident process. The gateway in front owns
sign-up, login (GitHub/Google OAuth) and routing; the daemon owns everything
else and enforces tenant boundaries itself — a gateway bug must not be enough
to cross them.

Non-goals for now: billing, multiple daemon replicas, kernel-grade isolation
on day one (M3 starts with hardened containers; gVisor/microVM later).

## 2. Where the daemon stands (audit of `a4c6ee1`)

The data plane is in good shape: sessions, runs, the event stream (filtered per
subscriber), chat, memory, reminders, calendar, mail, workflows, policy, MCP
server rows are tenant-scoped, and `X-Kura-Tenant-ID` is checked against both
the token grant and an active membership (`iam/identity/src/resolver.rs:64-133`).
Nothing is spawned per idle tenant.

The control plane and the execution plane still assume that everyone who can
authenticate is the machine's owner:

| # | Gap | Evidence | Milestone |
|---|---|---|---|
| G1 | Any authenticated caller can run any command as the daemon uid with `data_dir` as cwd: `POST /v1/sandboxes/executions` has no permission check and the caller picks the profile; default roots are the whole `data_dir`; only curl/ssh/scp/rm need approval | `routes/sandboxes.rs:181-194`, `sandbox/src/manager.rs:2036-2050,2668-2675` | M1 (gate), M3 (isolate) |
| G2 | Provider account/credential/default-model/model-role writes have no permission check and rewrite the one global dispatcher (arbitrary `base_url` + headers) | `routes/providers.rs:64,180,384,816` | M1 |
| G3 | "May change the daemon" = Owner of *any* tenant; every self-serve user owns a tenant | `middleware.rs:357-371` | M1 |
| G4 | Tenant taken from the request instead of the resolved context: retrieval, webhooks, connector register/ingress, exec-profile select; connector list is unscoped | `routes/retrieval.rs:54-63`, `routes/mcp.rs:509-595`, `routes/connectors.rs:300-322,819`, `routes/execprofile.rs:105-119` | M1 |
| G5 | `PATCH /v1/principals/{id}` changes a principal that may not belong to the caller's tenant | `routes/auth.rs:1494-1515` | M1 |
| G6 | The route audit gate counts the literal `tenant_id` as a tenancy marker, so G4 passed it | `api/tests/route_tenancy_audit.rs:46-56` | M1 |
| G7 | No way to provision principal + personal tenant + token except pairing, which is unauthenticated and not loopback-only | `routes/auth.rs:80-87`, `app/src/lib.rs:271-281` | M1 (loopback), M2 (provisioning) |
| G8 | One global LLM dispatcher; no per-tenant credentials (BYOK) or token budget | `engine/llm/src/dispatcher.rs:48,76`, `billing/src/catalog.rs:188-194` | M2 |
| G9 | MCP stdio servers, external plugins, the browser worker, adapter RPC run on the daemon host as the daemon uid; plugins/browser inherit the full env; one Chromium context for everyone | `mcp/src/manager.rs:3484-3527`, `app/src/external.rs:81-88`, `capabilities/browser/worker.mjs:45` | M1 (operator-only), M3 |
| G10 | Skills, prompt overlays, connectors are global; connectors bind to the oldest personal tenant | `skills/src/lib.rs:230-242`, `store/src/tenancy.rs:1275-1283` | M1 (operator-only), later per tenant |
| G11 | Egress (SSRF) checks only cover tool-profile `base_url`; not MCP http/ws, browser, sandbox; DNS rebinding open | `egress/src/lib.rs:1-24` | M3 |
| G12 | No rate limit, concurrency cap or run-queue fairness | — | M4 |
| G14 | Token rotate/revoke/grant-update need only `TenantManage` (every tenant owner) and then act on **any** token; rotate returns the new secret, so a tenant owner could take over the operator token. Token list with `principalId` showed any principal's tokens | `routes/auth.rs` `auth_token_rotate`/`revoke`/`grant_update`/`list` | M1 (all modes) |
| G15 | Chat turns are offered every tenant's MCP tools: `tools_for_surface` uses `list_servers()` and `authorize_tool` takes no tenant, so tenant A's turn can call tools tenant B allowlisted for `chat`, on B's server | `mcp/src/agent_tool.rs:259-278`, `mcp/src/manager.rs:455,689-787`, `app/src/tool_host.rs:63-65` | M1 (all modes) |
| G13 | Token auth scans all tokens under a write lock; a write per request; one SQLite writer; reader pool off; membership lookup capped at 500 | `identity/src/auth.rs:416-450`, `middleware.rs:212`, `store/src/pool.rs` | M4 |

## 3. Milestones

| | Scope | Result |
|---|---|---|
| **M1** shared mode + boundary fixes | `KURA_HOSTING=shared`; platform operator ≠ tenant owner; deny-by-default route policy; pairing loopback-only; fix G2–G6 | Strangers can share a daemon safely; no code execution for tenants |
| **M2** provisioning + BYOK | Operator-only provisioning API (principal, personal tenant, membership, token); per-tenant LLM credentials selected at dispatch; gateway OAuth sign-up | Self-serve sign-up; chat, memory, reminders, retrieval with the user's own key |
| **M3** isolated execution | Per-run hardened container (own uid, only the tenant's workspace mounted, cgroup limits, egress through `kura-egress`, DNS pinned); per-tenant browser contexts; MCP stdio in the same plane; remote worker later | Tenants get code execution and MCP back |
| **M4** fairness + scale | Per-tenant rate/concurrency/token limits, fair run queue, indexed token lookup, reader pool on, no write per request | More users per host |

## 4. M1 design

### 4.1 Hosting mode

`KURA_HOSTING` = `single` (default, today's behavior) | `shared`. Read by
`kura-app` and carried in `AppState::hosting`; it is not a `Config` field so the
change does not ripple through every `Config` literal. Shared mode requires
`KURA_ENV=hosted` (the embedded identity bootstrap); the daemon refuses to start
otherwise.

In shared mode the bootstrapped `ten_local` / `prn_local_operator` is the
**platform operator**: the one tenant whose Owner may change the daemon. Its
token comes from loopback pairing (`docker exec` / the host), exactly as the
pilot's node agent provisions cells today. End-user tenants are created later by
the operator (M2) and are never the operator tenant.

### 4.2 Operator check

`hosting::is_platform_operator(ctx)` = shared mode ⇒ `ctx.tenant_id ==
ten_local && role == Owner`; single mode ⇒ unchanged (`Owner`, or no tenant).
`require_daemon_global_operator` delegates to it, so the existing guards on
plugins, improvement, capabilities, sandbox reload and the config file are
correct in both modes.

### 4.3 Route policy (deny by default)

A layer inside `protected()` (it needs the resolved tenant) that runs only in
shared mode. It looks up `(method, MatchedPath)` in one table:

- **tenant**: any member, subject to the handler's own checks;
- everything not listed: **operator only** (403 `hosting_operator_only`).

A route added later is operator-only until someone decides otherwise, which is
the safe failure. A test asserts every table entry names a real route, so the
table cannot silently drift.

M1 tenant allowlist (reads and writes on the caller's own data only):
sessions, chat (query + stream), runs (read, cancel), events (list + stream),
memory, retrieval, reminders, routine/triage, approvals the caller can resolve,
auth self-service (`/v1/auth/me`, own tokens), tenant/principal reads, providers
and models **read**. Explicitly operator-only in M1: sandboxes, tools, MCP,
skills/skill proposals, plugins, capabilities, connectors, channel management,
webhooks, integrations, calendar/mail (they depend on global connectors),
computer use, providers **write**, config, improvement, release, evaluation,
setup wizard, workspace bindings, exec profiles, swarm, metrics.

### 4.4 Pairing

In shared mode `/v1/auth/pairings/*` answers 404 unless the peer address is
loopback. `serve` switches to `into_make_service_with_connect_info`.

### 4.5 Boundary fixes (all modes)

- Retrieval, webhooks, exec-profile select, connector register: the resolved
  tenant wins; a different tenant in the request is a 403. Connector list is
  filtered to the acting tenant.
- `PATCH /v1/principals/{id}`: the target must hold an active membership in the
  caller's tenant (else 404) and none in another tenant (else 409: remove the
  membership instead — a principal-wide switch is not a tenant's to flip).
- Audit gate: `tenant_id` alone no longer counts as a tenancy marker; a family
  must use the resolved context (`TenantContext`, `scoped_tenant`,
  `for_tenant`, the guards).

Implemented (2026-10-08) with `routes::scoped_tenant` as the one rule: with a
resolved tenant, an empty request tenant becomes the resolved one, a different
one is 403; without a tenant context (identity not configured) the request
value is used as before. Details per family:

- Webhooks: list/create/get/rotate/disable. The signed trigger ingress is
  unchanged (it authenticates by HMAC, not by tenant).
- Connectors: list is filtered to the acting tenant; get and
  health/fail/restart resolve only the acting tenant's connectors (404
  otherwise); register forces the acting tenant and answers 409 when the id
  belongs to another tenant (the supervisor used to move it to the caller);
  ingress takes the acting tenant.
- `PATCH /v1/principals/{id}` uses the new
  `SQLiteStore::list_principal_memberships`.

Token management (G14): rotate, revoke and tenant-grant changes act only on
the caller's own tokens or tokens of a principal whose only active membership
is the caller's tenant; anything else is 404 before any side effect. A
manager's token list for another principal is empty unless that principal is
an active member of the caller's tenant. `PATCH /v1/principals/{id}` shares
the same membership helper.

Chat MCP tools (G15): a turn is offered only servers owned by its tenant
(`kura_mcp::tools_for_tenant`), plus servers with no owner — those predate
2026-09-02, when servers began recording their tenant, so dropping them would
silently take tools away from existing single-mode daemons. Shared mode stops
offering unowned servers (§4.3: MCP is operator-only there).

Known limits:

- A connector persisted with an empty tenant id is invisible to tenant-scoped
  callers and can still be claimed by the first tenant that re-registers it.
  Connectors are operator-only in shared mode (§4.3), so this matters only for
  single-mode daemons with several tenants.
- The audit gate is still file-level: one correct handler makes the whole file
  pass. The per-family tests above are what pin the behavior.

### 4.6 Failure modes and rollback

- Unset `KURA_HOSTING` ⇒ single mode, byte-for-byte today's routing. The only
  behavior change in single mode is §4.5 (clients sending another tenant's id
  now get 403 instead of that tenant's data).
- A route missing from the allowlist shows up as a 403 for tenants, never as a
  leak; fix by adding it to the table with a reason.
- Rollback: unset `KURA_HOSTING`, or revert the commits; no schema change.

## 5. Open questions

1. M2: how the gateway authenticates as a user — per-user token minted by the
   provisioning API and stored encrypted by the gateway (preferred: the daemon
   keeps full authorization, a leaked gateway key is not a skeleton key) vs a
   gateway-signed identity header.
2. M3: Docker-per-run first, or go straight to gVisor (`runsc`) on the cell host.
3. Per-tenant skills/connectors (G10): which ones tenants actually need.
