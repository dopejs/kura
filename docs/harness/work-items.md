# Work Items: Long-Lived Work, Proactive Rules and Attention

> Status: design, 2026-11-02; decisions recorded 2026-11-03 (§13). Slices 1
> and 2 implemented 2026-11-03, slices 3 and 4 2026-11-09 (§12 records where
> each differs). Adapts docs 000–003 of
> DeepSeek's dsh-superagent prototype (Ticket, Mandate, Subscription,
> accept mode, Attention, recovery note, ordered replies) to kura. Owner:
> daemon.
>
> Evidence: every `path:line` below was read on `f011514`. Anything not
> read is marked *unverified*.

## 1. Problem

Every unit of agent work in kura is one turn. A turn can be steered while
it runs and resumed after a crash, but once it answers nothing of it is
left that can wait. So kura cannot do work like this:

1. A person asks in a group chat for a fix.
2. The agent works on it and sends an interim result.
3. It waits for CI (a webhook) and for the person's review.
4. It goes on when either arrives, under the same authority, without
   being mentioned again.

Three things are also missing:
- **Proactive work.** There is no rule that says "work like this may start
  without being asked", and no mode that only records what would have
  started.
- **A queue of what waits on the user.** There is no structured "this work
  needs a person, and here is the one question".
- **Recovery of daemon-initiated turns.** After a restart they are not
  recovered at all.

## 2. What kura already has

| Need | Exists | Evidence | Gap for this design |
|---|---|---|---|
| A conversation identity across messages | `SourceContinuationKey` (tenant, connector, account, conversation) → `thr_src_<hash>` | `engine/threads/src/source.rs:75-123`, `channels/im/src/lib.rs:376-379`, `:413-424` | Only IM builds one. Webhooks and mail have no conversation key. |
| Inbound dedupe | `create_connector_message_if_absent` | `channels/im/src/lib.rs:625-628` | — |
| A gate on who may start work in IM | Group/room participation: allowlist, then mention, fail-closed. Every decision is persisted. | `engine/threads/src/group_room.rs:354-419`, `channels/im/src/lib.rs:1202-1241` | Only groups and rooms are gated. DMs pass. There is no notion of "accepted work". |
| One turn | `Service::query` / `stream` / `stream_channel`, behind turn admission | `domains/chat/src/service.rs:80`, `:333`; `docs/harness/turn-admission.md` | — |
| Steering a running turn | `TurnInbox`, read before every model call. `steer_turn` addresses a turn by tenant + request id. | `engine/core/src/steer.rs:16-32`, `domains/chat/src/steer.rs:194` | — |
| Resume after a crash | Turn checkpoint keyed by (tenant, request id, thread, question). A started side-effecting call is reported as interrupted, not rerun. | `engine/core/src/checkpoint.rs:15-17`, `domains/chat/src/resume.rs:58-125`; `agent-runtime-program.md` 2.7 | Resume happens **only when a client retries**. IM and scheduled turns never resume (2.7 *Not covered*). Checkpoints live 24 h (`resume.rs:30`). |
| Structured final answer | `responseFormat` on non-streamed queries, validated, with one correction round | `service.rs:104`, `:251`; `plugin-architecture.md` chat | — |
| Human approval of a tool call | `kura_policy::Engine`: pending → approved / rejected. The MCP tool waits 180 s, then gives the turn back with the approval left pending. | `domains/policy/src/lib.rs:136`, `domains/mcp/src/agent_tool.rs:32`, `:252-296` | Engine is in-memory, restored at boot (`surface/app/src/restore.rs` "Policy"). Rows are written tenantless and bound to a tenant at the route (`routes/policy.rs:16-27`). Nothing wakes work when an approval resolves. |
| Rule matching | Triage: ordered rules, AND conditions, first match wins. It only proposes; it never acts. | `domains/triage/src/lib.rs:1-5`, `:362-416` | Mail fields only. Rules are scoped by `environment_scope`, not tenant. No accept mode. |
| Versioned user rules | Routines: "edits create new versions" | `domains/routine/src/lib.rs:1-5` | — |
| Webhooks | Signed and tenant-checked, with replay protection. They fire a routine, workflow or run. | `domains/webhook/src/lib.rs:1-6`, `:174`; firer `surface/app/src/adapters.rs:270-291` | The firer only creates a `Queued` run in the in-memory runtime ledger (`engine/runtime/src/lib.rs:397-428`). I found nothing that executes a queued run's goal as a turn (grep for `RunStatus::Queued` consumers). *Treat webhook → agent work as not wired.* |
| Outbound to a conversation | IM replies are sent inline by the connector loop (`execute_reply_path`, `channels/im/src/lib.rs:1634`). Delivery sends to a target chosen by preference, through `ConnectorAdapter`. | `domains/delivery/src/connector_adapter.rs:24-40`, `manager.rs:442-452` | See §8. |

Two constraints follow from this table:
- **Shared hosting.** Connectors, webhooks, delivery, triage and routines
  are not on the tenant route allowlist (`TENANT_ROUTES`,
  `surface/api/src/hosting.rs:84-185`). Only chat, sessions, threads, reminders
  and approvals are. In shared mode a tenant can therefore only create
  work items through chat or the API until those families are
  tenant-scoped. This design does not widen them.
- **Bots.** `InboundMessage` has no "author is a bot" flag
  (`channels/imtypes/src/lib.rs:129-149`). Whether connectors drop bot
  messages upstream is *unverified*.

## 3. Decisions

1. **One new aggregate: the work item.** It is the dsh Ticket. Nothing in
   kura already holds work that waits (§2). It owns its mandate,
   subscriptions, pending inputs, rounds and attention. Everything else is
   reused.
2. **A round is an ordinary chat turn.** Each time a work item wakes, it
   runs one `Service::query` through turn admission. The turn's request id
   is deterministic: `wi_<item>_r<seq>`. Reused as-is:
   - the 2.7 checkpoint, which gives crash resume;
   - 2.8 steering;
   - turn traces, admission fairness and the token budget.

   There is no separate executor, and no dsh "Assignment".
3. **Inputs that arrive while a round runs steer it.** They go through the
   existing `TurnInbox`. Inputs that arrive while the item is idle are
   stored and batched into the next round. If the inbox refuses an input
   (`turn_finished`, `turn_inbox_full`), the input is stored for the next
   round instead. Nothing is dropped.
4. **The end of a round is a structured report**, read through
   `responseFormat`:
   - `status`: `progress` | `waiting` | `ready` | `blocked` | `failed` |
     `dropped`
   - `summary`
   - optional `attention: {question, options?}`

   A report that fails validation counts as `blocked` with reason
   `report_unreadable`. A person sees it instead of the item idling
   silently.
5. **The mandate is immutable and records its basis.** It is checked again
   before an ended item resumes, never by a fresh evaluation (§5.4).
6. **Approvals are reused as one kind of attention, not replaced** (§7).
7. **Off by default.** `entries.work-items.config.enabled: false` (the
   plugin id is `work-items`; profile keys are plugin ids) keeps today's
   behaviour exactly. The proactive accept mode defaults to
   `observe`.
8. **Tenant-scoped from day one.** Every new row has `tenant_id NOT NULL`.
   Every read filters on the caller's tenant, and another tenant's id
   answers 404. This differs from approvals and delivery today, which are
   written tenantless.

**Not adopted from dsh:**
- *A separate main Session that evaluates.* A turn the IM gate already
  admitted is the explicit request. The model opens a work item with a
  tool call (§5.1).
- *Model assessment of inferred candidates.* v1 rules are deterministic;
  see open decision D3.
- *Hand-back of follow-ups between Executors.* v1 has one owning item per
  thread (D2).
- *Per-ticket directories and prepare commands.* The sandbox plane owns
  workspaces.

## 4. Model

```
WorkItem   tenant, id, title, state(open|ended), end_reason, mandate, chat thread,
           seq, last_report, attention?, policy_hash_seen, created/updated
Mandate    (immutable) basis, strength(explicit|inferred), policy_hash, matched_rules[],
           sender{connector_id, author_id | principal_id}, source{kind, ref, conversation},
           receiving_policy_id, scope_text, created_at
Subscription  (item, source key, role: owner|context, created, last_event_at, dropped_at?)
Input      (item, seq, kind: message|event|approval|policy_notice|recovery|text,
           author_is_person, body, source ref, state: pending|steered|consumed, round_seq?)
Round      (item, seq, request_id, query_text, state: running|settled|failed, report?, timestamps)
```

- **Source key.**
  - IM: the existing normalized `SourceContinuationKey`.
  - Webhook: `(tenant, "webhook:<id>", <thread ref>)`. The thread ref comes
    from a declared header, e.g. `X-Kura-Thread`, and is empty when there
    is none (D5).
  - Approval: the approval id.

  One shape, matched by equality. The key is not a new id scheme.
- **Chat thread.** Each item runs its rounds on its own thread,
  `thr_wi_<hex>`, where the item is `wi_<hex>` (a UUIDv7). Its history is the item's history. Each round's query
  quotes the inputs that woke it, including the IM messages. *Tradeoff:*
  an ordinary IM turn after the item ends does not see the rounds in
  continuity. The connector transcript still has the replies. The
  alternative, reusing `thr_src_*`, would mix two writers into one
  continuity window.
- **Round query text** is stored before the turn starts. A restart then
  rebuilds the same 2.7 turn key and finds the checkpoint (§6).

### 4.1 Stage (derived, never stored)

The stage is computed from stored facts on every read, so a restart keeps
it. The first matching row wins:

| Stage | Condition | Label |
|---|---|---|
| `person/delivery` | The last round ended without a report (admission refusal, provider or transport error), or an outbound message failed 3 times in a row. Once the driver retries (slice 2), only after its backoff gives up | Not delivered |
| `person/approval` | A pending approval is linked as attention | Awaiting approval |
| `person/answer` | `attention` is set | Awaiting answer |
| `person/blocked` · `person/failed` | The last report is `blocked` / `failed`, and no person's input arrived since | Blocked · Failed |
| `queued` | Pending inputs and no round running; waiting for an admission slot | Queued |
| `working` | A round is running | Working |
| `idle` | Anything else: the last report is `waiting`, or `progress` with no pending input | Waiting for an event |

`ready` and `dropped` end the item (`end_reason` = `completed` /
`dropped`). Ended items have no stage.

## 5. Flows

### 5.1 Accept: how a work item starts

| Path | Basis | Strength | Gate it already passed |
|---|---|---|---|
| A turn in IM calls the tool `work_item.open {title, scope, subscribe_origin: true}` | `im_mention` / `im_direct` | explicit | Participation decision (group/room) or a DM. The decision id is recorded. |
| `POST /v1/work-items` by a tenant member | `api` | explicit | Auth + `TenantContext` |
| A chat turn from web or TUI calls `work_item.open` | `api` | explicit | Same as above |
| An event matches a proactive rule (§5.5) | `rule` | inferred | Source gate (participation, webhook signature) + rule match + accept mode |

`work_item.open` is offered only to turns that are not themselves rounds.
A work item cannot spawn work items in v1. It returns the item id, and the
turn's own answer is the acknowledgement.

### 5.2 Wake: fan-out and ownership

The hook point is the IM loop, after dedupe and the archived check and
before participation (`channels/im/src/lib.rs:625-700`).

1. Look up live subscriptions for the message's source key.
2. If an owning item exists and is open, or ended and not stopped:
   - store an `Input`;
   - do **not** start the ordinary IM turn;
   - record the connector route outcome `work_item`.
3. Context subscriptions get the input too, but it grants no authority and
   does not wake an ended item.

| Event | Open item | Ended item (not stopped) | Stopped item |
|---|---|---|---|
| A person's message on an owner thread | Input; wakes or steers | Revalidate (§5.4). Pass: reopen and wake. Fail: the message falls through to the ordinary IM path. | Falls through |
| A bot or webhook event on a subscribed key | Input; wakes or steers | Recorded only | Nothing |
| A linked approval resolved | Input; wakes | — | — |
| Policy changed (hash differs at the next wake) | A `policy_notice` input names what changed | Checked at revalidation | — |

Messages in a group that do not mention the agent still reach an item
that owns the thread. That is what the subscription is for. *Risk:* the
agent now reads every message in that conversation, so `subscribe_origin`
on a group needs the participation policy's allowlist to hold for the
requester (D2).

### 5.3 Round driver

- **One driver task per daemon.** It starts a round when an item has
  pending inputs, no running round, and no person-stage hold.
- **One round per item at a time.** Across items, the turn admission gate
  is the only limiter. Its fair queue already gives each tenant a fair
  share.
- **A round runs as a non-streamed query** with:
  - the report schema as `responseFormat`;
  - the mandate's scope text and matched-rule instructions in the
    system frame;
  - the request id `wi_<item>_r<seq>`.
- **Report handling:**
  - `ready` / `dropped`: end the item.
  - `waiting`: idle.
  - `blocked` / `failed`: hold for a person.
  - `progress`: run again only if inputs are pending.
- **Transient failures** (429 throttle, provider 5xx, timeout) are retried
  with backoff: 2 s doubling to 60 s, three times. After that the item
  shows `person/delivery`.
- **Pause** is per tenant (`POST /v1/work-items:pause`). It stops the
  driver starting rounds and sending outbound. Inputs keep being recorded.

### 5.4 Mandate revalidation (before an ended item resumes)

- **Fast path:** the policy hash equals `mandate.policy_hash` → pass.
  An empty `mandate.policy_hash` (items opened before work rules exist)
  never takes the fast path.
  The hash is SHA-256 over the canonical JSON of the tenant's work rules,
  its accept mode, the channel route policy rows and allowlists of the
  mandate's connector, and the webhook endpoint's status.
- **Otherwise, check by basis:**

| Basis | Passes when |
|---|---|
| `api` | The principal is still a member of the tenant, with a role that may chat |
| `im_mention` / `im_direct` | The connector is still enabled for the tenant and still receives that conversation. The sender still passes the allowlist step of `evaluate_participation`, with the mention requirement waived. |
| `rule` | Accept mode is `auto`. Every matched rule still exists, is enabled and has the same version. The source is still enabled. |

- **On failure:**
  - the item stays ended, with `end_reason = revoked`;
  - the waking message falls through to the ordinary path, as if no item
    owned it;
  - a `work_item.mandate_revoked` event is emitted.
- **Live items are not cancelled on a policy change.** They get a
  `policy_notice` input. Cancelling leaves half-finished work (dsh 002).

**Who can stop an item:**
- A person with access to the tenant, through the API or the TUI/web.
  This is recorded as a manual stop and cannot be undone.
- The model in a round, with the report `dropped`. That is an end, not a
  stop, so a later person message may resume the item.

A "stop" written in chat is an ordinary input. The round decides what to
do with it.

### 5.5 Proactive rules and accept mode

A **work rule** has:
- `rule_id`, `version`, `enabled`;
- `source` (a connector id, or `webhook:<id>`);
- `conditions`: triage's `Condition {field, operator, value}`, with fields
  extended to `author`, `conversation`, `text` and `header:<name>`;
- `instructions`: text the item's rounds read;
- `subscribe`: which keys the item follows.

- **Reuse of triage:** the `match_condition` / `compare_*` logic
  (`domains/triage/src/lib.rs:407-440`) moves to a shared function. Triage
  keeps its own crate and its no-side-effects contract.
- **Versioning:** rules are versioned like routines. An edit is a new
  version; old versions stay for audit.

Accept mode is per tenant:

| Mode | Effect |
|---|---|
| `off` | No rule is evaluated |
| `observe` (default) | Matching events are recorded as `work_item.rule_observed {rule_id, version, source key, would_accept: true}`. No item is created and nothing is sent. |
| `auto` | A match creates an item with an inferred mandate |

- **Explicit paths (§5.1) work in every mode.** One event creates at most
  one item per rule. The idempotency key is (rule_id, source event id).
  For IM events that is the inbound `delivery_id`; for webhooks, the
  trigger's idempotency key.
- **Webhook events reach rules** only once `TargetKind::WorkItem` exists.
  Today's targets go to the unexecuted run ledger (§2).

## 6. Restart recovery

At boot, after `recover_persisted_state`:
1. Mark every round in `running` as interrupted.
2. Queue its item again, oldest first, through the normal driver. Turn
   admission bounds the burst.
3. Rerun the round with the **same request id and stored query text**, so
   `TurnResume::claim` (`domains/chat/src/resume.rs:58-125`) loads the
   checkpoint:
   - settled calls are replayed;
   - a started side-effecting call reads `INTERRUPTED_CALL_OUTPUT`.

   A checkpoint older than 24 h is gone, and the round runs fresh.
4. Put a `recovery` input before the round's other inputs:

   > The daemon restarted while you were working on this. Before you
   > repeat anything with an outside effect (a message, a mail, a tool
   > that changes something), check whether it already happened. Here
   > are the messages this item has sent so far, from the outbound log:
   > …

   The outbound log (§8) makes the check concrete instead of a guess.

Items that were idle, ended or held for a person are untouched. One
exception: an item that got a person's input after its last report starts
again, as in dsh 003.

This deliberately departs from 2.7's "resume only when the client
retries". The reason there was that a resumed answer must reach whoever
asked. A work item's answer has a home, its channels, so recovering at
boot spends tokens for someone. `recoverOnBoot: false` turns it off.

## 7. Attention and the waiting queue

- **The attention field.** `attention` is a field on the item, set only by
  a report's structured `attention`. Free-text summaries never set it
  (dsh rejected inferring "waiting" from prose). A person's next input on
  an owner thread, or `POST /v1/work-items/{id}/answer {text}`, clears it.
  The answer becomes an input.
- **Approvals.** If a round's MCP call gives up waiting (180 s), the
  approval stays pending (`agent_tool.rs:28-32`). The round links it as
  attention of kind `approval {approval_id}` and reports `waiting`. When
  `/v1/policy/approvals/{id}/resolve` resolves it, the route publishes the
  existing `policy.approval_resolved` event (`routes/policy.rs:529-535`). The driver subscribes to
  that event and adds an `approval` input. The round then calls the tool
  again; 2.7 already reports the interrupted call honestly.
- **No second approval system.** The approval engine stays the only place
  a person grants a tool call. A work item only remembers that it is
  waiting on one.
- **The queue.** `GET /v1/work-items/attention` lists, for the caller's
  tenant, the items in a `person/*` stage, oldest first. Each entry has
  the stage, the question or approval, and the item title. The TUI and
  web show this list first; a healthy item takes no space. Pending
  approvals not linked to any item stay in `/v1/policy/approvals` as
  today.

## 8. Ordered outbound: verdict on delivery

**Delivery does not give per-conversation order today.** Evidence:
- **No ordering.** Each outcome is dispatched and retried on its own: a
  detached thread sleeps until the retry time
  (`domains/delivery/src/dispatcher.rs:260-282`). Nothing makes a later
  outcome to the same conversation wait.
- **Not restored at boot.** Pending retries live only in that thread and
  the in-memory `retry_scheduled` map. `restore.rs` restores no delivery
  state, and I found no boot sweep (grep only).
- **Not addressed to a conversation.** The target is chosen by preference
  and result class (`manager.rs:455-456`), so an outcome cannot say "reply
  in this chat".
- **Not idempotent per message.** Outcomes are idempotent per
  `(source_kind, source_id)` (`manager.rs:446-452`), which fits one
  notification per run, not a series of replies.
- **No drop status.** Failures carry a reason code
  (`fail_outcome_without_retry`, e.g. `target_unavailable`,
  `connector_disabled`), but there is no terminal *dropped* status and no
  "never queued again" rule.
- **Tenantless rows.** Outcome rows are written tenantless
  (`persistence/store/src/delivery.rs:6-7`).

IM replies bypass delivery entirely (inline send in the connector loop).
So this feature stays.

**Design.** The work item gets an outbox, a table rather than a new
plane. It sends through delivery's existing `ConnectorReplySender`
registry (`connector_adapter.rs:24-40`), so connector code is not
duplicated.
- **Row:** `(tenant, item, seq, conversation key, binding{connector,
  channel, peer, thread}, body, state: queued|sent|failed|dropped,
  attempts, last_error, drop_reason, created, sent_at)`.
- **Order:** for one (item, conversation key), a row is sent only once
  every earlier row is `sent` or `dropped`. Other items and other
  conversations do not wait.
- **Retries:** backoff 5 s doubling to 300 s, persisted as
  `next_attempt_at`. A boot sweep resumes them, so there are no detached
  timers. After 3 failures in a row the row shows as a problem and puts
  the item in `person/delivery`. It keeps retrying, and its successors
  keep waiting.
- **Drop:** a row is dropped only when:
  - the connector no longer receives that conversation (`connector_gone`);
  - the platform answers that it can never deliver (`conversation_gone`,
    `forbidden`);
  - a person withdraws it.

  A drop keeps when and why, and is never queued again.
- **Sending is at-least-once.** A crash after the platform accepted a
  message but before `sent` was written sends it again on recovery. This
  is visible in the outbound log the recovery note quotes. Exactly-once
  would need platform idempotency keys, which no connector exposes here
  (*unverified per connector*).

Moving delivery itself onto this table (D4) is a separate decision. It
would also fix delivery's restart and tenant gaps.

## 9. Store, API, contracts, compatibility

- **Store:**
  - Migration v10 `work_items` (slice 1; v9 is the devices track's) adds
    `work_items`, `work_item_rounds` and `work_item_inputs`; v11
    `work_item_subscriptions` (slice 2) adds subscriptions, with a partial
    unique index for one live owner per source. Each later
    slice adds its own tables (`work_item_subscriptions`,
    `work_item_outbox`, `work_rules`, `work_policy`) in its own migration:
    a shipped migration is never edited. The mandate is JSON in
    `work_items`, never updated.
  - Indexes on `(tenant_id, state, updated_at)`, rounds by `state` (the
    boot scan), inputs by `(item_id, state)`; later, subscriptions by
    `(source_key)` where `dropped_at IS NULL`, and outbox by
    `(state, next_attempt_at)`.
  - Purely additive. No existing table is altered.
- **Existing data:** threads, `agent_turn_checkpoints`, connector messages
  and approvals are read, never rewritten.
  - Checkpoints written before this change carry no item and are never
    claimed by the driver: its request ids start with `wi_`.
  - Existing threads gain no subscription until an item is opened on them.
- **API** (tenant allowlist, every handler filters by tenant):
  - `GET/POST /v1/work-items`
  - `GET /v1/work-items/{id}`, with inputs, rounds, outbox and stage
  - `POST /v1/work-items/{id}/answer`, `/stop`, `/retry`
  - `POST /v1/work-items/{id}/outbox/{seq}/withdraw` (slice 3)
  - `DELETE /v1/work-items/{id}/subscriptions/{sub}`
  - `GET /v1/work-items/attention`
  - `GET/PUT /v1/work-rules`, `GET/PUT /v1/work-policy`
  - `POST /v1/work-items:pause` / `:resume`
- **Contracts first.** Add `schemas/api/work-item*.schema.json` and
  `schemas/events/work-item-*.event.schema.json`, regenerate the client
  types, then run `make daemon-contract-test`.
- **Chat tools:** `work_item.open` for ordinary turns, and
  `work_item.unsubscribe` for rounds (a round cannot drop its owner
  subscription).
- **Rollback:**
  - `enabled: false` stops the driver, the IM hook and the tools. IM
    messages take the ordinary path again.
  - The tables stay and are ignored. No data migration is reversed.

## 10. Observability

- **Metrics:**
  - `kura_work_items{stage}` — a gauge per tenant class, not per tenant id.
    Not built yet: the metrics registry has no gauges.
  - `kura_work_items_opened_total{basis}` (slice 1).
  - `kura_work_item_rounds_total{outcome}`, where `outcome` is a report
    status, `report_unreadable`, `turn_error` (the turn ended without a
    report) or, once the driver retries, `transient_retry`.
  - `kura_work_item_inputs_total{kind, path=steered|queued|recorded_only}`.
  - `kura_work_item_mandate_checks_total{result=pass_fast|pass|revoked}`.
  - `kura_work_rule_matches_total{mode=observe|auto}`.
  - `kura_work_item_outbox_total{result=sent|retry|dropped}`.
  - `kura_work_item_recovered_total{mode=resumed|fresh|skipped}` (slice 4).
- **Events** (bus + schemas): `work_item.opened`, `.woken`,
  `.round_settled`, `.attention_raised`, `.attention_cleared`, `.ended`,
  `.stopped`, `.mandate_revoked`, `.rule_observed`, `.outbox_dropped`.
  Each carries tenant, item, and mandate basis; no message bodies.
- **Logs:** one `[kura] work item:` line per state change, with tenant,
  item, from → to and reason. The effective config is logged once at boot.
- **Traces:** a round's turn trace carries `workItemId` and `roundSeq`.
  Slice 1 has neither attribute: the trace's request id
  (`wi_<hex>_r<seq>`) names both.

## 11. Failure modes and risks

| Failure | Effect | Mitigation |
|---|---|---|
| Crash mid-round | Round interrupted | §6: same request id → checkpoint; recovery note; outbox log |
| Crash after send, before `sent` | Duplicate message | Accepted, at-least-once; visible in the log the note quotes |
| Store write of an input fails | IM message lost to the item | The IM loop falls through to the ordinary turn, so the person still gets an answer |
| A runaway item wakes itself | Token spend | Rounds only start on new inputs; webhook storms coalesce into one round; per-tenant `maxOpenItems` (50) and `maxRoundsPerItemPerHour` (20); the token budget of turn admission |
| A group subscription reads unaddressed chatter | Privacy, cost | Owner subscription needs an explicit request from an allowlisted sender; subscriptions expire after `subscriptionIdleDays` (14) with no events; a person can drop any subscription |
| Prompt injection in a subscribed thread | The agent is steered by a third party | Inputs are framed like tool output (G9 framing); the mandate's scope is in the system frame; side-effect tools still pass the approval gate |
| A policy loosened then tightened while an item is idle | Item resumes under an old basis | Revalidation by basis on every resume, not just the hash |
| Approval resolved while the daemon is down | No wake event | At boot, the driver checks linked approvals' status |
| Deadlock: a round's tool opens a turn | Holds two permits | `work_item.open` is refused inside rounds; no round tool starts a turn |
| Multi-replica daemon | Two drivers | Not supported (single process, as turn-admission.md §6); the driver takes a store lease before running |

## 12. Slices

Each slice is shippable behind `enabled` and has its own tests.

1. **Items, mandate and API, no wake.**
   - Builds: migration v10 (items + rounds + inputs); `POST /v1/work-items`
     runs one round as a turn with the report schema; stage derivation;
     events; schemas.
   - Tests: store round-trip; stage first-match table; tenant isolation
     (two tenants, 404); report validation → `report_unreadable`.
   - As built (slice 1), where it differs from or adds to the above:
     - `POST /v1/work-items {title, scope, provider?, model?}` admits the
       round's turn before recording anything (a refusal is 429 and opens
       nothing), opens the item and answers 201 with stage `working`. The
       round runs after the response on its own thread, through the new
       `Service::query_admitted`; clients read the outcome with
       `GET /v1/work-items/{id}` or the events.
     - The scope text is in the round's query, not the system frame: in
       slice 1 every input is the requester's own. It moves to the system
       frame with slice 2, when third-party inputs arrive.
     - A round that ended without a report is `failed` with the chat error
       code and shows `person/delivery`; there is no retry until the
       driver exists. A reply that never conforms is `report_unreadable`
       → `blocked` (`person/blocked`).
     - `maxOpenItems` (D7, default 50) is enforced at open (409). The other
       D7 knobs are refused as unknown keys until their slice ships.
     - Events: `opened`, `round_settled`, `attention_raised`, `ended`.
     - Routes: `GET/POST /v1/work-items`, `GET /v1/work-items/{id}`, all on
       the tenant allowlist. Without a resolved tenant the default tenant
       is used, as reminders do.
2. **IM subscriptions and wake.**
   - Builds: `work_item.open` tool; subscriptions; the IM hook; steering
     of a running round; resume of an ended item with revalidation
     (`api`, `im_*` bases); stop.
   - Tests: an IM e2e with the test connector: open → idle → a person's
     message wakes the item → reply. A non-owner message falls through. A
     stopped item does not wake. A revoked allowlist entry falls through.
   - As built (slice 2), where it differs from or adds to the above
     (tests: `crates/domains/workitems/tests/im_wake.rs` over the real
     `MessageLoop`, plus unit tests in `wake.rs` and the store):
     - **Store.** Migration v11 adds `work_item_subscriptions`, additive.
       A subscription is never deleted, only dropped with a reason:
       `stopped`, `revoked`, `superseded` (an ended owner replaced by a
       new item on the same conversation) or `idle`. Idle expiry
       (`subscriptionIdleDays`) is applied when the source is looked up;
       there is no timer. An IM source key is the conversation's
       `thr_src_<hash>` id, which already carries the tenant.
     - **Who wakes an item.** Only the mandate's sender on the mandate's
       connector. Anyone else's message on an owned conversation falls
       through to the ordinary path, gated by participation as before, and
       is not recorded on the item: slice 2 creates no context
       subscriptions. This narrows §5.2's "messages that do not mention the
       agent still reach the item" to the requester's own messages.
     - **Open items are checked too.** Every waking message runs the same
       authority check as revalidation; a requester who no longer passes
       it falls through and the live item is neither cancelled nor
       revoked (§5.4). Without this a sender taken off the allowlist could
       still steer live work.
     - **Revalidation.** The fast path is never taken (the hash stays empty
       until slice 6). `im_*`: the connector is not disabled, failed or
       backing off, and for a group or room whose connector enforces
       participation, a valid route policy still receives the conversation
       and allows the sender (the mention is waived); a DM has no allowlist
       step on the ordinary path, so only the connector is checked. `api`:
       the principal holds an active membership in the tenant (every role
       may chat); an item without a principal fails closed. No slice 2
       route resumes an `api` item, so that check is covered by unit tests
       only. `rule` fails as `basis_unsupported`. A pass at the
       `maxOpenItems` cap does not reopen; the message falls through.
     - **No daemon-wide driver task.** Rounds are driven by whoever woke
       the item: `POST /v1/work-items` and `work_item.open` on a thread of
       their own, an IM wake inline on the connector loop, as an ordinary
       IM turn runs. A wake that finds a round running steers it, or leaves
       its input to that round's driver, which looks again after the round
       settles. The driver admits before it records a round, so a refusal
       records nothing. Not built: the transient-failure retry with backoff
       and pause. Inputs left queued by the hourly cap
       (`maxRoundsPerItemPerHour`) or a refused admission wait for the next
       wake (slice 4's boot scan does not pick them up either; see its
       known gaps).
     - **Replies until the outbox (slice 3)** go through the IM loop's own
       reply path: one reply per round an IM wake ran (the report summary,
       plus its question and options), a fixed line for a round that ended
       without a report, `Added to the work item's running round.` for a
       steered message, and a fixed notice when the cap or admission held
       the round back. A round nobody's IM message started (the first
       round after `work_item.open`, a round driven by the API) has no
       reply channel yet: its report is read through the API and events.
       The IM turn that opened the item acknowledges it in its own answer.
     - **Holds.** A person's input releases every hold (a failed round, a
       question, a blocked or failed report) and the round that reads it
       clears the question (`attention_cleared`); the IM message is how a
       person answers until `/answer` (slice 5). A running round counts as
       "a person spoke since the report" in the stage, so a released item
       shows `working`, not `person/blocked`.
     - **Third-party content.** The mandate's title and scope are the
       round's standing instructions, at the head of its system frame
       (`QueryInput.instructions`, daemon-set, never on the wire). Inputs
       are quoted in the question, and steered, inside
       `<work_item_input kind=… from="requester|other">`, with the tag
       escaped inside the body; the frame says only the requester may
       refine the work and anything else is information.
     - **`work_item.open`** takes `{title, scope, subscribe_origin}`
       (default true) and returns `{itemId, title, basis, stage,
       subscribed}`. It is offered to every turn except a round (rounds run
       on `thr_wi_` threads). An IM turn's origin (connector, conversation,
       author, shape, route policy) is registered in process for the
       turn's lifetime under its tenant and run id. A web or TUI turn opens
       on `api` without a principal: the tool does not see the HTTP
       principal. `source.ref` is the inbound delivery id and
       `receivingPolicyId` the route policy the participation decision
       used; the decision's own id is not recorded (the store does not
       hand it back).
     - **D2** is enforced in one transaction and by a partial unique index:
       a subscribing open on a conversation an open item owns is refused
       (`work_item_conversation_owned`, 409 on the API); an ended owner is
       superseded.
     - **Stop** is `POST /v1/work-items/{id}/stop`, idempotent, on the
       tenant allowlist: subscriptions dropped, a running round's turn
       cancelled, and whatever that round reports the item stays stopped
       (the store keeps `stopped` and `revoked` on settle). Not built:
       `DELETE …/subscriptions/{sub}`, `/answer`, `/retry`, pause.
     - **Events:** `woken` (steered or queued, `resumed`), `stopped`,
       `mandate_revoked` (with a reason), `attention_cleared`; `ended` is
       not sent for a stop or a revocation. `connector.route_outcome_recorded`
       now records the outcome `work_item` for every connector kind.
       Metrics: `kura_work_item_inputs_total{kind, path=steered|queued}`
       and `kura_work_item_mandate_checks_total{result=pass|revoked}`.
     - **Config:** `subscriptionIdleDays` (14) and
       `maxRoundsPerItemPerHour` (20) are accepted keys; `recoverOnBoot`
       is still refused until slice 4.
     - **Known gaps:** a steered input whose turn then fails stays
       `steered` and is not offered again; bots are not told apart
       (`InboundMessage` has no bot flag), so a bot can wake an item only
       if it was the requester.
3. **Outbox with order.**
   - Builds: the outbox table and sender; drop reasons; boot sweep. Rounds
     reply through it.
   - Tests: an order test with a failing first send; a drop on a gone
     conversation; restart mid-retry.
   - As built (slice 3), where it differs from or adds to the above
     (tests: `crates/domains/workitems/tests/outbox.rs` over the manager
     and a real store, two more in `im_wake.rs` over the real
     `MessageLoop`, plus store and API unit tests):
     - **Store.** Migration v12 adds `work_item_outbox`, additive, indexed
       on `(state, next_attempt_at)` and `(item_id, conversation_key,
       seq)`. A row carries the binding copied from the message it answers
       (connector, conversation, channel, peer, the platform message
       replied to) so a retry goes exactly where the first attempt went.
       States: `queued` (not tried), `failed` (tried, still unsent, retried
       at `next_attempt_at`), `sent`, `dropped`; the last two are final. The
       stage reads "an unsent row failed 3 times in a row" from the item's
       activity, so `person/delivery` survives a restart; an ended item has
       no stage, so its stuck reply shows only on its outbox rows.
     - **What goes through it.** Exactly the replies slice 2 sent inline:
       the reply per round an IM wake ran, the steered notice, the
       cap/admission notices. The first round after `work_item.open` and
       API-driven rounds still have no reply channel. The IM hook queues
       the replies, then sends them at once through the loop's own sender,
       lent for the message (`WorkItemInbound::replies`), so a person reads
       them when they did before. If queueing itself fails, the replies go
       back to the loop to send inline as in slice 2 (no retry). A held
       reply no longer makes the IM loop return an error: the inbound is
       `failed` (nothing sent) or `partial`, and the row is the reply's
       record. The connector transcript still gets one outbound
       `connector_messages` row per part sent, whoever sends it.
     - **Order and concurrency.** A row is offered only when it is the
       earliest unsent row of its (item, conversation); the store's due
       query enforces that. One sender per (item, conversation) at a time
       in process (a claim set); single-process only, as §11.
     - **Retries.** 5 s doubling to 300 s, persisted. One sweeper thread per
       daemon (`kura-work-item-outbox`), started with the daemon when the
       feature is on; its first pass is the boot sweep. It sleeps until the
       next head is due (at most 30 s, at least 250 ms) or until poked (a
       sender registered, a flush left rows, a withdrawal). A person's next
       message on the conversation retries the head at once, before the new
       replies, ignoring the backoff.
     - **Senders: decision for D4.** Delivery's `ConnectorReplySender`
       registry is never filled in production (nothing calls
       `ConnectorAdapter::register`; grep), and its trait is the same shape
       as `kura_im::ReplySender`, which every transport already implements.
       So the outbox keeps its own registry of `kura_im::ReplySender`s,
       filled at serve from each connector runtime's new owned
       `reply_sender()` (discord, telegram, slack, matrix; the discord and
       matrix runtimes now hold their transport in an `Arc`). Delivery is
       untouched. A row whose connector has no sender (the runtime is not
       running here) waits untried; that is logged once per connector.
     - **Drops.** `connector_gone`: the row's conversation is a group or
       room whose connector enforces participation, and no valid route
       policy still receives it (checked before every attempt, as the IM
       gate reads it). `conversation_gone` / `forbidden`: the senders
       return plain strings, so these come from documented platform codes
       in the error text (Slack `channel_not_found`, `is_archived`,
       `not_in_channel`, `restricted_action`, `access_denied`; Telegram
       `chat not found`, `bot was blocked by the user`, `bot was kicked`;
       Discord `Unknown Channel`, `Missing Access`/`Permissions`; Matrix
       `M_FORBIDDEN`). Anything else is retried. `withdrawn`: `POST
       /v1/work-items/{id}/outbox/{seq}/withdraw` (tenant allowlist; 409
       once sent or dropped or while being sent). A stopped or ended item's
       queued replies are still sent.
     - **No transport text is kept.** `lastError` is a code
       (`send_failed`, `receive_check_failed`, or the drop reason); logs
       carry codes too. A Telegram request error can carry the bot token in
       its URL.
     - **Observability.** `kura_work_item_outbox_total{result=sent|retry|
       dropped}`, `work_item.outbox_dropped {outboxSeq, reason, attempts,
       connectorId, roundSeq?}` with no body, one `[kura] work item:` line
       per row transition (`-> queued`, `-> failed`, `-> sent`,
       `-> dropped`) and one when a row reaches 3 failures.
     - **Known gaps.** Discord's error text is *unverified* to carry the
       platform message, so its drops may never classify (rows retry until
       withdrawn). A connector removed from config leaves its rows waiting
       forever (no sender); a person can withdraw them. A body over the
       platform limit is split only when the sender reports a length (only
       Discord's does, lent or owned, as for the slice 2 inline path), and a
       failure after some parts resends them all. Matrix
       derives its transaction id from the replied-to message, so several
       replies to one inbound can collapse into one event (pre-existing,
       not changed here). At least once: a crash between the platform's
       accept and `sent` sends again.
4. **Restart recovery.**
   - Builds: the boot scan, same-request-id rerun, the recovery note with
     the outbound log, and `recoverOnBoot`.
   - Tests: kill during a side-effecting call → the resumed round reads
     `INTERRUPTED_CALL_OUTPUT` and the note; a checkpoint older than 24 h
     → fresh round with the note.
   - As built (slice 4), where it differs from or adds to the above
     (tests: `crates/domains/workitems/tests/recovery.rs`, two "processes"
     over one database file, the first killed by leaving its turn stuck
     inside a side-effecting tool after the checkpoint recorded the call as
     started; plus store and note unit tests):
     - **No migration.** The scan reads `work_item_rounds` by `state` (its
       index from v10). A round is "left behind" when it is `running` and
       started before this manager came up (`Manager::boot_at`); a round
       this process starts, or claims for a rerun, never qualifies.
     - **Same round, not a new one.** §6 step 1 ("mark every round
       interrupted") is not done for a rerun: the rerun is the same round
       row, request id, stored question and thread, so the 2.7 turn key
       matches and `TurnResume::claim` loads the checkpoint. The round
       stays `running`; the claim moves its `started_at` to now. A round
       that is *not* rerun is ended `failed` with outcome and error code
       `interrupted`.
     - **Claimed once.** The claim is a compare-and-set in one statement
       (`running`, started before the cutoff, item open), with the
       recovery note written as a consumed `recovery` input of the round
       in the same transaction. Interrupting uses the same guard, so a
       scan never ends a round another scan is rerunning. Two managers
       over one file, or one scan run twice at once, rerun it once.
     - **Resumed or fresh.** `resumed` when a checkpoint for the round's
       request (`turn_checkpoints_for_request`, by tenant, thread and
       request id, since the turn key is the chat layer's) was written in
       the last 24 h and holds rounds or steered messages. Otherwise
       `fresh`; an older checkpoint is deleted once the round is claimed,
       because `TurnResume::claim` does not check age and would still
       load an unpruned one.
     - **The note** goes into the round's instructions, after the mandate
       (`recovery_note`): a restart happened, whether the round resumes
       (a started side-effecting call is marked interrupted) or starts
       again, check before repeating anything with an outside effect, and
       the outbound log: the latest 20 outbox rows, oldest first, each
       body cut at 600 characters, inside `<work_item_outbound seq state>`
       tags with those and the input tags escaped, saying what `sent`,
       `queued`/`failed` (the outbox keeps trying; do not resend) and
       `dropped` mean. *Tradeoff:* the system frame, not the question,
       because the question must stay what it was for the checkpoint to be
       found; the bodies are the item's own replies, framed as a record
       and never an instruction. The note is not a pending input before
       the round's others (§6 step 4): it is recorded as the round's
       consumed `recovery` input, so the item read shows what was said.
     - **Not rerun** (`round_settled` with outcome `interrupted` and a
       `reason`, the round `failed`): the item is stopped or revoked
       (`item_ended`); `recoverOnBoot: false` (`recover_off`); boots reran
       the round twice already (`recovery_limit`, against a round that
       takes the process down on every rerun); the item started
       `maxRoundsPerItemPerHour` other rounds in the past hour
       (`hourly_cap`; the rerun counts as a start, the round's own earlier
       start does not count twice); admission refused it four times, 2 s
       to 8 s apart (`throttled`). On an open item the round's consumed
       and steered inputs go back to `pending` (a round that never
       reported has not acted on them), and the failed round shows the
       item as `person/delivery` until a person writes; that releases the
       hold and the next round reads them.
     - **Off the boot path.** `Manager::start_recovery`, once per manager,
       on the work-items `on_start` hook next to the outbox sweeper, spawns
       `kura-work-item-recovery`; it handles at most 1000 rounds per boot
       (more is logged) with at most two at a time, each through turn
       admission. After a rerun the item is driven on as after any round.
     - **Replies.** The message that woke the round died with the
       process. If the item owns a conversation and has replied there
       before, the rerun's report (and any round driven after it) is
       queued to it through the outbox with that reply's binding; the
       sweeper sends it once the connector's sender registers. Otherwise
       (an API item, or one interrupted on its first IM wake) the report is
       read through the API and events, as for slice 2's API rounds.
     - **Observability.** `kura_work_item_recovered_total{mode=resumed|
       fresh|skipped}`, `kura_work_item_rounds_total{outcome=interrupted}`
       for a round not rerun, one `[kura] work item:` line per round
       (`round N rerun after a restart, resumed`, `round N interrupted by a
       restart, not rerun: <reason>`) plus a count at the start of the
       scan, and the effective `recoverOnBoot` in the boot config line. No
       new event: the design names none; a rerun round settles with the
       usual `round_settled`.
     - **Config:** `recoverOnBoot` (true) is an accepted key.
     - **Not built / known gaps.** §6's "an item that got a person's input
       after its last report starts again" is not built: the scan only
       looks at rounds left running, so inputs left queued by the hourly
       cap or a refused admission still wait for the next wake. A rerun
       may be labelled `resumed` and still run fresh if the chat layer's
       prune removes the checkpoint between the check and the turn (a
       checkpoint just under 24 h old). A crash between the platform's
       accept and the checkpoint's `settled` write is reported as an
       interrupted call, not as sent; the outbound log shows only what the
       outbox sent. A round over the hourly cap or throttled at boot is
       not retried later on its own. A clock that moved back across the
       restart can hide a left-behind round from the scan (it started
       "after" boot). Single process only, as §11.
5. **Attention and approvals.**
   - Builds: the attention field; `/answer`; linking an approval; waking
     on resolve; the queue endpoint; the TUI/web list.
   - Tests: an MCP call needing approval → round `waiting` → resolve →
     the round reruns the call.
6. **Proactive rules.**
   - Builds: `work_rules` and `work_policy`; the shared condition matcher;
     `observe` records, `auto` opens; the `rule` basis in revalidation;
     policy hash and notices.
   - Tests: observe creates nothing and records `would_accept`; auto is
     idempotent per event; disabling a rule revokes on resume and only
     notifies live items.
7. **Webhook source.**
   - Builds: `TargetKind::WorkItem`; a thread header; webhook events as
     inputs and rule candidates.
   - Tests: a signed CI webhook wakes the subscribed item; replay
     protection still holds.

Slices 1–4 deliver feature 1 plus restart recovery. Slice 5 is the
attention queue, slices 6–7 are proactive work, and slice 3 is the
ordered outbound.

## 13. Decisions (user, 2026-11-03)

- **D1 — Chat thread per item:** each item gets its own thread
  (`thr_wi_*`); the IM `thr_src_*` thread is only a subscription source
  and reply channel.
- **D2 — Owners per thread:** at most one owning item per thread in v1,
  with no confirm step in group chats. No hand-back.
- **D3 — Proactive rules:** deterministic conditions only (triage's
  matcher). No model assessment per unaddressed event.
- **D4 — Outbox:** lives in the work-item domain, ordered per
  conversation, sending through delivery's existing connector senders.
  Delivery's own ordering, restart and tenant gaps stay open, tracked
  separately.
- **D5 — Webhook thread identity:** a header the sender sets. The daemon
  does not parse payloads to find the thread.
- **D6 — Shared hosting:** connectors and webhooks stay off the tenant
  allowlist; shared tenants get API- and chat-originated items only.
- **D7 — Defaults:** `subscriptionIdleDays` 14, `maxOpenItems` 50,
  `maxRoundsPerItemPerHour` 20, `recoverOnBoot` true, accepted as
  configuration; revisit with pilot data.
