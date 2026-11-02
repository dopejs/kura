# Work Items: Long-Lived Work, Proactive Rules and Attention

> Status: design, 2026-11-02. Not implemented. Adapts docs 000–003 of
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
  are not on the tenant route allowlist (`surface/api/src/hosting.rs`
  tenant list, around `:100-176`). Only chat, sessions, threads, reminders
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
7. **Off by default.** `entries.workItems.config.enabled: false` keeps
   today's behaviour exactly. The proactive accept mode defaults to
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
  `thr_wi_<hash>`. Its history is the item's history. Each round's query
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
| `person/delivery` | The last round could not start (admission refusal, transient provider error) and is in backoff, or an outbound message failed 3 times in a row | Not delivered |
| `person/approval` | A pending approval is linked as attention | Awaiting approval |
| `person/answer` | `attention` is set | Awaiting answer |
| `person/blocked` · `person/failed` | The last report is `blocked` / `failed`, and no person's input arrived since | Blocked · Failed |
| `queued` | Pending inputs and no round running; waiting for an admission slot | Queued |
| `working` | A round is running | Working |
| `idle` | The last report is `waiting`, or `progress` with no pending input | Waiting for an event |

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
  - Migration v9 `work_items` adds the tables `work_items`,
    `work_item_subscriptions`, `work_item_inputs`, `work_item_rounds`,
    `work_item_outbox`, `work_rules` and `work_policy` (accept mode per
    tenant). The mandate is JSON in `work_items`, never updated.
  - Indexes on `(tenant_id, state)`, on subscriptions by `(source_key)`
    where `dropped_at IS NULL`, and on outbox by
    `(state, next_attempt_at)`.
  - Purely additive. No existing table is altered. The current last
    migration is v8 (`persistence/store/src/migrations.rs:3185`).
- **Existing data:** threads, `agent_turn_checkpoints`, connector messages
  and approvals are read, never rewritten.
  - Checkpoints written before this change carry no item and are never
    claimed by the driver: its request ids start with `wi_`.
  - Existing threads gain no subscription until an item is opened on them.
- **API** (tenant allowlist, every handler filters by tenant):
  - `GET/POST /v1/work-items`
  - `GET /v1/work-items/{id}`, with inputs, rounds, outbox and stage
  - `POST /v1/work-items/{id}/answer`, `/stop`, `/retry`
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
  - `kura_work_item_rounds_total{outcome}`, where `outcome` is a report
    status, `report_unreadable` or `transient_retry`.
  - `kura_work_item_inputs_total{kind, path=steered|queued|recorded_only}`.
  - `kura_work_item_mandate_checks_total{result=pass_fast|pass|revoked}`.
  - `kura_work_rule_matches_total{mode=observe|auto}`.
  - `kura_work_item_outbox_total{result=sent|retry|dropped}`.
  - `kura_work_item_recovered_total`.
- **Events** (bus + schemas): `work_item.opened`, `.woken`,
  `.round_settled`, `.attention_raised`, `.attention_cleared`, `.ended`,
  `.stopped`, `.mandate_revoked`, `.rule_observed`, `.outbox_dropped`.
  Each carries tenant, item, and mandate basis; no message bodies.
- **Logs:** one `[kura] work item:` line per state change, with tenant,
  item, from → to and reason. The effective config is logged once at boot.
- **Traces:** a round's turn trace carries `workItemId` and `roundSeq`.

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
   - Builds: migration v9 (items + rounds + inputs); `POST /v1/work-items`
     runs one round as a turn with the report schema; stage derivation;
     events; schemas.
   - Tests: store round-trip; stage first-match table; tenant isolation
     (two tenants, 404); report validation → `report_unreadable`.
2. **IM subscriptions and wake.**
   - Builds: `work_item.open` tool; subscriptions; the IM hook; steering
     of a running round; resume of an ended item with revalidation
     (`api`, `im_*` bases); stop.
   - Tests: an IM e2e with the test connector: open → idle → a person's
     message wakes the item → reply. A non-owner message falls through. A
     stopped item does not wake. A revoked allowlist entry falls through.
3. **Outbox with order.**
   - Builds: the outbox table and sender; drop reasons; boot sweep. Rounds
     reply through it.
   - Tests: an order test with a failing first send; a drop on a gone
     conversation; restart mid-retry.
4. **Restart recovery.**
   - Builds: the boot scan, same-request-id rerun, the recovery note with
     the outbound log, and `recoverOnBoot`.
   - Tests: kill during a side-effecting call → the resumed round reads
     `INTERRUPTED_CALL_OUTPUT` and the note; a checkpoint older than 24 h
     → fresh round with the note.
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

## 13. Open decisions

- **D1 — Chat thread per item** (`thr_wi_*`) or reuse of the IM
  `thr_src_*` thread. Recommended: own thread (§4).
- **D2 — Owners per thread.** At most one owning item per thread in v1
  (simpler, no hand-back) or dsh's many items per thread with hand-back.
  Also: whether owning a group thread needs a confirm step. Recommended:
  one owner, no confirm.
- **D3 — Model assessment for inferred candidates.** v1 is deterministic
  conditions only. Alternatively, a cheap structured-output turn per
  candidate judges every enabled rule, as dsh does. It costs tokens on
  every unaddressed event.
- **D4 — Where the outbox lives.** Keep it in the work-item domain (this
  doc), or rebuild delivery on it, fixing delivery's ordering, restart
  and tenant gaps at the same time.
- **D5 — Webhook thread identity.** A header the sender sets, or a JSON
  pointer into the payload configured per endpoint. The second parses
  untrusted payloads in the daemon.
- **D6 — Shared hosting.** Whether connectors and webhooks join the
  tenant allowlist as part of this work, or stay operator-only so that
  shared tenants get API- and chat-originated items only.
- **D7 — Defaults.** `subscriptionIdleDays` 14, `maxOpenItems` 50,
  `maxRoundsPerItemPerHour` 20, `recoverOnBoot` true. These are not
  measured; they are dsh's or guesses.
