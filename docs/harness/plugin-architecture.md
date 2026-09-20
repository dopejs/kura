# Agent Pluginization

Decision date: 2026-08-17 (operator). Reference architecture:
[deepseek-ai/deepseek-harness](https://github.com/deepseek-ai/deepseek-harness)
("Everything is a Plugin"). This document is the planning record for the
program — features are now planned directly in design docs like this one and
implemented; the numbered-spec flow (`docs/specs/NNN-*`) is retired for new
work and kept as history.

## Why

Before building further agent capabilities (session/context management,
retrieval, skills), the daemon is restructured so that every non-kernel
subsystem is a **plugin**: a named unit with declared dependencies, resolved
enablement, and inspectable assembly. The payoff:

- **Session management becomes a plugin decision.** A pure personal agent
  runs a long-session plugin (when to compress, what stays resident, what is
  written to memory); IM channels that support threads run a
  context-per-thread plugin. The daemon core stops hard-coding one context
  policy.
- **Existing features become default plugins** — swappable, disableable,
  and eventually replaceable by out-of-process providers.
- **The assembly is a config artifact**, not compiled folklore: what runs,
  what is disabled, and why is dumped from `/v1/plugins`.

## What we adopt from deepseek-harness

1. **Seam model** — a capability is a Service Definition (interface) + a
   Provider (implementation) + Consumers, designed together. Swapping the
   provider swaps the behavior for every consumer at once.
2. **Profile-layered assembly** — a running daemon is a resolved tree of
   plugin entries; any entry can be disabled/configured by id; the resolved
   result is dumpable.
3. **Three event domains** — persistent facts (our event bus + store),
   waterfall interception hooks (new: `HookBus`, handlers may mutate the
   payload or halt the action), and capability events.
4. **"Model-visible = logged"** — everything that reaches a model must be
   reconstructable from the session log (lands with the hookable agent loop
   in phase 2).

## Deliberate deviation

deepseek-harness has no privileged kernel; we keep one. The trust boundary —
**store, event bus, identity, auth, policy, secrets, audit** — is built
directly in `App::with_profile` and cannot be disabled or replaced by a
plugin. Security posture outranks composability; this is a fixed decision.

## The two-tier model (Rust reality)

- **Tier 1 — builtin plugins** (shipped, phase 1): compiled-in plugins
  (`crates/surface/app/src/plugins.rs`) registered with the kernel crate
  `kura-plugin` (`crates/foundation/plugin`). No dynamic loading (dylib ABI
  hazards are not worth taking for the first release).
- **Tier 2 — out-of-process providers** (phase 3): any seam served remotely
  over the proven planes — adapter RPC, supervised capability processes,
  MCP — installable at runtime through the catalog (`kind=plugin`).
  Language-agnostic, sandboxed, supervised. Consumers cannot tell the tiers
  apart.

## Mechanisms (phase 1, shipped 2026-08-17)

- **`kura-plugin` kernel crate**: `PluginDescriptor` (id, summary,
  provides, requires), `resolve()` (explicit + transitive disable, warnings
  for unknown ids — nothing silently dropped), `PluginProfile`
  (`<data_dir>/plugins.json`; missing file = default profile, malformed file
  fails boot loudly), `SeamMap` (typed assembly-time registry), `HookBus`
  (waterfall interception: ordered handlers, mutate-or-halt).
- **Builtin plugin set**: llm, skills, sandbox, mcp, integrations, calendar,
  mail, providers, connectors, capabilities, chat, billing, activation,
  computer-use, delivery, scheduler, reminders, routines, memory, triage,
  webhooks, catalog, exec-profiles, evidence, evaluation, live-validation,
  setup-wizard, channel-{discord,telegram,slack,matrix}. Declared
  `requires` edges keep fail-closed gates honest (e.g. disabling `billing`
  transitively disables `webhooks` — the quota gate can never run
  half-wired).
- **Disable semantics**: a disabled plugin's `AppState` field stays `None`;
  the API layer already answers not-wired. Channel plugins additionally gate
  their serve-time runtime construction (profile wins over the connector
  config flag; no network or credential is touched).
- **Introspection**: `GET /v1/plugins` returns the boot-time
  `AssemblyReport` (build order, enablement, reasons, warnings). Contracts:
  `schemas/plugin/plugin-profile.schema.json`,
  `schemas/api/plugins-report.schema.json`; SDK `listPlugins()`.
- **Profile management** (added with the config surface): `GET/PUT
  /v1/plugins/profile` read and atomically replace `plugins.json` (boot-time
  effect; responses carry `restartRequired: true`); SDK
  `getPluginProfile()`/`updatePluginProfile()`; the operator shell has a
  Plugins section (`/plugins`) — assembly list with enable/disable toggles,
  hook registrations, profile warnings, and the restart notice.

## Phase 2 — hookable agent loop (shipped 2026-08-17)

The chat pipeline (query and stream) now runs three kernel hook points:

- `chat/turn-start` — before prompt assembly; hooks may rewrite the query
  or halt (veto). Payload: `{tenantId, threadId, query, sourceKind}`.
- `chat/pre-dispatch` — after full context assembly (skills, profile,
  continuity), before the dispatch is prepared and persisted; hooks may
  rewrite provider/model/messages or halt. **This ordering is the
  "model-visible = logged" invariant**: the dispatch record is created
  after the hooks run, so what is persisted is byte-identical to what the
  provider receives (covered by tests at the service layer and end-to-end
  through the real assembly).
- `chat/turn-end` — after the dispatch settled and continuity was
  persisted; observational (a halt only stops later handlers).
- `chat/tool-call` (Stage 9.0, 2026-09-19) — before a tool the model asked
  for is executed. Payload: `{tenantId, threadId, agentProfileId, callId,
  name, arguments}`; hooks may rewrite `arguments` or halt. A veto is reported to
  the model as a tool error (and as `chat.hook.vetoed`), never silently
  dropped, so the model can answer without the tool.

Tool calling (Stage 9.0, merged 2026-09-20): a turn is the agent loop
`kura-core` already had, driven over the dispatcher — `chat/src/round.rs`
makes one dispatcher round a `ModelProvider`, so every round is prepared,
hooked, persisted (`tools_json`, `tool_calls_json`) and evented exactly
like the single dispatch a turn used to be. The assembly's `ToolSource`
(`crates/surface/app/src/tool_host.rs`) resolves the registry per turn and
per tenant: the connected MCP servers' tools, `memory.lookup`, and one tool
per Ready tool profile; each is wrapped so a call runs `chat/tool-call`,
is recorded as a `chat.tool.called` event and counted. The loop is capped
at `toolMaxRounds` (`plugins.json` → `entries.chat.config`, default 16);
a turn that hits the cap is failed rather than answered with half a result.

A veto surfaces as `ChatError::HookVetoed` → HTTP 403 and is recorded as a
`chat.hook.vetoed` event. `GET /v1/plugins` reports every hook registration
(`hooks: [{point, pluginId}]`). Context already derives from persisted
records (continuity turns render into messages per dispatch — the
`derive_messages` shape); generalizing that log into a session-event model
is folded into the session-strategy plugin slice, which attaches at
`chat/pre-dispatch`.

### Session-strategy plugin (first slice shipped 2026-08-17)

The `session-strategy` builtin (policy engine: `crates/domains/session`)
attaches at `chat/pre-dispatch` and shapes the assembled window
deterministically — before the dispatch is persisted, so the shaped window
is exactly what is logged and what the model sees. Mechanism:
frame-preserving elision — system messages (persona, skills, safety) are
the frame and never elided; the most recent `keepRecent` non-system
messages are always kept; over-budget oldest history is replaced by one
marker line pointing at thread continuity and the memory plane. Eviction
is safe by construction: every turn is captured to L0 at settle
independently of the window.

Two strategies share the mechanism and differ by budget, keyed off the
turn's `sourceKind`: **personal** (long-session default, 48k chars) and
**thread** (`sourceKind=channel`, tight 16k chars — one context per
thread; thread scoping itself comes from the continuity plane). Operator
config via the profile entry
(`entries.session-strategy.config.{personalBudgetChars,threadBudgetChars,keepRecent}`);
a malformed config fails the boot loudly. Disabling the plugin restores
unshaped windows.

Compression-to-memory (second slice, 2026-08-17): eviction never
plain-drops content. When the turn has a thread, the elided span is
captured as an L0 ref (thread source link, bounded excerpt) through the
governed memory pipeline — the async consolidator distills it into L1/L2
off the reply path with the write policy intact — and the elision marker
cites the captured asset (`…; elided span captured as Memory[l0_ref …]`)
so the model can drill back. Threadless turns keep their spans reachable
through dispatch records only.

Later slices: session frame objects (explicit goal/constraint records)
and channel-native thread segmentation policies — planned as Stage 5 of
[`agent-deepening-program.md`](agent-deepening-program.md).

### Context plugin (first slice shipped 2026-08-17)

The `context` builtin (policy engine: `crates/domains/context`) is the
default context manager, and other plugins modify its result by design —
it runs first in the `chat/pre-dispatch` waterfall, then
`session-strategy` shapes the window, then any later builtin/external
hook may rewrite or veto.

What it does, deterministically: injects the tenant's memory bootstrap —
Ready L3 personas first, then Ready L2 scenarios, newest first
(private/team visibility; restricted/agent wait for binding-aware
loadouts) — as system-frame messages under
`entries.context.config.memoryBudgetChars` (default 4000, fail-loud
validation). Every injected message carries its citation inline
(`Memory[l3 mem_xxx] title: content`): recalled memory is evidence, never
bare text. L1 atoms are never bulk-injected (drill-down/retrieval only,
per the design root). Every pass emits a `context.assembled` event whose
`AssemblyRecord` lists inclusions (asset, layer, chars) and exclusions
with reasons (`over_budget`/`empty_content`/`visibility`) — nothing
enters or misses the context silently, and an engineer can reconstruct
what memory the model saw for any dispatch. Because injection happens
before the dispatch is prepared, the persisted dispatch record carries
the bootstrap verbatim (model-visible = logged holds).

Query-time retrieval (second slice, 2026-08-17): the turn's last user
message recalls Ready L1 atoms through BM25 + recency rankers fused with
reciprocal-rank fusion (k=60). Atoms with no lexical overlap are never
recalled (recency alone cannot pull unrelated memory in); the top-8 fused
candidates inject under `retrievalBudgetChars` (default 2000) as
`Memory[l1 …] (recalled): …` system messages, merged into the same
AssemblyRecord with `source: retrieval`. The vector ranker (third slice, 2026-08-17) joins the fusion through the
`Embedder` seam. Default provider: the deterministic hashed
character-trigram embedder (256-dim FNV feature hashing, L2-normalized
cosine) — a character-level vector space that recalls what the word-based
BM25 tokenizer misses, CJK above all (`请用中文回复` matches
`中文回复偏好` on trigrams with zero shared word tokens). Candidacy
widens to BM25>0 OR cosine ≥ 0.25; below the threshold, hash noise never
leaks unrelated memory into context. A neural embedding provider replaces
the default through the seam without touching the fusion.

Symbolic compression (fourth slice, 2026-08-17, `92191d5`): non-frame
messages over `refThresholdChars` (default 8000) externalize at
`chat/pre-dispatch` — the full content persists as an L0 ref and the window
keeps a 200-char preview plus its `Memory[l0_ref …]` citation. Binding-aware
loadouts (fifth slice, 2026-08-17, `81a0529`): `Visibility::Agent` assets are
admitted only when their bindings contain the turn's active agent profile id,
fail closed and recorded in the AssemblyRecord.

Still open (planned in
[`agent-deepening-program.md`](agent-deepening-program.md)): a neural
embedding provider for the seam, a model-facing lookup tool for following
`Memory[…]` citations mid-turn (Stage 1.5), retrieval over L2/L3 rather than
L1 atoms only (Stage 1.4), and a dedicated assembly-record read API
(Stage 1.6).

## Behavioral pluginization (shipped 2026-08-17)

Assembly-level pluginization (phase 1) made subsystems disableable; this
slice moved their **behavior** onto plugin mechanisms:

- **Lifecycle seam**: plugins register `on_start`/`on_close` callbacks
  during build; `App::serve`/`App::close` run the registries instead of
  hardcoding manager names. Scheduler and reminders own their start+close,
  sandbox its close, and memory its 60s consolidation/retention tick.
- **Memory capture is a hook**: the memory plugin registers a
  `chat/turn-end` observer that L0-captures the settled turn (thread +
  dispatch + source-message links) and schedules due consolidation off
  the reply path — the hardcoded API-layer call is gone. Coverage notes:
  stream turns are captured (previously only non-stream queries were),
  and channel-source turns are captured here too — gateway-driven IM
  traffic reaches chat without touching the HTTP ingress pipeline, so
  the hook is its only capture point. HTTP-pipeline messages that also
  dispatch chat produce both an `inbound_message` and a `chat_turn` L0;
  accepted — L0s are excerpt evidence and consolidation extracts through
  citations. Every capture path (chat hook, ingress, workflow, session
  eviction) now honors the due turn-trigger by running consolidation off
  the request path.
- **Connector runtimes stay kernel-hosted for now** (recorded decision):
  the four channel runtime builders live in `App::serve` because the
  telegram/matrix transports are `!Send` and run on raw-pointer threads —
  moving that gymnastics behind a plugin-owned lifecycle callback adds
  risk without changing behavior. Channel plugins own identity,
  enablement, and gating; full runtime ownership lands with the seam-RPC
  slice, where a channel can be served out of process entirely.

## Phase 3 — out-of-process plugin providers (slice 1 shipped 2026-08-17)

Shipped: external plugins as supervised stdio processes attaching to the
hook plane.

- **Manifest** (`schemas/plugin/plugin-manifest.schema.json`): id/version/
  summary/provides/requires, `hooks: [{point, onError}]`, `entry: {kind:
  "process", command, args, timeoutMs}`.
- **Discovery**: `<data_dir>/plugins/<dir>/manifest.json` at boot.
  Third-party content never bricks the boot — malformed manifests are
  skipped with report warnings (unlike the operator-owned profile, which
  fails loudly). Externals resolve through the same profile/`requires`
  machinery as builtins and appear in `/v1/plugins` as `source:
  "external"`; duplicate ids lose to builtins.
- **Process host**: lazy spawn on first hook call, line-JSON protocol
  (request `{point, payload}` → response `{outcome, reason?, payload?}`,
  response payload replaces the hook payload), per-call timeout, one
  respawn per call for a dead child, killed on daemon close. Failures
  follow the hook's `onError` policy: `continue` (availability first,
  default) or `veto` (fail closed — policy plugins). An external process
  can therefore rewrite context or veto turns exactly like a builtin hook
  (proven end to end: manifest on disk → assembly → chat turn rewritten by
  the child process).
- **Catalog**: `kind=plugin` accepted by the install catalog.

Trust note: manifests execute commands from the data dir — installing an
external plugin is code execution with daemon privileges, same trust class
as installing a capability. Distribution/verification flows ride the
catalog trust tiers.

Seam dispatch (slice 2, 2026-08-17): manifests declare `seams`; calls
ride the same line-JSON channel as hooks (point `seam:<name>:<op>`). The
first served seam is `context.embedder`: install an external embedding
process (neural or otherwise) and the vector ranker in both the context
plugin and `/v1/retrieval/queries` switches to it — no daemon change, no
fusion change. Failures fall back to the deterministic in-process
embedder (availability first, logged). First declaration wins; duplicate
providers warn and are ignored (deterministic assembly).

Remaining for later slices: more seams over the process protocol (the
memory consolidator, whole channel runtimes) and catalog-driven
install/update lifecycle placing plugins into `<data_dir>/plugins/`.

## Sequencing

Pluginization precedes the remaining capability program (operator decision):
session/context management ships **as plugins** after phase 2; retrieval,
agent-managed skills, and audited self-improvement follow. The operator-run
release work (Roadmap 76 soaks, Roadmap 77 launch gate) continues in
parallel and is unaffected — with the default profile the assembly is
behavior-identical to the pre-plugin daemon.

## Verification (phase 1)

- `kura-plugin` unit tests: resolution (explicit/transitive/unknown-id),
  profile load, seam map, hook waterfall (mutate + halt ordering).
- `kura-app` tests: default profile ⇒ all builtins enabled and every seam
  adapter wired (existing wiring tests unchanged); leaf disable ⇒ manager
  `None` + reason recorded; `billing` disable ⇒ dependents transitively
  disabled; `plugins.json` picked up by `App::new`; channel plugin disable
  ⇒ connector runtime skipped despite the config flag.
- Contract tests: profile + report schemas, including a round-trip of the
  actual `resolve()` output through the report schema.

Rollback: revert to the pre-pluginization assembly commit; `plugins.json`
is additive and ignored by older daemons.
