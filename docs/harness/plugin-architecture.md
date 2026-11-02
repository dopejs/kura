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

Parallel tool calls (agent-runtime-program.md 2.6): within one round,
consecutive calls that are *parallel-safe* run concurrently, at most
`toolMaxParallel` at once (`entries.chat.config`, default 4; `1` is
strictly serial and is the rollback). Any other call is a barrier that
runs alone, in the order the model asked, after everything before it.
Results enter the history in call order whatever finished first; each
call keeps its own `tool_call` span, closed when that call ends.
Parallel-safe today: `memory.lookup`, and MCP tools whose server declares
`annotations.readOnlyHint: true` **and** whose exposure rule on the
surface is a plain allow (`Manager::runs_without_approval`, a state read
that raises nothing). A read-only tool behind `approval_required` stays
serial, so a batch never starts beside a call waiting for a person.
Tool-profile tools (`web.search`, …) stay serial because they reserve
quota inside the call. Blocking work (MCP `call_tool`, recall) runs on
the blocking pool so calls actually overlap; the tenant context is
carried into it.

Turn resume (agent-runtime-program.md 2.7): a turn that calls tools keeps a
checkpoint (`agent_turn_checkpoints`, schema v7) of every round's calls and
each call's state — planned, started, settled with its output — written
before the work it describes, so a call not recorded as started certainly
did not run. The turn is identified by tenant, `X-Request-Id`, thread and
question; a client that retries the same request after a restart or a failed
round resumes it: settled calls are replayed, never-started calls run, a
started read-only (parallel-safe) call runs again, and a started call with
side effects is **not** run again — the model reads that it was interrupted
and its effect is unknown. The checkpoint is deleted when the turn is
answered (kept when its last round failed, so that retry resumes too) and
pruned after 24 h otherwise. A retry that arrives while the same turn is
still running in this process runs on its own without a checkpoint. A store
write that fails stops checkpointing for that turn and deletes the row, so a
stale record is never resumed. Server-generated request ids never repeat, so
only a client that sends its own id ever resumes. `turnResume: false`
(`entries.chat.config`) turns it off. Observed as
`kura_chat_turn_checkpoints_total{event}`,
`kura_chat_turn_resumed_calls_total{kind}` and the trace attributes
`resumedRounds`/`replayedCalls`/`interruptedCalls`/`rerunCalls`. Streaming
turns run no tools and keep none.

Untrusted tool output (agent-runtime-program.md 2.8, G9): every tool result
reaches the model enclosed in `<tool_output>…</tool_output>`
(`chat/src/framing.rs`), and any dispatch that offers tools or carries a
result has a fixed notice appended to its system message saying what is
inside is data from wherever the tool reached, never instructions, and that
a request found there is reported to the user rather than acted on. A result
cannot close its own frame: `<tool_output` and `</tool_output` inside it, in
any case, are escaped to `&lt;…`. Framing happens where the loop's history
becomes dispatch messages, so the persisted dispatch is still exactly what
the provider received. `toolOutputFraming: false` (`entries.chat.config`)
sends results verbatim. This is posture, not a boundary: what stops a tool
from doing harm is authorization and approval.

Turn admission (agent-runtime-program.md 4.1, `turn-admission.md`): every
turn takes a slot in `chat/src/admission.rs` before it runs. Over a
concurrency cap it waits in a queue that hands free slots to tenants in
round-robin. Over the turn rate or token budget it is refused with 429 and
`Retry-After`. The limits live in `entries.chat.config.limits`:
`globalConcurrency`, `tenantConcurrency`, `turnsPerMinute`,
`tokensPerMinute`, `tenantQueueDepth`, `globalQueueDepth` and
`queueTimeoutSeconds`. `enabled: false` removes the gate.

Structured output (agent-runtime-program.md 2.8, G7): `POST /v1/chat/query`
takes `responseFormat: {type: "json_schema", name, schema}`. The schema is
checked on arrival against the subset `kura_llm::ResponseFormat` can enforce
(`crates/engine/llm/src/structured.rs`: `type`, `properties`, `required`,
`additionalProperties`, `items`, `enum`, `const`, `anyOf`/`oneOf`/`allOf`,
internal `$ref`/`$defs`, numeric and length bounds, `pattern`; annotations
including `format` are allowed and not enforced); anything else is a 400
(`invalid_response_format`), never a keyword silently skipped. Every round
carries the schema in its system message, and the loop's final answer goes
through `kura_core::AnswerCheck`: a reply that does not parse or conform is
sent back once with the violations, as one more round of the same turn
(it counts against `toolMaxRounds`); a second failure ends the turn with
502 (`structured_output_invalid`). An accepted reply is returned parsed as
`structuredOutput`; `reply` keeps the model's own text (a reply wrapped in a
single code fence is accepted as its contents). Validation is bounded by
nesting depth and a step budget, so a self-referential `anyOf` cannot make
it run away. The streamed query refuses `responseFormat` with 400: it
forwards text before it could be checked. The schema is also handed to the
provider (`ProviderRequest.response_format`, not persisted: the system
message already records it). The OpenAI-compatible client sends it as
`response_format: {type: "json_schema", json_schema: {name, schema,
strict: false}}`, only on a round that offers no tools, because a server that
constrains every token to the schema (vLLM, llama.cpp behind the same wire)
would leave the model no way to call one. The Responses client sends it as
`text.format`, tools or not. Not strict on either, since strict mode takes
only a subset of JSON Schema. A 400 that names the field (or `json_schema`)
is answered by resending without it, and that client stops sending it.
Anthropic gets the instruction only.

Steering (agent-runtime-program.md 2.8, G8): a running query, streamed or
not, can be reached by the tenant that sent it, by the `X-Request-Id` it
was sent with (the SDK's `queryChat(input, {requestId})`).
`POST /v1/chat/turns/{requestId}/messages` with `{message}` answers 202
`{requestId, pending}`; the loop reads the message as the user's before its
next model call, after the results of the calls it is waiting on. One that
arrives while the model writes its answer makes that answer interim: the
model reads both and continues, and the extra rounds count against
`toolMaxRounds`. When the model answers with nothing waiting, the inbox is
closed in the same step, so a later message gets 409 `turn_finished` rather
than being accepted and never read. 404 `turn_not_found` is no such turn
for this tenant (another tenant's request id names nothing), 429
`turn_inbox_full` is 8 unread messages, 400 `invalid_steer_message` is empty
or over 32 KiB. `POST /v1/chat/turns/{requestId}/cancel` stops the turn; its
query answers 408 with a cancelled dispatch. A second turn started with a
request id a running turn holds runs unreachable. Counted as
`kura_chat_turn_controls_total{action, outcome}`; a turn's trace carries
`steerMessages`, and the session emits a `steer_message` event per message.
Streamed turns (2026-11-02): a streamed query (`POST /v1/chat/query/stream`,
and IM connectors that stream their replies) runs the same agent loop as a
query, with the same tools, resume, steering, hooks and records. Only the
text differs: each round streams as it is written. A round is a dispatch, so
a new `dispatchId` in a `chat.query.delta` frame starts a new round and its
`reply` starts over. Text before a tool call is interim, and the turn's
answer is the last round's, as for a query. Chunks reach the caller in the
order they were written, all before the turn reports done. A caller that
stops listening cancels the turn, which ends with `stream emitter failed`.
Before this, a stream was one dispatch with no tools.

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

Token budgets (agent-runtime-program.md 2.2, 2026-10-29): `budgetUnit`
picks what the budget counts. `chars` (default this release) is the
behavior above; note it counts bytes, so CJK text gets a third of the
window it appears to. `tokens` counts estimated tokens (ASCII at 4 chars
per token, any other character as one token, plus 4 per message), using
`personalBudgetTokens` / `threadBudgetTokens`, or the char budgets
converted at 4:1 when those are unset. When `modelContextWindows` names
the turn's model (`"*"` as fallback), the token budget never exceeds that
window less `reserveOutputTokens` (default 4096). `shadow` shapes by chars
but also computes the token decision and counts it in
`kura_session_window_shadow_total{agreement=same|tokens_elide_more|tokens_elide_fewer}`,
logging each disagreement, so the switch is made on measured traffic.
Rollback is `budgetUnit: "chars"` or removing the key.

Compression-to-memory (second slice, 2026-08-17): eviction never
plain-drops content. When the turn has a thread, the elided span is
captured as an L0 ref (thread source link, bounded excerpt) through the
governed memory pipeline — the async consolidator distills it into L1/L2
off the reply path with the write policy intact — and the elision marker
cites the captured asset (`…; elided span captured as Memory[l0_ref …]`)
so the model can drill back. Threadless turns keep their spans reachable
through dispatch records only.

In-window summary (agent-runtime-program.md 2.5, opt-in): with
`summaryBudgetTokens` set, the elided span is shown to the model as a
model-written summary (a system message prefixed `[session summary]`,
framed as a record rather than instructions) instead of only the marker.
The summary is per thread and per session segment, stored as a
`session_summary` manager document, and rolling: a dispatch uses whatever
summary exists, and when at least `summaryRefreshMessages` (default 4)
newly elided messages lie past it — or no summary exists yet — a
background job folds them in on the tenant's default model and
credential, off the reply path (pre-dispatch runs once per tool round).
Messages not yet covered keep the marker, so a late or failing summary
degrades to the behavior above. The window makes room for the summary
(its cost comes out of the history budget), the summary is cut to its
budget whatever the model returns, and each injection is recorded as a
`context.assembled` inclusion (`source: session_summary`, `layer:
summary`). One update runs per thread, at most 4 at once, with a 5-minute
pause after a failure. Metrics:
`kura_session_summary_windows_total{coverage=full|partial|none}` and
`kura_session_summary_updates_total{outcome=written|failed|empty|busy|backoff}`.
Rollback is removing `summaryBudgetTokens`; stored summaries are then
ignored.

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
(`Memory[l3 mem_xxx] title:` then the content): recalled memory is evidence,
never bare text. The content is framed as data (2026-11-02, G9 for memory):
it sits inside `<memory>` and `</memory>`, with those tags escaped inside
it, and one system notice before the first item says that what is inside
was remembered from earlier conversations and tools and is never
instructions. Memory can hold web pages and tool results captured into it,
so it gets the same posture as tool output. `memoryFraming: false`
(`entries.context.config`) injects it unframed, as before. L1 atoms are never bulk-injected (drill-down/retrieval only,
per the design root). Every pass emits a `context.assembled` event whose
`AssemblyRecord` lists inclusions (asset, layer, chars) and exclusions
with reasons (`over_budget`/`empty_content`/`visibility`) — nothing
enters or misses the context silently, and an engineer can reconstruct
what memory the model saw for any dispatch. Because injection happens
before the dispatch is prepared, the persisted dispatch record carries
the bootstrap verbatim (model-visible = logged holds).

Token budgets (agent-runtime-program.md 2.2, 2026-10-30): the context
config takes the same `budgetUnit` as session-strategy. `tokens` enforces
`memoryBudgetTokens` / `retrievalBudgetTokens` (unset: the char budgets
converted at 4:1), charging each injected message its estimated tokens as
rendered, citation included, plus the 4-token message overhead. `chars`
(default) and `shadow` enforce the char budgets as before. Whatever the
unit, the `AssemblyRecord` carries `budgetUnit`, both budgets, used tokens
next to used chars, and `tokens` per included item, so comparing the two is
a query over `context.assembled` events rather than a separate shadow
path. Rollback is `budgetUnit: "chars"` or removing the key.

The estimator is checked, not trusted: each dispatch span in a turn trace
carries `estimatedInputTokens` beside the provider's `inputTokens`, and
`kura_token_estimate_input_tokens_total{provider,source}` sums the two for
completed dispatches whose provider reported usage. Their ratio is how far
the estimate is off for that provider; size token budgets with it.

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

Built-in model embedder (G10, 2026-10-30): when the `embed` model role is
routed to a provider on the OpenAI-compatible wire (the
`openai_compatible` slot or an `openai_compatible` account) with a
non-empty model, the daemon serves the seam itself from that model's
`/embeddings` endpoint — vectors L2-normalized, cached under
`role-embed/<provider>/<model>/v1`. An external `context.embedder` plugin
still wins (logged). The binding is resolved at boot, so a role change
applies on restart. On the reply path a retrieval embeds at most 16
uncached docs, newest first, in one batch; the rest stay BM25-only
(`kura_retrieval_embeddings_total{outcome="deferred"}`) until the
operator runs `POST /v1/memory/indexes/rebuild?reembed=true`, which embeds
the whole recallable corpus off the async workers and reports
`embedded`/`failed`. A failed embed yields no vector — never a vector
from the hashed space — so the doc is BM25-only and is not cached; an
empty query vector skips the vector ranker for that retrieval. The 0.25
candidacy threshold was tuned on hashed trigrams; tune
`context.vectorMinSimilarity` for a neural model.

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
fusion change. A failed call yields no vector (logged): the doc stays
BM25-only and nothing is cached. (It used to answer with a hashed vector,
which was then cached under the plugin's fingerprint — two vector spaces
in one index.) First declaration wins; duplicate
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
