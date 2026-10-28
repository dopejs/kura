# Shared hosting: one daemon, many tenants

> Status: M1 implemented (2026-10-08): boundary fixes (§4.5), shared mode
> (§4.1-4.4). M2 in progress: provisioning and per-user tokens implemented
> 2026-10-28 (§6); BYOK and gateway sign-up next. Owner: daemon.
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

`Hosting::is_platform_operator(ctx)` = shared mode ⇒ `ctx.tenant_id ==
ten_local && role == Owner`; single mode ⇒ always (handlers keep their own
guards).

As built, `require_daemon_global_operator` is unchanged (it takes no state).
In shared mode every route it guards (plugins, improvement, capabilities,
sandbox reload, config) is outside the tenant allowlist, so the route policy
(§4.3) has already admitted only the platform operator by the time the guard
runs.

### 4.3 Route policy (deny by default)

A layer inside `protected()` (it needs the resolved tenant) that runs only in
shared mode. It looks up `(method, MatchedPath)` in one table:

- **tenant**: any member, subject to the handler's own checks;
- everything not listed: **operator only** (403 `hosting_operator_only`).

A route added later is operator-only until someone decides otherwise, which is
the safe failure. A request with no resolved tenant is not the operator either.
A test asserts every table entry names a real route, so the table cannot
silently drift.

As built: `kura_api::hosting::{TENANT_ROUTES, shared_route_policy}`, a
`route_layer` added just before `protected()`. Tests:
`every_tenant_route_reaches_its_handler_in_shared_mode` (every entry, as a
non-operator owner, never gets the hosting 403, a fallback 404 or a 405) and
`everything_else_is_reserved_for_the_platform_operator` (sandbox exec,
provider/model-role writes, MCP, tenant secrets, tenant creation,
invitations, principal PATCH, raw LLM dispatch, run creation, routines,
memory consolidation, metrics: 403 for another tenant's owner, the operator
tenant's admin and a tenant-less request; reachable for the operator and in
single mode). In shared mode chat turns are also no longer offered unowned
MCP servers (G15).

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

As built (`hosting::loopback_pairing`): a request carrying `Forwarded`,
`X-Forwarded-For` or `X-Real-IP` is refused even from loopback, because a
reverse proxy on the same host connects from loopback too; a missing peer
address is refused (fail closed). `KURA_HOSTING` other than unset / `single` /
`shared` stops startup, and `shared` outside `KURA_ENV=hosted` stops startup
(`AppError::Hosting`).

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

Decisions on 1 and 2 recorded 2026-10-28 in
[agent-runtime-program.md](agent-runtime-program.md): per-user token minted by
the provisioning API; Docker-per-run first, gVisor after measurement.

1. M2: how the gateway authenticates as a user — per-user token minted by the
   provisioning API and stored encrypted by the gateway (preferred: the daemon
   keeps full authorization, a leaked gateway key is not a skeleton key) vs a
   gateway-signed identity header.
2. M3: Docker-per-run first, or go straight to gVisor (`runsc`) on the cell host.
3. Per-tenant skills/connectors (G10): which ones tenants actually need.

## 6. M2 design — provisioning (agent-runtime-program 1.1, 1.2)

### 6.1 Route

`POST /v1/provisioning/users` — the only way a stranger gets an identity on a
shared daemon. Pairing stays loopback-only and operator-only (§4.4).

```json
{ "externalSubject": "github:583231", "displayName": "Ada", "tokenLabel": "gateway", "expiresAt": "2027-01-01T00:00:00Z" }
```

- `externalSubject` (required, 1–256 chars after trimming): the gateway's
  stable id for the user, conventionally `<idp>:<id>`. It is the idempotency
  key.
- `displayName` (optional): used for the principal and the personal tenant
  ("Ada's workspace"); defaults to "User".
- `tokenLabel` (optional, default `gateway`) and `expiresAt` (optional) shape
  the token minted by this call.

Response — `201` when the identity was created, `200` when it already existed:

```json
{ "created": true, "principal": {…}, "tenant": {…}, "membership": {…},
  "token": {…}, "accessToken": "kura_…", "grants": [{…}] }
```

### 6.2 Who may call it

- **Shared mode only.** On a single daemon the route answers `409
  provisioning_requires_shared_hosting`: one owner has nobody to provision,
  and the pilot tier (one container per user) pairs instead.
- **Platform operator only** (`Hosting::is_platform_operator`). The route is
  not in `TENANT_ROUTES`, so `shared_route_policy` already refuses everyone
  else; the handler checks again so the guarantee does not depend on the
  allowlist staying correct.

### 6.3 Identity shape and idempotency

Ids are derived from the subject rather than stored beside it, so
idempotency needs no schema change and no lookup table:

```
h = hex(sha256("kura-provisioning:v1:" + externalSubject))[..16]
principal = prn_<h>   tenant = ten_<h>   membership = mem_<h>
```

Same shape as random ids (`random_id(8)` is 16 hex chars); a collision with a
random id is a 64-bit event. The subject itself is not stored, so a database
leak does not hand out IdP account ids; the operator finds a user by
recomputing the hash. Consequence: a subject cannot be renamed — account
linking at the gateway must keep the original subject.

- Principal: `kind = user`, `defaultTenantId = ten_<h>`.
- Tenant: `kind = personal`, owner = the principal.
- Membership: `role = owner`, active. The user owns their personal tenant;
  shared mode keeps "owner" from meaning "may change the daemon" (§4.2).
- Each row is created only if missing, in the order tenant → principal →
  membership. The writes are not one transaction: a crash midway leaves a
  prefix, and the next call for the same subject completes it.
- A principal or tenant that exists but is not `active` (disabled, removed)
  is **not** revived: `409 provisioning_identity_inactive`. Re-enabling a
  user is an operator decision, not a side effect of a sign-in retry.
- The operator tenant can never be the target: derived ids cannot equal it,
  and the handler refuses if they ever did.

### 6.4 The token (decision 2026-10-28: per-user token held by the gateway)

Every call mints a **new** token for the principal, granted to the personal
tenant only (`isDefault = true`). The plaintext is in this response and
nowhere else; the gateway stores it encrypted and presents it as the user's
bearer token. The daemon therefore authorizes each request as that user, and
a leaked gateway credential reaches only the users whose tokens leaked with
it. Retrying accumulates tokens; the gateway revokes the ones it discards
(`POST /v1/auth/tokens/{id}/revoke` as the user). A token cap per principal
belongs to M4.

### 6.5 Audit

`tenant.user_provisioned` (on creation, tenant = the new tenant, principal =
the operator) and `tenant.token_issued` (every call). Both are written before
the rows they describe, and failing to write them fails the call, as with
every other identity mutation.

### 6.6 Failure modes and rollback

- Operator token leaked → attacker can mint users and tokens for any subject,
  including existing users. Same blast radius as the operator token today;
  mitigation is the operator token never leaving the gateway host.
- Rollback: revert the route; provisioned rows are ordinary identity rows and
  keep working with the tokens already issued.
