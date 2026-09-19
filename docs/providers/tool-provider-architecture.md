# Tool Provider Architecture

Design date: 2026-09-18. Planning record for Stage 9 of
[`../harness/agent-deepening-program.md`](../harness/agent-deepening-program.md):
how built-in tools (web search, image generation, video generation, browser)
are configured, how their credentials are stored, and how a provider is added
without changing the daemon.

This document deliberately answers the *provider-agnostic* questions first.
Which vendor serves web search is a later, smaller decision — it becomes
filling in a profile once this is in place.

## The problem this solves

Kura is a tool **host** with no batteries: the MCP plane, the skill
`execution.*` escape hatch, and the runtime's `LocalTool`/`McpTool`/`DomainTool`
call ledger all exist, but nothing ships that can search the web, generate an
image, or drive a real browser. Adding those means answering, once:

- where a tool provider's configuration lives,
- where its credential lives (and where it must *not* live),
- how a second provider for the same capability is added,
- what stops a tool from becoming an unbounded spend or exfiltration surface.

## What we reuse

The LLM provider plane already solved the analogous problem
([`provider-architecture.md`](provider-architecture.md)) with a three-plane
split — family / auth mode / profile. Tool providers mirror it. The tenant
secret plane already solved credential storage. Phase 3 of the plugin
architecture already solved out-of-process providers. None of that is rebuilt.

## The four planes

The LLM split has three planes because an LLM's *capability* is implied by the
family. A tool's is not — "search the web" and "generate an image" are
different agent-facing contracts served by overlapping vendors. So there are
four.

### 1. Tool capability — the agent-facing contract

What the model sees and calls. Provider-agnostic by construction: `web.search`,
`image.generate`, `video.generate`, `browser.session`.

A capability owns the request shape, the result shape, the error classes, and
the citation/artifact rules. **The model never names a provider.** Swapping
Brave for Tavily changes no prompt, no skill, and no transcript shape.

This is the seam in the plugin sense: one Service Definition per capability.

### 2. Tool family — the upstream API shape

How one vendor's API is encoded and decoded: `brave_search_v1`,
`tavily_v1`, `openai_images_v1`, `cdp_chromium`. Answers request encoding,
result decoding, error mapping, and which capability it serves.

A family serves exactly one capability. A vendor offering both search and
images is two families, because they fail, price, and authenticate
independently.

**Recorded 2026-09-19 — the main path is `mcp_backed`, not vendor families.**
The operator's instruction was: *users configure their own providers; Kura
does not pick a vendor for them*. Provider-hosted search (the way Codex CLI
answers search through its own server) was rejected because it means the
vendor's server, not the operator's, does the work. So the first shipped
family is `mcp_backed`: a profile names an MCP server the user has already
installed (`mcpServerId`) and one of its tools (`mcpToolName`); the model is
offered that tool's real `inputSchema` under the capability name
(`web.search`, `image.generate`, `video.generate`), and calls are authorized
under the MCP plane's exposure rules for the `chat` runtime surface before
they run. Credentials, vendor URLs and result shapes belong to the MCP
server's own configuration, which is already secret-ref'd and
egress-screened; the profile carries none of them. `builtin_stub` remains
for proving the mechanism with no vendor. Vendor-specific families
(`brave_search_v1`, …) stay possible as later additions but are not
required for any capability.

### 3. Auth mode — how access is obtained

Reuses the existing vocabulary (`crates/modeling/providers/src/types.rs`):
`none`, `api_key`, `local_cli_bridge`, plus `oauth_device` when a tool vendor
needs it. This layer answers acquisition, refresh, and — critically — **who
owns the credential**, which is never the profile (see below).

### 4. Tool profile — the configured instance

The operator-facing object: capability + family + auth mode + endpoint +
credential *reference* + limits + capability flags. Profiles are what the
operator lists, selects, and checks.

## Decision: the registry is data, not struct fields

`LlmConfig` carries one struct field per provider —
`openai_compatible`, `claude`, `codex`
(`crates/foundation/config/src/types.rs:96`). Adding a fourth LLM provider
means editing a struct and recompiling.

**Tool profiles do not follow that shape.** They are a keyed collection, so a
new provider is data. This is a deliberate divergence from the precedent, not
an oversight: the LLM plane has three providers and a stable roadmap; the tool
plane is open-ended by design.

Profiles carry a `source`, mirroring `providers::Source`:

| Source | Where it lives | Scope | For |
|---|---|---|---|
| `builtin` | compiled in | daemon | A no-network stub per capability, so tests and the default assembly never depend on a vendor |
| `config` | the config file, a keyed map | daemon | Single-user hosts; one place, no API calls, survives a wiped database |
| `managed` | `tool_profiles` table | **per tenant** | Multi-user deployments; CRUD'd over the API with an audit trail |

Resolution order for a capability: an explicitly selected profile, else the
tenant's default `managed` profile, else `config`, else `builtin`. A
capability with no usable profile reports `unconfigured` rather than failing at
call time — the agent should be told the tool is absent, not handed an error
mid-turn.

## Decision: credentials never live in the profile

The profile stores a **`secret_ref`** — a string — exactly as
`providers::Profile` does today (`secret_ref` + `secret_configured`, no value
field). Resolution goes through the tenant secret plane:

```rust
secrets.resolve(kura_secrets::ResolveInput { tenant_id, secret_ref }).await
```

That is the same call `crates/surface/app/src/adapters.rs:550` already makes
for MCP server secrets. Using it buys, at no cost:

- **per-tenant isolation** — two tenants' keys for the same vendor are two
  secrets, never one shared value;
- **rotation** with a version history;
- **an audit trail** (`ResourceKind::TenantSecret`, `AuditAction::SecretCreate`
  / `SecretUpdate` / rotate);
- **redaction** — `redact_secret_refs` / `REDACTED_VALUE` keep values out of
  projections and logs by default rather than by remembering to.

Hard rules, stated so they can be tested:

1. No API response ever contains a credential value. The profile projection
   carries `secretConfigured: bool` and the ref; a test asserts the serialized
   profile contains no secret material.
2. No credential is written to the config file by the daemon. A `config`-source
   profile may name an **env var** or a secret ref, never an inline value. (The
   existing `OpenAiCompatibleProviderConfig` has both `api_key` and
   `api_key_env`; tool profiles keep only the indirection.)
3. A profile whose `secret_ref` does not resolve is `unconfigured`, not
   `error`. An unset key is an operator to-do, not an incident.

## Decision: egress and spend are part of the profile

Every tool provider is an egress point and a cost centre. Both are declared,
not assumed.

**Egress.** The profile declares its `base_url`. The SSRF blocklist (Stage 7.3)
applies to it — and, separately, to anything fetched *because* of a result.
This is the sharp edge: **web-search results are attacker-influenced URLs.** A
search tool that fetches result pages is a confused deputy unless result
fetching goes through the same blocklist as any other outbound fetch. The
capability contract therefore returns URLs and snippets; fetching one is a
separate, separately-gated action.

**Spend.** Each profile carries a quota binding into the billing plane. The
capability seam reserves before the call and commits after, the way
`WebhookQuotaGateImpl` already does for webhook triggers. This is the concrete
form of the Stage 4.1 dependency: concurrency and tools both multiply spend,
and the bound lands before either.

## Decision: third-party providers are plugins, not daemon changes

Phase 3 slice 2 already serves seams over the external-plugin channel
(`seam:<name>:<op>`, `schemas/plugin/plugin-manifest.schema.json`). A
third-party tool provider is a manifest declaring
`seams: ["tools.web.search"]` — no daemon change, no recompile, sandboxed and
supervised like any other external plugin.

In-process families ship for the common cases; the seam is what keeps that
from being a ceiling.

Trust note, inherited and unchanged: installing an external plugin is code
execution with daemon privileges. Tool plugins are the same trust class as any
other, and ride the catalog's trust tiers.

## Storage

One new tenant-partitioned table, following `memory_assets`: indexed
projections plus the document JSON.

```sql
CREATE TABLE tool_profiles (
    profile_id   TEXT PRIMARY KEY,
    tenant_id    TEXT,              -- NULL = the single-user assembly's own
    capability   TEXT NOT NULL,     -- web.search | image.generate | ...
    family       TEXT NOT NULL,     -- brave_search_v1 | openai_images_v1 | ...
    auth_mode    TEXT NOT NULL,
    enabled      INTEGER NOT NULL,
    is_default   INTEGER NOT NULL,  -- per (tenant, capability)
    created_at   TEXT NOT NULL,
    updated_at   TEXT NOT NULL,
    document_json TEXT NOT NULL
);
CREATE UNIQUE INDEX uq_tool_profiles_default
    ON tool_profiles(tenant_id, capability) WHERE is_default = 1;
```

`tenant_id IS NULL` means the single-user assembly's own rows — the convention
established in Stage 2.2 and shared by every tenant-scoped query in the
workspace. The unique partial index makes "two defaults for one capability"
unrepresentable rather than a validation rule someone forgets.

**No credential column, by construction.** The document holds `secretRef`; the
value lives only in the secret plane.

## API surface

| Route | Purpose |
|---|---|
| `GET /v1/tools/capabilities` | What the agent could do here, and whether each is configured |
| `GET /v1/tools/profiles` | Tenant's profiles (never a credential value) |
| `POST /v1/tools/profiles` | Create a `managed` profile |
| `PATCH /v1/tools/profiles/{id}` | Edit limits/endpoint/default; **not** the secret |
| `POST /v1/tools/profiles/{id}/check` | Preflight: auth / transport / upstream / quota, mirroring the provider check error classes |
| `DELETE /v1/tools/profiles/{id}` | Remove |

Credentials are written through the existing tenant-secret routes, not these.
That is the point: there is one way to store a secret in this system.

`/check` matters more than it looks. Without it, a mistyped key surfaces as a
failed turn in front of the user; with it, it surfaces where the operator is
already standing.

## What this makes cheap

Adding web search becomes: one family implementation, one profile, one secret.
Adding a second search vendor becomes: one family implementation. Switching
vendors becomes: changing which profile is default. None of those touch the
agent, the prompt, or the transcript.

## Sequencing

1. This design (done).
2. Capability seam + `tool_profiles` storage + profile CRUD + `/check`, with
   the `builtin` no-network stub as the first family — provable end to end with
   no vendor account.
3. Stage 7.3 (SSRF blocklist) and Stage 4.1 (quota bound) — **both before any
   real vendor family**, per the rules above.
4. The first real family (web search), then image generation, then the browser
   driver. **Done 2026-09-19 as one family**: `mcp_backed` serves search,
   image and video alike (Stages 9.2/9.3/9.5), because with tool calling in
   the chat loop (9.0) and the user's own MCP server as the vendor, the three
   differ only in capability name and in what the operator's server returns.

Step 2 is deliberately shaped to be verifiable without credentials: the whole
mechanism — storage, resolution, redaction, check, quota — is exercised by the
stub. A vendor key then only exercises the vendor.

## Recorded decision: cross-channel sends are default-deny

Recorded 2026-09-19 (Stage 7.4). OpenClaw's Unreleased changelog flips
cross-provider messaging to **default-allow** (`allowAcrossProviders`
omitted ⇒ permitted). Kura does not follow, and this section exists so the
question is not reopened by accident.

**Where Kura stands today, verified:**

- The conversational reply path (`crates/channels/im`) replies only to the
  inbound binding — `connector_id` and `channel_id` are copied from the
  inbound message (`im/src/lib.rs:1659`, `:2485`). There is no way for the
  model to redirect a reply.
- Proactive delivery (`crates/domains/delivery`) is **operator-configured**:
  `DeliveryPreference.preferred_targets_by_class` maps a result class to a
  target the operator created. The model never names a target.
- The live chat path has no tool calling at all (see Stage 9.0), so there is
  currently no model-directed send of any kind to deny.

**The decision:** when a `message.send` capability is added to this plane
(there is deliberately none in `Capability` yet), it must be
**default-deny across connector bindings**. A send may reach a target whose
`ConnectorBinding` matches the originating conversation's connector unless a
delivery preference the operator wrote explicitly names another target. The
reasoning is the one this document already applies to search results: a
model-controlled destination plus an open default turns a prompt injection
into an outbound send capability. Default-allow trades that for convenience;
we do not.

## Open questions, recorded rather than assumed

- **Per-agent profile selection.** Profiles are per tenant. Whether one agent
  profile can pin a different search provider than another is a binding-plane
  question (`workspace-capability-binding.md`) and is not answered here.
- **Result caching.** Search results are cacheable and expensive; image results
  are artifacts. The artifact plane (`crates/domains/artifacts`) is the obvious
  home for the latter. Caching policy for the former is deferred until there is
  a real vendor's rate limit to design against.
- **Cost attribution granularity.** Per-call is assumed. Per-token or
  per-image-pixel pricing may need a richer reservation unit than the
  webhook precedent provides.
