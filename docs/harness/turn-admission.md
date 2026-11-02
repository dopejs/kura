# Turn Admission: Per-Tenant Limits and a Fair Queue

> Status: implemented 2026-11-02 (agent-runtime-program.md 4.1, shared-hosting
> G12). Owner: daemon.

## 1. Problem

Nothing bounds how many agent turns run at once, for one tenant or for the
daemon. Every entry point starts work immediately:

| Entry | Where it runs | Bound today |
|---|---|---|
| `POST /v1/chat/query` | tokio blocking pool (`spawn_blocking`) | pool size (512) |
| `POST /v1/chat/query/stream` | a new `std::thread` per request | none |
| IM connectors (`kura-im`) | the connector loop | none |
| swarm children | `std::thread::scope`, 4 per run | runs per tenant: none |
| activation test chat, skill proposals | caller's thread | none |

All of them go through `kura_chat::Service::{query, stream, stream_channel}`.

The 4.3 load test shows what one heavy tenant does to everyone else. All
turns write through one SQLite writer, and its wait grows faster than the
load:

| Sessions | Turn p95 | Writer mutex p95 / max |
|---|---|---|
| 32 | 205 ms | 12 / 24 ms |
| 128 | 702 ms | 37 / 143 ms |

So a tenant that opens a hundred streams slows every other tenant's turns
by 3–4×. It can also fill the blocking pool, and then other tenants' turns
cannot start at all.

## 2. Decisions

1. **One gate, in the chat service.** Every turn already passes through
   `Service::query`/`stream`/`stream_channel`. The gate goes there, not in
   HTTP middleware, so IM, swarm and internal callers are covered too.
   `POST /v1/llm/dispatches` is a raw dispatch, not a turn. In shared mode
   it is operator-only, so it stays outside the gate.
2. **A permit is held for the whole turn.** This covers every round and
   tool call, because the store writes happen inside the turn. The permit
   is released on drop, so every exit path releases it, including panic
   and cancel.
3. **How each limit fails:**
   - *Concurrency:* the turn queues, with a bounded wait. A burst within
     the cap is ordinary use and should not fail.
   - *Turn rate and token budget:* immediate 429 with `Retry-After`. These
     limit sustained use, and queueing would only hold a thread.
4. **The queue is fair across tenants.** When a slot frees, it goes to the
   next tenant in round-robin among tenants that have waiters and are under
   their own cap. Within one tenant, turns run first in, first out. A
   tenant with 100 queued turns gets one slot in turn, the same as a tenant
   with 1.
5. **Token budget:** charged after each settled dispatch from the usage the
   provider reported. It is checked only when a turn is admitted. A turn
   that is already running finishes, so the budget can go into debt by at
   most one turn's usage. Estimating usage up front would not be more
   accurate, and stopping a turn mid-tool-loop leaves worse state than
   overshooting by one turn.
6. **These limits sit beside billing.** Billing quotas (`kura-billing`)
   count monthly units, such as runs and tool calls, against a plan. These
   limits shape load over seconds and minutes. They do not replace or
   touch billing reservations. On a shared daemon tenants dispatch with
   their own key (shared-hosting.md §7), so the token budget protects a
   tenant's key from a runaway loop, not the platform's spend. It is off
   by default.
7. **Exempt work.** Work with no tenant (the daemon's own) and, on a
   shared daemon, the operator tenant skip the per-tenant limits. They
   still count toward the global cap and wait in the same fair queue. A
   single-owner daemon exempts no tenant, but its per-tenant limits
   default to off, so only the global cap applies unless the owner sets
   them.

## 3. Limits and defaults

Configured in the plugin profile under `entries.chat.config.limits`, beside
the other chat knobs. Each value takes the default for the hosting mode
when absent. `0` means unbounded.

| Key | Shared | Single | Meaning |
|---|---|---|---|
| `enabled` | true | true | `false` removes the gate entirely; this is the rollback |
| `globalConcurrency` | 64 | 64 | turns running at once, daemon-wide |
| `tenantConcurrency` | 8 | 0 | turns running at once per tenant |
| `turnsPerMinute` | 30 | 0 | turn starts per tenant per minute (bucket, burst = the same number) |
| `tokensPerMinute` | 0 | 0 | provider-reported tokens per tenant per minute (bucket; debt allowed) |
| `tenantQueueDepth` | 16 | 0 | turns one tenant may have waiting |
| `globalQueueDepth` | 256 | 256 | turns waiting, daemon-wide |
| `queueTimeoutSeconds` | 120 | 120 | longest wait for a slot |

How the defaults were chosen:
- **64 global:** between the two measured points. Writer p95 stays well
  under the 37 ms seen at 128.
- **8 per tenant:** a swarm run takes 4 children, and this leaves room for
  interactive turns beside it.
- **Waiting threads:** at most `globalConcurrency + globalQueueDepth` = 320
  turn threads, under tokio's blocking pool of 512.
- **120 s timeout:** a real turn takes 10–60 s, so a wait has to outlast at
  least one turn ahead of it.

## 4. Behaviour at the edges

| Outcome | Chat HTTP | Stream HTTP | Others |
|---|---|---|---|
| rate limited | 429 `turn_rate_limited`, `Retry-After` = time to next token | same, before the SSE response starts | `ChatError::Throttled` |
| token budget | 429 `token_budget_exhausted`, `Retry-After` = time the debt takes to clear | same | same |
| queue full | 429 `turn_queue_full`, `Retry-After: 5` | same | same |
| waited too long | 429 `turn_queue_timeout`, `Retry-After: 5` | same | same |

Notes:
- The stream route admits before it answers. A throttled stream therefore
  gets a plain 429, not a 200 followed by a failure frame.
- A swarm child that is throttled fails like any other failed child. It
  releases its billing reservation and its error is the throttle code.
- An IM message that is throttled gets the connector's ordinary failure
  reply.

A turn denied at the gate:
- is not a turn: it has no trace and no dispatch row;
- consumes a turn-rate token only once it is admitted or queued, so a
  refusal for a full queue costs nothing.

## 5. Observability

- `kura_chat_turn_admissions_total{outcome}`. `outcome` is one of
  `admitted`, `queued`, `turn_rate_limited`, `token_budget_exhausted`,
  `turn_queue_full` or `turn_queue_timeout`. `admitted` counts turns that
  ran without waiting, and `queued` counts turns that ran after waiting.
- `kura_chat_turn_queue_wait_seconds`: the wait of every queued turn that
  ran.
- Each denial logs one `[kura] turn admission:` line with the tenant, the
  reason and the retry hint. The effective limits are logged once at boot.

## 5a. Verification

- Unit tests (`kura-chat`, `admission::tests`):
  - round-robin hand-off order;
  - a newcomer cannot take a slot already handed to a waiter;
  - per-tenant and global queue bounds;
  - timeout withdrawal;
  - rate and token buckets with their `Retry-After`;
  - exemption;
  - sweep.
- Route tests (`kura-api`): query and stream both answer 429 with
  `Retry-After` and the limit's code, and a stream is refused before it
  starts.
- Config tests (`kura-app`, `turn_limits_tests`): the defaults per hosting
  mode, overrides, the `enabled: false` rollback, and boot refusing an
  unknown key or a zero timeout.
- Assembly: `tests/agent_load.rs` runs its tenants, tool turns and swarms
  through a global cap of 4. Every turn and child completes, none is
  refused, and turns did queue (CI: 24 turns and 12 children, 32 waits).

## 6. Risks and rollback

- **Deadlock if a turn waits on another turn.** No path does this today:
  no tool starts a chat turn, and swarm children are started from a
  background thread, not from inside a turn. A future tool that runs a
  sub-turn would hold one permit and need another. With
  `tenantConcurrency` permits all held by parents, it would wait until the
  timeout and then fail, not hang. Such a tool must take its permit
  through the parent's (not built).
- **Process-local.** One daemon process is the only deployment for now
  (shared-hosting.md non-goals). Multiple replicas would need a shared
  limiter.
- **Queued threads are real threads.** The bound is
  `globalConcurrency + globalQueueDepth`. Raising both past the blocking
  pool size moves the starvation back into tokio.
- **Rollback:** `"limits": {"enabled": false}` in the chat plugin config
  restores the previous behaviour exactly. No store migration is involved.
