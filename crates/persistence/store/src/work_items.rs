//! Work items, their rounds, inputs, subscriptions and outbox
//! (docs/harness/work-items.md).
//!
//! Rows only: what a state, a report or a mandate means is `kura-workitems`'
//! business, and it hands this module strings and JSON. Every method takes
//! the caller's tenant and filters on it, so a row of another tenant is
//! indistinguishable from a row that does not exist.

use chrono::{DateTime, Utc};
use rusqlite::{OptionalExtension, Row, params};

use crate::SQLiteStore;
use crate::crud::{now_rfc3339, parse_rfc3339};

/// One `work_items` row.
#[derive(Debug, Clone, PartialEq)]
pub struct WorkItemRecord {
    pub item_id: String,
    pub tenant_id: String,
    pub title: String,
    /// `open` | `ended`.
    pub state: String,
    pub end_reason: Option<String>,
    pub thread_id: String,
    pub provider: String,
    pub model: String,
    /// The last round started; 0 before the first.
    pub seq: i64,
    /// Written once with the item; no method here updates it.
    pub mandate_json: String,
    pub last_report_json: Option<String>,
    pub attention_json: Option<String>,
    pub policy_hash_seen: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// What the item's other rows say about it, read with the item: the facts
/// its stage is derived from besides its own columns.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WorkItemActivity {
    /// The state of the item's latest round, if it has one.
    pub last_round_state: Option<String>,
    pub pending_inputs: i64,
    /// Pending inputs a person wrote: a person's input since the last
    /// report, because a round consumes every input pending when it starts.
    pub pending_person_inputs: i64,
    /// An unsent outbox row of the item failed [`OUTBOX_PROBLEM_ATTEMPTS`]
    /// times in a row (slice 3): the item shows `person/delivery`.
    pub outbox_problem: bool,
}

/// Failures in a row after which an unsent outbox row is a problem a person
/// sees (work-items.md §8). The row keeps being retried. `ITEM_COLUMNS`
/// spells the same number in SQL.
pub const OUTBOX_PROBLEM_ATTEMPTS: i64 = 3;

/// One `work_item_outbox` row: a reply to a conversation (slice 3).
#[derive(Debug, Clone, PartialEq)]
pub struct WorkItemOutboxRecord {
    pub item_id: String,
    /// Ignored by [`SQLiteStore::enqueue_work_item_outbox`], which takes
    /// the item's next.
    pub seq: i64,
    pub tenant_id: String,
    /// The order key with `item_id`: for IM, the conversation's
    /// `thr_src_<hash>` id.
    pub conversation_key: String,
    pub connector_id: String,
    /// The conversation as the connector names it, for the receive check.
    pub conversation_id: String,
    pub channel_id: String,
    pub peer_id: String,
    /// The platform message the reply answers (Slack's thread, Telegram's
    /// reply-to).
    pub reply_to_external_message_id: String,
    /// The conversation is a group or room whose connector enforces
    /// participation: before each attempt a route policy must still
    /// receive it.
    pub participation_gated: bool,
    pub round_seq: Option<i64>,
    /// The inbound delivery the reply answers.
    pub source_ref: String,
    pub body: String,
    /// `queued` | `failed` (retried at `next_attempt_at`) | `sent` | `dropped`.
    pub state: String,
    /// Failed attempts in a row.
    pub attempts: i64,
    pub next_attempt_at: DateTime<Utc>,
    pub last_error: Option<String>,
    /// `connector_gone` | `conversation_gone` | `forbidden` | `withdrawn`.
    pub drop_reason: Option<String>,
    /// The platform's id of the last part sent.
    pub external_message_id: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub sent_at: Option<DateTime<Utc>>,
    pub dropped_at: Option<DateTime<Utc>>,
}

/// One `work_item_rounds` row.
#[derive(Debug, Clone, PartialEq)]
pub struct WorkItemRoundRecord {
    pub item_id: String,
    pub seq: i64,
    pub tenant_id: String,
    pub request_id: String,
    pub query_text: String,
    /// `running` | `settled` | `failed`.
    pub state: String,
    pub outcome: Option<String>,
    pub report_json: Option<String>,
    pub error_code: Option<String>,
    pub turn_id: Option<String>,
    pub started_at: DateTime<Utc>,
    pub finished_at: Option<DateTime<Utc>>,
}

/// One `work_item_inputs` row.
#[derive(Debug, Clone, PartialEq)]
pub struct WorkItemInputRecord {
    pub item_id: String,
    pub seq: i64,
    pub tenant_id: String,
    pub kind: String,
    pub author_is_person: bool,
    pub body: String,
    pub source_ref: String,
    /// `pending` | `steered` | `consumed`.
    pub state: String,
    pub round_seq: Option<i64>,
    pub created_at: DateTime<Utc>,
}

/// One `work_item_subscriptions` row: a source an item follows (slice 2).
#[derive(Debug, Clone, PartialEq)]
pub struct WorkItemSubscriptionRecord {
    pub subscription_id: String,
    pub item_id: String,
    pub tenant_id: String,
    /// `im` in slice 2.
    pub source_kind: String,
    /// Matched by equality. For IM, the conversation's `thr_src_<hash>`.
    pub source_key: String,
    pub connector_id: String,
    pub conversation_id: String,
    /// `owner` | `context`.
    pub role: String,
    pub created_at: DateTime<Utc>,
    pub last_event_at: DateTime<Utc>,
    pub dropped_at: Option<DateTime<Utc>>,
    /// `stopped` | `revoked` | `superseded` | `idle`.
    pub drop_reason: Option<String>,
}

/// How a round ended, and what that does to its item. Applied in one
/// transaction so an item never shows a settled round with a stale report.
#[derive(Debug, Clone, PartialEq)]
pub struct WorkItemRoundSettlement {
    /// `settled` (a report was read, or recorded as unreadable) or `failed`
    /// (the turn ended without one).
    pub round_state: String,
    pub outcome: String,
    pub report_json: Option<String>,
    pub error_code: Option<String>,
    pub turn_id: Option<String>,
    pub finished_at: DateTime<Utc>,
    /// The item's state after the round: `open` or `ended`.
    pub item_state: String,
    pub end_reason: Option<String>,
    /// Replaces the item's last report when set; a failed round leaves it.
    pub last_report_json: Option<String>,
    /// Replaces the item's attention when set; `None` leaves it as it was.
    pub attention_json: Option<String>,
}

const ITEM_COLUMNS: &str = "i.item_id, i.tenant_id, i.title, i.state, i.end_reason, \
     i.thread_id, i.provider, i.model, i.seq, i.mandate_json, i.last_report_json, \
     i.attention_json, i.policy_hash_seen, i.created_at, i.updated_at, \
     (SELECT r.state FROM work_item_rounds r WHERE r.item_id = i.item_id \
        ORDER BY r.seq DESC LIMIT 1), \
     (SELECT COUNT(*) FROM work_item_inputs n WHERE n.item_id = i.item_id \
        AND n.state = 'pending'), \
     (SELECT COUNT(*) FROM work_item_inputs n WHERE n.item_id = i.item_id \
        AND n.state = 'pending' AND n.author_is_person = 1), \
     EXISTS (SELECT 1 FROM work_item_outbox o WHERE o.item_id = i.item_id \
        AND o.state = 'failed' AND o.attempts >= 3)";

const OUTBOX_COLUMNS: &str = "item_id, seq, tenant_id, conversation_key, connector_id, \
     conversation_id, channel_id, peer_id, reply_to_external_message_id, participation_gated, \
     round_seq, source_ref, body, state, attempts, next_attempt_at, last_error, drop_reason, \
     external_message_id, created_at, updated_at, sent_at, dropped_at";

/// The states of a row not yet sent or dropped.
const OUTBOX_UNSENT: &str = "('queued', 'failed')";

const ROUND_COLUMNS: &str = "item_id, seq, tenant_id, request_id, query_text, state, outcome, \
     report_json, error_code, turn_id, started_at, finished_at";

const INPUT_COLUMNS: &str = "item_id, seq, tenant_id, kind, author_is_person, body, source_ref, \
     state, round_seq, created_at";

const SUBSCRIPTION_COLUMNS: &str = "subscription_id, item_id, tenant_id, source_kind, source_key, \
     connector_id, conversation_id, role, created_at, last_event_at, dropped_at, drop_reason";

impl SQLiteStore {
    /// Writes a new item together with its first round, so an item is never
    /// visible without the round that was started for it. Fails if the id
    /// is taken.
    pub fn create_work_item(
        &self,
        item: &WorkItemRecord,
        round: &WorkItemRoundRecord,
    ) -> Result<(), String> {
        let tx = self
            .conn
            .unchecked_transaction()
            .map_err(|e| format!("begin create work item: {e}"))?;
        insert_item(&tx, item)?;
        insert_round(&tx, round)?;
        tx.commit()
            .map_err(|e| format!("commit work item {}: {e}", item.item_id))
    }

    /// The item, if `tenant_id` owns it.
    pub fn get_work_item(
        &self,
        tenant_id: &str,
        item_id: &str,
    ) -> Result<Option<(WorkItemRecord, WorkItemActivity)>, String> {
        self.conn
            .query_row(
                &format!(
                    "SELECT {ITEM_COLUMNS} FROM work_items i \
                     WHERE i.tenant_id = ?1 AND i.item_id = ?2"
                ),
                params![tenant_id, item_id],
                item_from_row,
            )
            .optional()
            .map_err(|e| format!("get work item {item_id}: {e}"))?
            .transpose()
    }

    /// The tenant's items, most recently updated first; `state` narrows to
    /// `open` or `ended`.
    pub fn list_work_items(
        &self,
        tenant_id: &str,
        state: Option<&str>,
        limit: usize,
    ) -> Result<Vec<(WorkItemRecord, WorkItemActivity)>, String> {
        let limit = i64::try_from(limit).unwrap_or(i64::MAX);
        let mut stmt = self
            .conn
            .prepare(&format!(
                "SELECT {ITEM_COLUMNS} FROM work_items i \
                 WHERE i.tenant_id = ?1 AND (?2 IS NULL OR i.state = ?2) \
                 ORDER BY i.updated_at DESC, i.item_id DESC LIMIT ?3"
            ))
            .map_err(|e| format!("prepare list work items: {e}"))?;
        let rows = stmt
            .query_map(params![tenant_id, state, limit], item_from_row)
            .map_err(|e| format!("list work items: {e}"))?;
        let mut items = Vec::new();
        for row in rows {
            items.push(row.map_err(|e| format!("read work item: {e}"))??);
        }
        Ok(items)
    }

    /// The tenant's open items, the least recently updated first: the
    /// attention queue (slice 5) reads them oldest first.
    pub fn list_open_work_items_oldest_first(
        &self,
        tenant_id: &str,
        limit: usize,
    ) -> Result<Vec<(WorkItemRecord, WorkItemActivity)>, String> {
        let limit = i64::try_from(limit).unwrap_or(i64::MAX);
        let mut stmt = self
            .conn
            .prepare(&format!(
                "SELECT {ITEM_COLUMNS} FROM work_items i \
                 WHERE i.tenant_id = ?1 AND i.state = 'open' \
                 ORDER BY i.updated_at ASC, i.item_id ASC LIMIT ?2"
            ))
            .map_err(|e| format!("prepare list open work items: {e}"))?;
        let rows = stmt
            .query_map(params![tenant_id, limit], item_from_row)
            .map_err(|e| format!("list open work items: {e}"))?;
        collect(rows, "work item")
    }

    /// A person answers the item's question (slice 5): when the item is
    /// open and its attention is exactly `attention_json`, the attention is
    /// cleared and `answer` recorded as its next input, pending, in one
    /// transaction (`answer.seq`, `state` and `round_seq` are ignored).
    /// `None` when the item moved on (ended, or its attention changed);
    /// then nothing was written.
    pub fn answer_work_item(
        &self,
        tenant_id: &str,
        item_id: &str,
        attention_json: &str,
        answer: &WorkItemInputRecord,
    ) -> Result<Option<i64>, String> {
        if answer.tenant_id != tenant_id || answer.item_id != item_id {
            return Err(format!("the answer for {item_id} names another item"));
        }
        let tx = self
            .conn
            .unchecked_transaction()
            .map_err(|e| format!("begin answer work item: {e}"))?;
        let changed = tx
            .execute(
                "UPDATE work_items SET attention_json = NULL, updated_at = ?4 \
                 WHERE tenant_id = ?1 AND item_id = ?2 AND state = 'open' \
                   AND attention_json = ?3",
                params![
                    tenant_id,
                    item_id,
                    attention_json,
                    now_rfc3339(&answer.created_at)
                ],
            )
            .map_err(|e| format!("answer work item {item_id}: {e}"))?;
        if changed == 0 {
            return Ok(None);
        }
        let seq = crate::work_item_approvals::append_input(&tx, answer, "pending", None)?;
        tx.commit()
            .map_err(|e| format!("commit answer work item {item_id}: {e}"))?;
        Ok(Some(seq))
    }

    /// Open items the tenant holds, for the per-tenant cap.
    pub fn count_open_work_items(&self, tenant_id: &str) -> Result<i64, String> {
        self.conn
            .query_row(
                "SELECT COUNT(*) FROM work_items WHERE tenant_id = ?1 AND state = 'open'",
                params![tenant_id],
                |row| row.get(0),
            )
            .map_err(|e| format!("count open work items: {e}"))
    }

    /// The item's rounds, oldest first.
    pub fn list_work_item_rounds(
        &self,
        tenant_id: &str,
        item_id: &str,
    ) -> Result<Vec<WorkItemRoundRecord>, String> {
        let mut stmt = self
            .conn
            .prepare(&format!(
                "SELECT {ROUND_COLUMNS} FROM work_item_rounds \
                 WHERE tenant_id = ?1 AND item_id = ?2 ORDER BY seq ASC"
            ))
            .map_err(|e| format!("prepare list work item rounds: {e}"))?;
        let rows = stmt
            .query_map(params![tenant_id, item_id], round_from_row)
            .map_err(|e| format!("list work item rounds {item_id}: {e}"))?;
        let mut rounds = Vec::new();
        for row in rows {
            rounds.push(row.map_err(|e| format!("read work item round: {e}"))??);
        }
        Ok(rounds)
    }

    /// Records an input. Its `seq` must be the item's next; a taken one
    /// fails rather than overwriting what was said.
    pub fn insert_work_item_input(&self, input: &WorkItemInputRecord) -> Result<(), String> {
        self.conn
            .execute(
                &format!(
                    "INSERT INTO work_item_inputs ({INPUT_COLUMNS}) \
                     SELECT ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10 \
                     WHERE EXISTS (SELECT 1 FROM work_items \
                        WHERE item_id = ?1 AND tenant_id = ?3)"
                ),
                params![
                    input.item_id,
                    input.seq,
                    input.tenant_id,
                    input.kind,
                    input.author_is_person,
                    input.body,
                    input.source_ref,
                    input.state,
                    input.round_seq,
                    now_rfc3339(&input.created_at),
                ],
            )
            .map_err(|e| {
                format!(
                    "insert work item input {}#{}: {e}",
                    input.item_id, input.seq
                )
            })
            .and_then(|written| match written {
                1 => Ok(()),
                _ => Err(format!(
                    "insert work item input {}#{}: no such item for the tenant",
                    input.item_id, input.seq
                )),
            })
    }

    /// The item's inputs, oldest first.
    pub fn list_work_item_inputs(
        &self,
        tenant_id: &str,
        item_id: &str,
    ) -> Result<Vec<WorkItemInputRecord>, String> {
        let mut stmt = self
            .conn
            .prepare(&format!(
                "SELECT {INPUT_COLUMNS} FROM work_item_inputs \
                 WHERE tenant_id = ?1 AND item_id = ?2 ORDER BY seq ASC"
            ))
            .map_err(|e| format!("prepare list work item inputs: {e}"))?;
        let rows = stmt
            .query_map(params![tenant_id, item_id], input_from_row)
            .map_err(|e| format!("list work item inputs {item_id}: {e}"))?;
        let mut inputs = Vec::new();
        for row in rows {
            inputs.push(row.map_err(|e| format!("read work item input: {e}"))??);
        }
        Ok(inputs)
    }

    /// Ends a running round and applies what it did to its item, in one
    /// transaction. `false` when the round is not running (already settled,
    /// or not the tenant's): a settlement is applied at most once.
    pub fn settle_work_item_round(
        &self,
        tenant_id: &str,
        item_id: &str,
        seq: i64,
        settlement: &WorkItemRoundSettlement,
    ) -> Result<bool, String> {
        let tx = self
            .conn
            .unchecked_transaction()
            .map_err(|e| format!("begin settle work item round: {e}"))?;
        let finished = now_rfc3339(&settlement.finished_at);
        let changed = tx
            .execute(
                r#"UPDATE work_item_rounds SET
                    state = ?4, outcome = ?5, report_json = ?6, error_code = ?7,
                    turn_id = ?8, finished_at = ?9
                WHERE tenant_id = ?1 AND item_id = ?2 AND seq = ?3 AND state = 'running'"#,
                params![
                    tenant_id,
                    item_id,
                    seq,
                    settlement.round_state,
                    settlement.outcome,
                    settlement.report_json,
                    settlement.error_code,
                    settlement.turn_id,
                    finished,
                ],
            )
            .map_err(|e| format!("settle work item round {item_id}#{seq}: {e}"))?;
        if changed == 0 {
            return Ok(false);
        }
        // A stopped or revoked item stays ended whatever the round that was
        // still running reports: neither end can be undone by a round.
        tx.execute(
            r#"UPDATE work_items SET
                state = CASE WHEN end_reason IN ('stopped', 'revoked') THEN state ELSE ?3 END,
                end_reason = CASE WHEN end_reason IN ('stopped', 'revoked')
                    THEN end_reason ELSE ?4 END,
                last_report_json = COALESCE(?5, last_report_json),
                attention_json = COALESCE(?6, attention_json),
                updated_at = ?7
            WHERE tenant_id = ?1 AND item_id = ?2"#,
            params![
                tenant_id,
                item_id,
                settlement.item_state,
                settlement.end_reason,
                settlement.last_report_json,
                settlement.attention_json,
                finished,
            ],
        )
        .map_err(|e| format!("apply round to work item {item_id}: {e}"))?;
        tx.commit()
            .map_err(|e| format!("commit settle work item round {item_id}#{seq}: {e}"))?;
        Ok(true)
    }

    /// [`Self::create_work_item`] for an item that owns a source (D2: one
    /// live owner per source). In the same transaction an owner whose item
    /// has ended is superseded (its subscription dropped), while an owner
    /// whose item is open keeps the source and nothing is written:
    /// `Ok(Err(owner item id))`.
    pub fn create_work_item_with_owner(
        &self,
        item: &WorkItemRecord,
        round: &WorkItemRoundRecord,
        owner: &WorkItemSubscriptionRecord,
    ) -> Result<Result<(), String>, String> {
        let tx = self
            .conn
            .unchecked_transaction()
            .map_err(|e| format!("begin create owned work item: {e}"))?;
        let live_owner: Option<String> = tx
            .query_row(
                "SELECT s.item_id FROM work_item_subscriptions s \
                 JOIN work_items i ON i.item_id = s.item_id \
                 WHERE s.tenant_id = ?1 AND s.source_key = ?2 AND s.role = 'owner' \
                   AND s.dropped_at IS NULL AND i.state = 'open'",
                params![owner.tenant_id, owner.source_key],
                |row| row.get(0),
            )
            .optional()
            .map_err(|e| format!("find owner of {}: {e}", owner.source_key))?;
        if let Some(owner_item) = live_owner {
            return Ok(Err(owner_item));
        }
        tx.execute(
            "UPDATE work_item_subscriptions SET dropped_at = ?3, drop_reason = 'superseded' \
             WHERE tenant_id = ?1 AND source_key = ?2 AND role = 'owner' AND dropped_at IS NULL",
            params![
                owner.tenant_id,
                owner.source_key,
                now_rfc3339(&owner.created_at)
            ],
        )
        .map_err(|e| format!("supersede owner of {}: {e}", owner.source_key))?;
        insert_item(&tx, item)?;
        insert_round(&tx, round)?;
        insert_subscription(&tx, owner)?;
        tx.commit()
            .map_err(|e| format!("commit owned work item {}: {e}", item.item_id))?;
        Ok(Ok(()))
    }

    /// The live subscriptions of a source, owner first. Those that saw no
    /// event before `idle_before` are dropped as `idle` first, so an
    /// abandoned conversation stops being read without a timer.
    pub fn live_work_item_subscriptions_for_source(
        &self,
        tenant_id: &str,
        source_key: &str,
        idle_before: Option<DateTime<Utc>>,
    ) -> Result<Vec<WorkItemSubscriptionRecord>, String> {
        if let Some(cutoff) = idle_before {
            self.conn
                .execute(
                    "UPDATE work_item_subscriptions SET dropped_at = ?4, drop_reason = 'idle' \
                     WHERE tenant_id = ?1 AND source_key = ?2 AND dropped_at IS NULL \
                       AND last_event_at < ?3",
                    params![
                        tenant_id,
                        source_key,
                        now_rfc3339(&cutoff),
                        now_rfc3339(&Utc::now())
                    ],
                )
                .map_err(|e| format!("expire idle subscriptions of {source_key}: {e}"))?;
        }
        let mut stmt = self
            .conn
            .prepare(&format!(
                "SELECT {SUBSCRIPTION_COLUMNS} FROM work_item_subscriptions \
                 WHERE tenant_id = ?1 AND source_key = ?2 AND dropped_at IS NULL \
                 ORDER BY CASE role WHEN 'owner' THEN 0 ELSE 1 END, created_at ASC"
            ))
            .map_err(|e| format!("prepare live subscriptions: {e}"))?;
        let rows = stmt
            .query_map(params![tenant_id, source_key], subscription_from_row)
            .map_err(|e| format!("live subscriptions of {source_key}: {e}"))?;
        collect(rows, "work item subscription")
    }

    /// Every subscription the item ever had, live or dropped, oldest first.
    pub fn list_work_item_subscriptions(
        &self,
        tenant_id: &str,
        item_id: &str,
    ) -> Result<Vec<WorkItemSubscriptionRecord>, String> {
        let mut stmt = self
            .conn
            .prepare(&format!(
                "SELECT {SUBSCRIPTION_COLUMNS} FROM work_item_subscriptions \
                 WHERE tenant_id = ?1 AND item_id = ?2 \
                 ORDER BY created_at ASC, subscription_id ASC"
            ))
            .map_err(|e| format!("prepare list subscriptions: {e}"))?;
        let rows = stmt
            .query_map(params![tenant_id, item_id], subscription_from_row)
            .map_err(|e| format!("list subscriptions of {item_id}: {e}"))?;
        collect(rows, "work item subscription")
    }

    /// Records that a live subscription saw an event, so it is not idle.
    pub fn touch_work_item_subscription(
        &self,
        tenant_id: &str,
        subscription_id: &str,
        at: DateTime<Utc>,
    ) -> Result<(), String> {
        self.conn
            .execute(
                "UPDATE work_item_subscriptions SET last_event_at = ?3 \
                 WHERE tenant_id = ?1 AND subscription_id = ?2 AND dropped_at IS NULL",
                params![tenant_id, subscription_id, now_rfc3339(&at)],
            )
            .map_err(|e| format!("touch subscription {subscription_id}: {e}"))?;
        Ok(())
    }

    /// Records an input as the item's next one and returns its seq. The
    /// seq is taken in the same transaction, so two writers cannot both
    /// take it; `input.seq` is ignored.
    pub fn append_work_item_input(&self, input: &WorkItemInputRecord) -> Result<i64, String> {
        let tx = self
            .conn
            .unchecked_transaction()
            .map_err(|e| format!("begin append work item input: {e}"))?;
        let seq: i64 = tx
            .query_row(
                "SELECT COALESCE(MAX(seq), 0) + 1 FROM work_item_inputs WHERE item_id = ?1",
                params![input.item_id],
                |row| row.get(0),
            )
            .map_err(|e| format!("next input seq of {}: {e}", input.item_id))?;
        let written = tx
            .execute(
                &format!(
                    "INSERT INTO work_item_inputs ({INPUT_COLUMNS}) \
                     SELECT ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10 \
                     WHERE EXISTS (SELECT 1 FROM work_items \
                        WHERE item_id = ?1 AND tenant_id = ?3)"
                ),
                params![
                    input.item_id,
                    seq,
                    input.tenant_id,
                    input.kind,
                    input.author_is_person,
                    input.body,
                    input.source_ref,
                    input.state,
                    input.round_seq,
                    now_rfc3339(&input.created_at),
                ],
            )
            .map_err(|e| format!("append work item input {}: {e}", input.item_id))?;
        if written != 1 {
            return Err(format!(
                "append work item input {}: no such item for the tenant",
                input.item_id
            ));
        }
        tx.commit()
            .map_err(|e| format!("commit work item input {}#{seq}: {e}", input.item_id))?;
        Ok(seq)
    }

    /// Starts the item's next round. `round.seq` must be the item's seq + 1,
    /// the item open, and no round of it running. In one transaction the
    /// pending inputs up to `upto_input_seq` become consumed by the round
    /// (they are what its query quotes) and, with `clear_attention`, the
    /// item's question is cleared because a person answered it. `false`
    /// when the item moved on in between; then nothing was written.
    pub fn start_work_item_round(
        &self,
        round: &WorkItemRoundRecord,
        upto_input_seq: i64,
        clear_attention: bool,
    ) -> Result<bool, String> {
        let tx = self
            .conn
            .unchecked_transaction()
            .map_err(|e| format!("begin start work item round: {e}"))?;
        let changed = tx
            .execute(
                "UPDATE work_items SET seq = ?3, updated_at = ?4, \
                    attention_json = CASE WHEN ?5 THEN NULL ELSE attention_json END \
                 WHERE tenant_id = ?1 AND item_id = ?2 AND state = 'open' AND seq = ?3 - 1 \
                   AND NOT EXISTS (SELECT 1 FROM work_item_rounds r \
                        WHERE r.item_id = ?2 AND r.state = 'running')",
                params![
                    round.tenant_id,
                    round.item_id,
                    round.seq,
                    now_rfc3339(&round.started_at),
                    clear_attention
                ],
            )
            .map_err(|e| format!("start round {}#{}: {e}", round.item_id, round.seq))?;
        if changed == 0 {
            return Ok(false);
        }
        tx.execute(
            "UPDATE work_item_inputs SET state = 'consumed', round_seq = ?3 \
             WHERE tenant_id = ?1 AND item_id = ?2 AND state = 'pending' AND seq <= ?4",
            params![round.tenant_id, round.item_id, round.seq, upto_input_seq],
        )
        .map_err(|e| format!("consume inputs of {}#{}: {e}", round.item_id, round.seq))?;
        insert_round(&tx, round)?;
        tx.commit()
            .map_err(|e| format!("commit start round {}#{}: {e}", round.item_id, round.seq))?;
        Ok(true)
    }

    /// Rounds of the item started at or after `since`, for the hourly cap.
    pub fn count_work_item_rounds_since(
        &self,
        tenant_id: &str,
        item_id: &str,
        since: DateTime<Utc>,
    ) -> Result<i64, String> {
        self.conn
            .query_row(
                "SELECT COUNT(*) FROM work_item_rounds \
                 WHERE tenant_id = ?1 AND item_id = ?2 AND started_at >= ?3",
                params![tenant_id, item_id, now_rfc3339(&since)],
                |row| row.get(0),
            )
            .map_err(|e| format!("count rounds of {item_id}: {e}"))
    }

    /// Opens again an item that ended on its own report (`completed` or
    /// `dropped`). `false` for any other item: a stopped or revoked item
    /// never reopens.
    pub fn reopen_work_item(
        &self,
        tenant_id: &str,
        item_id: &str,
        at: DateTime<Utc>,
    ) -> Result<bool, String> {
        let changed = self
            .conn
            .execute(
                "UPDATE work_items SET state = 'open', end_reason = NULL, updated_at = ?3 \
                 WHERE tenant_id = ?1 AND item_id = ?2 AND state = 'ended' \
                   AND end_reason IN ('completed', 'dropped')",
                params![tenant_id, item_id, now_rfc3339(&at)],
            )
            .map_err(|e| format!("reopen work item {item_id}: {e}"))?;
        Ok(changed == 1)
    }

    /// Ends the item for good with `reason` and drops its live
    /// subscriptions with the same reason, in one transaction. `stopped`
    /// applies to any item not already stopped; `revoked` only to an item
    /// that ended on its own report. `false` when it did not apply.
    pub fn end_work_item_finally(
        &self,
        tenant_id: &str,
        item_id: &str,
        reason: &str,
        at: DateTime<Utc>,
    ) -> Result<bool, String> {
        let guard = match reason {
            "stopped" => "(end_reason IS NULL OR end_reason <> 'stopped')",
            "revoked" => "state = 'ended' AND end_reason IN ('completed', 'dropped')",
            other => return Err(format!("{other:?} is not a final end reason")),
        };
        let tx = self
            .conn
            .unchecked_transaction()
            .map_err(|e| format!("begin end work item: {e}"))?;
        let when = now_rfc3339(&at);
        let changed = tx
            .execute(
                &format!(
                    "UPDATE work_items SET state = 'ended', end_reason = ?3, updated_at = ?4 \
                     WHERE tenant_id = ?1 AND item_id = ?2 AND {guard}"
                ),
                params![tenant_id, item_id, reason, when],
            )
            .map_err(|e| format!("end work item {item_id}: {e}"))?;
        if changed == 0 {
            return Ok(false);
        }
        tx.execute(
            "UPDATE work_item_subscriptions SET dropped_at = ?3, drop_reason = ?4 \
             WHERE tenant_id = ?1 AND item_id = ?2 AND dropped_at IS NULL",
            params![tenant_id, item_id, when, reason],
        )
        .map_err(|e| format!("drop subscriptions of {item_id}: {e}"))?;
        tx.commit()
            .map_err(|e| format!("commit end work item {item_id}: {e}"))?;
        Ok(true)
    }

    /// Queues replies as the item's next outbox rows, in the given order,
    /// and returns their seqs. The seqs are taken in the same transaction,
    /// so two writers cannot both take one; each row's `seq` is ignored.
    /// Writes nothing if any row names an item the tenant does not hold.
    pub fn enqueue_work_item_outbox(
        &self,
        rows: &[WorkItemOutboxRecord],
    ) -> Result<Vec<i64>, String> {
        let tx = self
            .conn
            .unchecked_transaction()
            .map_err(|e| format!("begin enqueue work item outbox: {e}"))?;
        let mut seqs = Vec::with_capacity(rows.len());
        for row in rows {
            let seq: i64 = tx
                .query_row(
                    "SELECT COALESCE(MAX(seq), 0) + 1 FROM work_item_outbox WHERE item_id = ?1",
                    params![row.item_id],
                    |r| r.get(0),
                )
                .map_err(|e| format!("next outbox seq of {}: {e}", row.item_id))?;
            let written = tx
                .execute(
                    &format!(
                        "INSERT INTO work_item_outbox ({OUTBOX_COLUMNS}) \
                         SELECT ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, \
                                ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?23 \
                         WHERE EXISTS (SELECT 1 FROM work_items \
                            WHERE item_id = ?1 AND tenant_id = ?3)"
                    ),
                    params![
                        row.item_id,
                        seq,
                        row.tenant_id,
                        row.conversation_key,
                        row.connector_id,
                        row.conversation_id,
                        row.channel_id,
                        row.peer_id,
                        row.reply_to_external_message_id,
                        row.participation_gated,
                        row.round_seq,
                        row.source_ref,
                        row.body,
                        row.state,
                        row.attempts,
                        now_rfc3339(&row.next_attempt_at),
                        row.last_error,
                        row.drop_reason,
                        row.external_message_id,
                        now_rfc3339(&row.created_at),
                        now_rfc3339(&row.updated_at),
                        row.sent_at.as_ref().map(now_rfc3339),
                        row.dropped_at.as_ref().map(now_rfc3339),
                    ],
                )
                .map_err(|e| format!("enqueue outbox row of {}: {e}", row.item_id))?;
            if written != 1 {
                return Err(format!(
                    "enqueue outbox row of {}: no such item for the tenant",
                    row.item_id
                ));
            }
            seqs.push(seq);
        }
        tx.commit()
            .map_err(|e| format!("commit work item outbox: {e}"))?;
        Ok(seqs)
    }

    /// The item's outbox, oldest first.
    pub fn list_work_item_outbox(
        &self,
        tenant_id: &str,
        item_id: &str,
    ) -> Result<Vec<WorkItemOutboxRecord>, String> {
        let mut stmt = self
            .conn
            .prepare(&format!(
                "SELECT {OUTBOX_COLUMNS} FROM work_item_outbox \
                 WHERE tenant_id = ?1 AND item_id = ?2 ORDER BY seq ASC"
            ))
            .map_err(|e| format!("prepare list work item outbox: {e}"))?;
        let rows = stmt
            .query_map(params![tenant_id, item_id], outbox_from_row)
            .map_err(|e| format!("list outbox of {item_id}: {e}"))?;
        collect(rows, "work item outbox row")
    }

    /// The row of (item, conversation) to send next, due or not: its
    /// earliest unsent row, so every earlier one is sent or dropped.
    pub fn work_item_outbox_head(
        &self,
        tenant_id: &str,
        item_id: &str,
        conversation_key: &str,
    ) -> Result<Option<WorkItemOutboxRecord>, String> {
        self.conn
            .query_row(
                &format!(
                    "SELECT {OUTBOX_COLUMNS} FROM work_item_outbox \
                     WHERE tenant_id = ?1 AND item_id = ?2 AND conversation_key = ?3 \
                       AND state IN {OUTBOX_UNSENT} \
                     ORDER BY seq ASC LIMIT 1"
                ),
                params![tenant_id, item_id, conversation_key],
                outbox_from_row,
            )
            .optional()
            .map_err(|e| format!("outbox head of {item_id}: {e}"))?
            .transpose()
    }

    /// Every tenant's outbox rows due at or before `due_by` that are the
    /// earliest unsent row of their (item, conversation), so a row never
    /// overtakes an earlier one; earliest due first. A row behind an unsent
    /// one is never offered, however long it has been due. The one
    /// daemon-wide read in this module: the outbox sweeper serves every
    /// tenant, and each row carries its own.
    pub fn due_work_item_outbox(
        &self,
        due_by: DateTime<Utc>,
        limit: usize,
    ) -> Result<Vec<WorkItemOutboxRecord>, String> {
        let limit = i64::try_from(limit).unwrap_or(i64::MAX);
        let mut stmt = self
            .conn
            .prepare(&format!(
                "SELECT {OUTBOX_COLUMNS} FROM work_item_outbox o \
                 WHERE o.state IN {OUTBOX_UNSENT} AND o.next_attempt_at <= ?1 \
                   AND NOT EXISTS (SELECT 1 FROM work_item_outbox p \
                        WHERE p.item_id = o.item_id AND p.conversation_key = o.conversation_key \
                          AND p.seq < o.seq AND p.state IN {OUTBOX_UNSENT}) \
                 ORDER BY o.next_attempt_at ASC, o.item_id ASC, o.seq ASC LIMIT ?2"
            ))
            .map_err(|e| format!("prepare due work item outbox: {e}"))?;
        let rows = stmt
            .query_map(params![now_rfc3339(&due_by), limit], outbox_from_row)
            .map_err(|e| format!("due work item outbox: {e}"))?;
        collect(rows, "work item outbox row")
    }

    /// Records that an unsent row was delivered. `false` when it is no
    /// longer unsent (a person withdrew it meanwhile).
    pub fn mark_work_item_outbox_sent(
        &self,
        tenant_id: &str,
        item_id: &str,
        seq: i64,
        external_message_id: &str,
        at: DateTime<Utc>,
    ) -> Result<bool, String> {
        let changed = self
            .conn
            .execute(
                &format!(
                    "UPDATE work_item_outbox SET state = 'sent', external_message_id = ?4, \
                        sent_at = ?5, updated_at = ?5 \
                     WHERE tenant_id = ?1 AND item_id = ?2 AND seq = ?3 \
                       AND state IN {OUTBOX_UNSENT}"
                ),
                params![
                    tenant_id,
                    item_id,
                    seq,
                    external_message_id,
                    now_rfc3339(&at)
                ],
            )
            .map_err(|e| format!("mark outbox {item_id}#{seq} sent: {e}"))?;
        Ok(changed == 1)
    }

    /// Records a failed attempt on an unsent row: one more failure in a
    /// row, retried at `next_attempt_at`. Returns the failures in a row,
    /// or `None` when the row is no longer unsent.
    pub fn mark_work_item_outbox_failed(
        &self,
        tenant_id: &str,
        item_id: &str,
        seq: i64,
        error: &str,
        next_attempt_at: DateTime<Utc>,
        at: DateTime<Utc>,
    ) -> Result<Option<i64>, String> {
        self.conn
            .query_row(
                &format!(
                    "UPDATE work_item_outbox SET state = 'failed', attempts = attempts + 1, \
                        last_error = ?4, next_attempt_at = ?5, updated_at = ?6 \
                     WHERE tenant_id = ?1 AND item_id = ?2 AND seq = ?3 \
                       AND state IN {OUTBOX_UNSENT} \
                     RETURNING attempts"
                ),
                params![
                    tenant_id,
                    item_id,
                    seq,
                    error,
                    now_rfc3339(&next_attempt_at),
                    now_rfc3339(&at)
                ],
                |row| row.get(0),
            )
            .optional()
            .map_err(|e| format!("mark outbox {item_id}#{seq} failed: {e}"))
    }

    /// Drops an unsent row for good with `reason`; `error`, when given,
    /// replaces `last_error`. `false` when it is no longer unsent.
    pub fn drop_work_item_outbox(
        &self,
        tenant_id: &str,
        item_id: &str,
        seq: i64,
        reason: &str,
        error: Option<&str>,
        at: DateTime<Utc>,
    ) -> Result<bool, String> {
        let changed = self
            .conn
            .execute(
                &format!(
                    "UPDATE work_item_outbox SET state = 'dropped', drop_reason = ?4, \
                        last_error = COALESCE(?5, last_error), dropped_at = ?6, updated_at = ?6 \
                     WHERE tenant_id = ?1 AND item_id = ?2 AND seq = ?3 \
                       AND state IN {OUTBOX_UNSENT}"
                ),
                params![tenant_id, item_id, seq, reason, error, now_rfc3339(&at)],
            )
            .map_err(|e| format!("drop outbox {item_id}#{seq}: {e}"))?;
        Ok(changed == 1)
    }

    /// Every tenant's rounds still `running` that started before
    /// `started_before`, oldest first: at boot, the rounds a previous
    /// process left behind (slice 4). A round this process starts, or
    /// claims for a rerun, starts at or after it and is never listed. The
    /// other daemon-wide read here besides the outbox sweep: the boot scan
    /// serves every tenant, and each row carries its own.
    pub fn list_interrupted_work_item_rounds(
        &self,
        started_before: DateTime<Utc>,
        limit: usize,
    ) -> Result<Vec<WorkItemRoundRecord>, String> {
        let limit = i64::try_from(limit).unwrap_or(i64::MAX);
        let mut stmt = self
            .conn
            .prepare(&format!(
                "SELECT {ROUND_COLUMNS} FROM work_item_rounds \
                 WHERE state = 'running' AND started_at < ?1 \
                 ORDER BY started_at ASC, item_id ASC, seq ASC LIMIT ?2"
            ))
            .map_err(|e| format!("prepare interrupted work item rounds: {e}"))?;
        let rows = stmt
            .query_map(params![now_rfc3339(&started_before), limit], round_from_row)
            .map_err(|e| format!("interrupted work item rounds: {e}"))?;
        collect(rows, "work item round")
    }

    /// Recovery inputs recorded for the round: how many times a boot has
    /// rerun it.
    pub fn count_work_item_round_recoveries(
        &self,
        tenant_id: &str,
        item_id: &str,
        seq: i64,
    ) -> Result<i64, String> {
        self.conn
            .query_row(
                "SELECT COUNT(*) FROM work_item_inputs \
                 WHERE tenant_id = ?1 AND item_id = ?2 AND round_seq = ?3 AND kind = 'recovery'",
                params![tenant_id, item_id, seq],
                |row| row.get(0),
            )
            .map_err(|e| format!("count recoveries of {item_id}#{seq}: {e}"))
    }

    /// Claims an interrupted round for its rerun: the compare-and-set that
    /// lets only one scan rerun it. The round must still be `running`, have
    /// started before `started_before` (a claim moves `started_at` to `at`,
    /// so a claimed round no longer qualifies) and its item be open. In one
    /// transaction the round's `started_at` becomes `at` and `note` is
    /// recorded as the item's next input, consumed by the round (`note.seq`,
    /// `state` and `round_seq` are ignored). Returns the note's seq, or
    /// `None` when the round was not claimable; then nothing was written.
    pub fn claim_work_item_round_recovery(
        &self,
        tenant_id: &str,
        item_id: &str,
        seq: i64,
        started_before: DateTime<Utc>,
        note: &WorkItemInputRecord,
        at: DateTime<Utc>,
    ) -> Result<Option<i64>, String> {
        let tx = self
            .conn
            .unchecked_transaction()
            .map_err(|e| format!("begin claim work item round: {e}"))?;
        let when = now_rfc3339(&at);
        let changed = tx
            .execute(
                "UPDATE work_item_rounds SET started_at = ?5 \
                 WHERE tenant_id = ?1 AND item_id = ?2 AND seq = ?3 AND state = 'running' \
                   AND started_at < ?4 \
                   AND EXISTS (SELECT 1 FROM work_items i \
                        WHERE i.tenant_id = ?1 AND i.item_id = ?2 AND i.state = 'open')",
                params![tenant_id, item_id, seq, now_rfc3339(&started_before), when],
            )
            .map_err(|e| format!("claim round {item_id}#{seq}: {e}"))?;
        if changed == 0 {
            return Ok(None);
        }
        let input_seq: i64 = tx
            .query_row(
                "SELECT COALESCE(MAX(seq), 0) + 1 FROM work_item_inputs WHERE item_id = ?1",
                params![item_id],
                |row| row.get(0),
            )
            .map_err(|e| format!("next input seq of {item_id}: {e}"))?;
        tx.execute(
            &format!(
                "INSERT INTO work_item_inputs ({INPUT_COLUMNS}) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 'consumed', ?8, ?9)"
            ),
            params![
                item_id,
                input_seq,
                tenant_id,
                note.kind,
                note.author_is_person,
                note.body,
                note.source_ref,
                seq,
                when,
            ],
        )
        .map_err(|e| format!("record recovery of {item_id}#{seq}: {e}"))?;
        tx.execute(
            "UPDATE work_items SET updated_at = ?3 WHERE tenant_id = ?1 AND item_id = ?2",
            params![tenant_id, item_id, when],
        )
        .map_err(|e| format!("touch work item {item_id}: {e}"))?;
        tx.commit()
            .map_err(|e| format!("commit claim round {item_id}#{seq}: {e}"))?;
        Ok(Some(input_seq))
    }

    /// Ends an interrupted round without a rerun: `failed`, with outcome
    /// and error code `interrupted`. Guarded as
    /// [`Self::claim_work_item_round_recovery`] is, so a round claimed for a
    /// rerun is never interrupted under it. With `release_inputs`, what the
    /// round had consumed or been steered (never a recovery note) is
    /// pending again, so the item's next round reads it: a round that never
    /// reported cannot be said to have acted on it. `false` when it did not
    /// apply.
    pub fn interrupt_work_item_round(
        &self,
        tenant_id: &str,
        item_id: &str,
        seq: i64,
        started_before: DateTime<Utc>,
        release_inputs: bool,
        at: DateTime<Utc>,
    ) -> Result<bool, String> {
        let tx = self
            .conn
            .unchecked_transaction()
            .map_err(|e| format!("begin interrupt work item round: {e}"))?;
        let when = now_rfc3339(&at);
        let changed = tx
            .execute(
                "UPDATE work_item_rounds SET state = 'failed', outcome = 'interrupted', \
                    error_code = 'interrupted', finished_at = ?5 \
                 WHERE tenant_id = ?1 AND item_id = ?2 AND seq = ?3 AND state = 'running' \
                   AND started_at < ?4",
                params![tenant_id, item_id, seq, now_rfc3339(&started_before), when],
            )
            .map_err(|e| format!("interrupt round {item_id}#{seq}: {e}"))?;
        if changed == 0 {
            return Ok(false);
        }
        if release_inputs {
            tx.execute(
                "UPDATE work_item_inputs SET state = 'pending', round_seq = NULL \
                 WHERE tenant_id = ?1 AND item_id = ?2 AND round_seq = ?3 \
                   AND state IN ('consumed', 'steered') AND kind <> 'recovery'",
                params![tenant_id, item_id, seq],
            )
            .map_err(|e| format!("release inputs of {item_id}#{seq}: {e}"))?;
        }
        tx.execute(
            "UPDATE work_items SET updated_at = ?3 WHERE tenant_id = ?1 AND item_id = ?2",
            params![tenant_id, item_id, when],
        )
        .map_err(|e| format!("touch work item {item_id}: {e}"))?;
        tx.commit()
            .map_err(|e| format!("commit interrupt round {item_id}#{seq}: {e}"))?;
        Ok(true)
    }
}

fn outbox_from_row(row: &Row<'_>) -> Parsed<WorkItemOutboxRecord> {
    let (next_attempt_at, created_at, updated_at) =
        match (time_at(row, 15)?, time_at(row, 19)?, time_at(row, 20)?) {
            (Ok(next), Ok(created), Ok(updated)) => (next, created, updated),
            (Err(err), _, _) | (_, Err(err), _) | (_, _, Err(err)) => return Ok(Err(err)),
        };
    let (sent_at, dropped_at) = match (optional_time_at(row, 21)?, optional_time_at(row, 22)?) {
        (Ok(sent), Ok(dropped)) => (sent, dropped),
        (Err(err), _) | (_, Err(err)) => return Ok(Err(err)),
    };
    Ok(Ok(WorkItemOutboxRecord {
        item_id: row.get(0)?,
        seq: row.get(1)?,
        tenant_id: row.get(2)?,
        conversation_key: row.get(3)?,
        connector_id: row.get(4)?,
        conversation_id: row.get(5)?,
        channel_id: row.get(6)?,
        peer_id: row.get(7)?,
        reply_to_external_message_id: row.get(8)?,
        participation_gated: row.get(9)?,
        round_seq: row.get(10)?,
        source_ref: row.get(11)?,
        body: row.get(12)?,
        state: row.get(13)?,
        attempts: row.get(14)?,
        next_attempt_at,
        last_error: row.get(16)?,
        drop_reason: row.get(17)?,
        external_message_id: row.get(18)?,
        created_at,
        updated_at,
        sent_at,
        dropped_at,
    }))
}

fn optional_time_at(row: &Row<'_>, index: usize) -> Parsed<Option<DateTime<Utc>>> {
    Ok(row
        .get::<_, Option<String>>(index)?
        .as_deref()
        .map(parse_rfc3339)
        .transpose())
}

fn insert_item(conn: &rusqlite::Connection, item: &WorkItemRecord) -> Result<(), String> {
    conn.execute(
        r#"INSERT INTO work_items
            (item_id, tenant_id, title, state, end_reason, thread_id, provider, model, seq,
             mandate_json, last_report_json, attention_json, policy_hash_seen,
             created_at, updated_at)
        VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)"#,
        params![
            item.item_id,
            item.tenant_id,
            item.title,
            item.state,
            item.end_reason,
            item.thread_id,
            item.provider,
            item.model,
            item.seq,
            item.mandate_json,
            item.last_report_json,
            item.attention_json,
            item.policy_hash_seen,
            now_rfc3339(&item.created_at),
            now_rfc3339(&item.updated_at),
        ],
    )
    .map_err(|e| format!("insert work item {}: {e}", item.item_id))?;
    Ok(())
}

fn insert_subscription(
    conn: &rusqlite::Connection,
    sub: &WorkItemSubscriptionRecord,
) -> Result<(), String> {
    conn.execute(
        &format!(
            "INSERT INTO work_item_subscriptions ({SUBSCRIPTION_COLUMNS}) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)"
        ),
        params![
            sub.subscription_id,
            sub.item_id,
            sub.tenant_id,
            sub.source_kind,
            sub.source_key,
            sub.connector_id,
            sub.conversation_id,
            sub.role,
            now_rfc3339(&sub.created_at),
            now_rfc3339(&sub.last_event_at),
            sub.dropped_at.as_ref().map(now_rfc3339),
            sub.drop_reason,
        ],
    )
    .map_err(|e| format!("insert subscription {}: {e}", sub.subscription_id))?;
    Ok(())
}

/// Collects mapped rows, failing on the first unreadable one.
fn collect<T>(
    rows: impl Iterator<Item = rusqlite::Result<Result<T, String>>>,
    what: &str,
) -> Result<Vec<T>, String> {
    let mut out = Vec::new();
    for row in rows {
        out.push(row.map_err(|e| format!("read {what}: {e}"))??);
    }
    Ok(out)
}

fn subscription_from_row(row: &Row<'_>) -> Parsed<WorkItemSubscriptionRecord> {
    let (created_at, last_event_at) = match (time_at(row, 8)?, time_at(row, 9)?) {
        (Ok(created), Ok(last)) => (created, last),
        (Err(err), _) | (_, Err(err)) => return Ok(Err(err)),
    };
    let dropped_at = match row
        .get::<_, Option<String>>(10)?
        .as_deref()
        .map(parse_rfc3339)
    {
        Some(Err(err)) => return Ok(Err(err)),
        Some(Ok(at)) => Some(at),
        None => None,
    };
    Ok(Ok(WorkItemSubscriptionRecord {
        subscription_id: row.get(0)?,
        item_id: row.get(1)?,
        tenant_id: row.get(2)?,
        source_kind: row.get(3)?,
        source_key: row.get(4)?,
        connector_id: row.get(5)?,
        conversation_id: row.get(6)?,
        role: row.get(7)?,
        created_at,
        last_event_at,
        dropped_at,
        drop_reason: row.get(11)?,
    }))
}

fn insert_round(conn: &rusqlite::Connection, round: &WorkItemRoundRecord) -> Result<(), String> {
    conn.execute(
        &format!(
            "INSERT INTO work_item_rounds ({ROUND_COLUMNS}) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)"
        ),
        params![
            round.item_id,
            round.seq,
            round.tenant_id,
            round.request_id,
            round.query_text,
            round.state,
            round.outcome,
            round.report_json,
            round.error_code,
            round.turn_id,
            now_rfc3339(&round.started_at),
            round.finished_at.as_ref().map(now_rfc3339),
        ],
    )
    .map_err(|e| {
        format!(
            "insert work item round {}#{}: {e}",
            round.item_id, round.seq
        )
    })?;
    Ok(())
}

/// A row read, then its timestamps parsed: the outer `Result` is SQLite's,
/// the inner one says a stored timestamp did not parse.
type Parsed<T> = rusqlite::Result<Result<T, String>>;

/// Reads a stored timestamp column, leaving a parse failure for the caller
/// to report with the row's context.
fn time_at(row: &Row<'_>, index: usize) -> rusqlite::Result<Result<DateTime<Utc>, String>> {
    Ok(parse_rfc3339(&row.get::<_, String>(index)?))
}

fn item_from_row(row: &Row<'_>) -> Parsed<(WorkItemRecord, WorkItemActivity)> {
    let (created_at, updated_at) = match (time_at(row, 13)?, time_at(row, 14)?) {
        (Ok(created), Ok(updated)) => (created, updated),
        (Err(err), _) | (_, Err(err)) => return Ok(Err(err)),
    };
    let record = WorkItemRecord {
        item_id: row.get(0)?,
        tenant_id: row.get(1)?,
        title: row.get(2)?,
        state: row.get(3)?,
        end_reason: row.get(4)?,
        thread_id: row.get(5)?,
        provider: row.get(6)?,
        model: row.get(7)?,
        seq: row.get(8)?,
        mandate_json: row.get(9)?,
        last_report_json: row.get(10)?,
        attention_json: row.get(11)?,
        policy_hash_seen: row.get(12)?,
        created_at,
        updated_at,
    };
    let activity = WorkItemActivity {
        last_round_state: row.get(15)?,
        pending_inputs: row.get(16)?,
        pending_person_inputs: row.get(17)?,
        outbox_problem: row.get(18)?,
    };
    Ok(Ok((record, activity)))
}

fn round_from_row(row: &Row<'_>) -> Parsed<WorkItemRoundRecord> {
    let started_at = match time_at(row, 10)? {
        Ok(at) => at,
        Err(err) => return Ok(Err(err)),
    };
    let finished_at = match row
        .get::<_, Option<String>>(11)?
        .as_deref()
        .map(parse_rfc3339)
    {
        Some(Err(err)) => return Ok(Err(err)),
        Some(Ok(at)) => Some(at),
        None => None,
    };
    Ok(Ok(WorkItemRoundRecord {
        item_id: row.get(0)?,
        seq: row.get(1)?,
        tenant_id: row.get(2)?,
        request_id: row.get(3)?,
        query_text: row.get(4)?,
        state: row.get(5)?,
        outcome: row.get(6)?,
        report_json: row.get(7)?,
        error_code: row.get(8)?,
        turn_id: row.get(9)?,
        started_at,
        finished_at,
    }))
}

fn input_from_row(row: &Row<'_>) -> Parsed<WorkItemInputRecord> {
    let created_at = match time_at(row, 9)? {
        Ok(at) => at,
        Err(err) => return Ok(Err(err)),
    };
    Ok(Ok(WorkItemInputRecord {
        item_id: row.get(0)?,
        seq: row.get(1)?,
        tenant_id: row.get(2)?,
        kind: row.get(3)?,
        author_is_person: row.get(4)?,
        body: row.get(5)?,
        source_ref: row.get(6)?,
        state: row.get(7)?,
        round_seq: row.get(8)?,
        created_at,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> String {
        let dir = std::env::temp_dir().join(format!(
            "kura_store_work_items_{name}_{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        dir.to_string_lossy().to_string()
    }

    fn item(tenant_id: &str, item_id: &str, at: DateTime<Utc>) -> WorkItemRecord {
        WorkItemRecord {
            item_id: item_id.to_string(),
            tenant_id: tenant_id.to_string(),
            title: "Fix the build".to_string(),
            state: "open".to_string(),
            end_reason: None,
            thread_id: format!("thr_{item_id}"),
            provider: "echo".to_string(),
            model: String::new(),
            seq: 1,
            mandate_json: r#"{"basis":"api"}"#.to_string(),
            last_report_json: None,
            attention_json: None,
            policy_hash_seen: String::new(),
            created_at: at,
            updated_at: at,
        }
    }

    fn round(tenant_id: &str, item_id: &str, at: DateTime<Utc>) -> WorkItemRoundRecord {
        WorkItemRoundRecord {
            item_id: item_id.to_string(),
            seq: 1,
            tenant_id: tenant_id.to_string(),
            request_id: format!("{item_id}_r1"),
            query_text: "do the work".to_string(),
            state: "running".to_string(),
            outcome: None,
            report_json: None,
            error_code: None,
            turn_id: None,
            started_at: at,
            finished_at: None,
        }
    }

    fn input(tenant_id: &str, item_id: &str, seq: i64, person: bool) -> WorkItemInputRecord {
        WorkItemInputRecord {
            item_id: item_id.to_string(),
            seq,
            tenant_id: tenant_id.to_string(),
            kind: "text".to_string(),
            author_is_person: person,
            body: format!("input {seq}"),
            source_ref: String::new(),
            state: "pending".to_string(),
            round_seq: None,
            created_at: Utc::now(),
        }
    }

    #[test]
    fn an_item_its_round_and_its_inputs_read_back_as_written() {
        let store = SQLiteStore::new(&temp_dir("roundtrip")).unwrap();
        let at = Utc::now();
        let written = item("ten_a", "wi_1", at);
        store
            .create_work_item(&written, &round("ten_a", "wi_1", at))
            .unwrap();
        store
            .insert_work_item_input(&input("ten_a", "wi_1", 1, true))
            .unwrap();
        store
            .insert_work_item_input(&input("ten_a", "wi_1", 2, false))
            .unwrap();

        let (read, activity) = store.get_work_item("ten_a", "wi_1").unwrap().unwrap();
        assert_eq!(read.item_id, written.item_id);
        assert_eq!(read.mandate_json, written.mandate_json);
        assert_eq!(read.provider, "echo");
        assert_eq!(read.created_at.timestamp_micros(), at.timestamp_micros());
        assert_eq!(
            activity,
            WorkItemActivity {
                last_round_state: Some("running".to_string()),
                pending_inputs: 2,
                pending_person_inputs: 1,
                outbox_problem: false,
            }
        );
        let rounds = store.list_work_item_rounds("ten_a", "wi_1").unwrap();
        assert_eq!(rounds.len(), 1);
        assert_eq!(rounds[0].request_id, "wi_1_r1");
        assert_eq!(rounds[0].query_text, "do the work");
        let inputs = store.list_work_item_inputs("ten_a", "wi_1").unwrap();
        assert_eq!(inputs.iter().map(|i| i.seq).collect::<Vec<_>>(), vec![1, 2]);
        assert!(inputs[0].author_is_person);
        assert!(!inputs[1].author_is_person);
        // A seq is said once.
        assert!(
            store
                .insert_work_item_input(&input("ten_a", "wi_1", 2, true))
                .is_err()
        );
        assert_eq!(store.count_open_work_items("ten_a").unwrap(), 1);
    }

    #[test]
    fn a_settlement_applies_once_and_keeps_the_mandate() {
        let store = SQLiteStore::new(&temp_dir("settle")).unwrap();
        let at = Utc::now();
        store
            .create_work_item(&item("ten_a", "wi_1", at), &round("ten_a", "wi_1", at))
            .unwrap();
        let settlement = WorkItemRoundSettlement {
            round_state: "settled".to_string(),
            outcome: "ready".to_string(),
            report_json: Some(r#"{"status":"ready","summary":"done"}"#.to_string()),
            error_code: None,
            turn_id: Some("trn_1".to_string()),
            finished_at: at + chrono::Duration::seconds(5),
            item_state: "ended".to_string(),
            end_reason: Some("completed".to_string()),
            last_report_json: Some(r#"{"status":"ready","summary":"done"}"#.to_string()),
            attention_json: None,
        };
        assert!(
            store
                .settle_work_item_round("ten_a", "wi_1", 1, &settlement)
                .unwrap()
        );
        assert!(
            !store
                .settle_work_item_round("ten_a", "wi_1", 1, &settlement)
                .unwrap(),
            "a settled round settled again"
        );
        let (read, activity) = store.get_work_item("ten_a", "wi_1").unwrap().unwrap();
        assert_eq!(read.state, "ended");
        assert_eq!(read.end_reason.as_deref(), Some("completed"));
        assert_eq!(read.mandate_json, r#"{"basis":"api"}"#);
        assert!(read.attention_json.is_none());
        assert_eq!(activity.last_round_state.as_deref(), Some("settled"));
        let rounds = store.list_work_item_rounds("ten_a", "wi_1").unwrap();
        assert_eq!(rounds[0].outcome.as_deref(), Some("ready"));
        assert_eq!(rounds[0].turn_id.as_deref(), Some("trn_1"));
        assert!(rounds[0].finished_at.is_some());
        assert_eq!(store.count_open_work_items("ten_a").unwrap(), 0);
        assert_eq!(
            store
                .list_work_items("ten_a", Some("ended"), 10)
                .unwrap()
                .len(),
            1
        );
        assert!(
            store
                .list_work_items("ten_a", Some("open"), 10)
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn another_tenants_item_reads_as_absent() {
        let store = SQLiteStore::new(&temp_dir("tenant")).unwrap();
        let at = Utc::now();
        store
            .create_work_item(&item("ten_a", "wi_1", at), &round("ten_a", "wi_1", at))
            .unwrap();
        assert!(store.get_work_item("ten_b", "wi_1").unwrap().is_none());
        assert!(store.list_work_items("ten_b", None, 10).unwrap().is_empty());
        assert!(
            store
                .list_work_item_rounds("ten_b", "wi_1")
                .unwrap()
                .is_empty()
        );
        assert!(
            store
                .insert_work_item_input(&input("ten_b", "wi_1", 1, true))
                .is_err(),
            "an input was written to another tenant's item"
        );
        let settlement = WorkItemRoundSettlement {
            round_state: "failed".to_string(),
            outcome: "provider".to_string(),
            report_json: None,
            error_code: Some("provider".to_string()),
            turn_id: None,
            finished_at: at,
            item_state: "open".to_string(),
            end_reason: None,
            last_report_json: None,
            attention_json: None,
        };
        assert!(
            !store
                .settle_work_item_round("ten_b", "wi_1", 1, &settlement)
                .unwrap(),
            "another tenant settled the round"
        );
        assert_eq!(store.list_work_items("ten_a", None, 10).unwrap().len(), 1);
    }

    #[test]
    fn a_v9_database_gains_the_tables_on_upgrade() {
        let dir = temp_dir("upgrade");
        {
            let store = SQLiteStore::new_at_version(&dir, 9).unwrap();
            assert!(store.get_work_item("ten_a", "wi_1").is_err());
        }
        let store = SQLiteStore::new(&dir).unwrap();
        assert_eq!(
            store.schema_version().unwrap(),
            crate::CURRENT_SCHEMA_VERSION
        );
        let at = Utc::now();
        store
            .create_work_item(&item("ten_a", "wi_1", at), &round("ten_a", "wi_1", at))
            .unwrap();
        assert!(store.get_work_item("ten_a", "wi_1").unwrap().is_some());
    }

    fn owner(
        tenant_id: &str,
        item_id: &str,
        key: &str,
        at: DateTime<Utc>,
    ) -> WorkItemSubscriptionRecord {
        WorkItemSubscriptionRecord {
            subscription_id: format!("wis_{item_id}"),
            item_id: item_id.to_string(),
            tenant_id: tenant_id.to_string(),
            source_kind: "im".to_string(),
            source_key: key.to_string(),
            connector_id: "slack-main".to_string(),
            conversation_id: "c1".to_string(),
            role: "owner".to_string(),
            created_at: at,
            last_event_at: at,
            dropped_at: None,
            drop_reason: None,
        }
    }

    fn settled(item_state: &str, end_reason: Option<&str>) -> WorkItemRoundSettlement {
        WorkItemRoundSettlement {
            round_state: "settled".to_string(),
            outcome: "ready".to_string(),
            report_json: None,
            error_code: None,
            turn_id: None,
            finished_at: Utc::now(),
            item_state: item_state.to_string(),
            end_reason: end_reason.map(str::to_string),
            last_report_json: None,
            attention_json: None,
        }
    }

    /// Slice 2 upgrade: a v10 database (slice 1's items, no subscriptions)
    /// keeps its items and gains the subscriptions table.
    #[test]
    fn a_v10_database_keeps_its_items_and_gains_subscriptions_on_upgrade() {
        let dir = temp_dir("upgrade_v10");
        let at = Utc::now();
        {
            let store = SQLiteStore::new_at_version(&dir, 10).unwrap();
            assert_eq!(store.schema_version().unwrap(), 10);
            store
                .create_work_item(&item("ten_a", "wi_1", at), &round("ten_a", "wi_1", at))
                .unwrap();
            assert!(
                store.list_work_item_subscriptions("ten_a", "wi_1").is_err(),
                "v10 has no subscriptions table"
            );
        }
        let store = SQLiteStore::new(&dir).unwrap();
        assert_eq!(
            store.schema_version().unwrap(),
            crate::CURRENT_SCHEMA_VERSION
        );
        let (read, _) = store.get_work_item("ten_a", "wi_1").unwrap().unwrap();
        assert_eq!(read.mandate_json, r#"{"basis":"api"}"#);
        assert!(
            store
                .list_work_item_subscriptions("ten_a", "wi_1")
                .unwrap()
                .is_empty(),
            "an existing item gains no subscription"
        );
        store
            .create_work_item_with_owner(
                &item("ten_a", "wi_2", at),
                &round("ten_a", "wi_2", at),
                &owner("ten_a", "wi_2", "thr_src_1", at),
            )
            .unwrap()
            .unwrap();
    }

    #[test]
    fn one_open_owner_per_source_and_an_ended_owner_is_superseded() {
        let store = SQLiteStore::new(&temp_dir("owner")).unwrap();
        let at = Utc::now();
        store
            .create_work_item_with_owner(
                &item("ten_a", "wi_1", at),
                &round("ten_a", "wi_1", at),
                &owner("ten_a", "wi_1", "thr_src_1", at),
            )
            .unwrap()
            .unwrap();
        // D2: the open owner keeps the conversation; nothing is written.
        let refused = store
            .create_work_item_with_owner(
                &item("ten_a", "wi_2", at),
                &round("ten_a", "wi_2", at),
                &owner("ten_a", "wi_2", "thr_src_1", at),
            )
            .unwrap();
        assert_eq!(refused, Err("wi_1".to_string()));
        assert!(store.get_work_item("ten_a", "wi_2").unwrap().is_none());
        // Another tenant's conversation with the same key is its own.
        store
            .create_work_item_with_owner(
                &item("ten_b", "wi_3", at),
                &round("ten_b", "wi_3", at),
                &owner("ten_b", "wi_3", "thr_src_1", at),
            )
            .unwrap()
            .unwrap();

        // Once wi_1 ends on its own report, a new item takes over.
        assert!(
            store
                .settle_work_item_round("ten_a", "wi_1", 1, &settled("ended", Some("completed")))
                .unwrap()
        );
        store
            .create_work_item_with_owner(
                &item("ten_a", "wi_4", at),
                &round("ten_a", "wi_4", at),
                &owner("ten_a", "wi_4", "thr_src_1", at),
            )
            .unwrap()
            .unwrap();
        let live = store
            .live_work_item_subscriptions_for_source("ten_a", "thr_src_1", None)
            .unwrap();
        assert_eq!(live.len(), 1);
        assert_eq!(live[0].item_id, "wi_4");
        let old = store.list_work_item_subscriptions("ten_a", "wi_1").unwrap();
        assert_eq!(old[0].drop_reason.as_deref(), Some("superseded"));
        assert!(
            store
                .live_work_item_subscriptions_for_source("ten_c", "thr_src_1", None)
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn an_idle_subscription_is_dropped_when_looked_up() {
        let store = SQLiteStore::new(&temp_dir("idle")).unwrap();
        let long_ago = Utc::now() - chrono::Duration::days(30);
        store
            .create_work_item_with_owner(
                &item("ten_a", "wi_1", long_ago),
                &round("ten_a", "wi_1", long_ago),
                &owner("ten_a", "wi_1", "thr_src_1", long_ago),
            )
            .unwrap()
            .unwrap();
        let cutoff = Utc::now() - chrono::Duration::days(14);
        // Touched recently: still live.
        store
            .touch_work_item_subscription("ten_a", "wis_wi_1", Utc::now())
            .unwrap();
        assert_eq!(
            store
                .live_work_item_subscriptions_for_source("ten_a", "thr_src_1", Some(cutoff))
                .unwrap()
                .len(),
            1
        );
        store
            .touch_work_item_subscription("ten_a", "wis_wi_1", long_ago)
            .unwrap();
        assert!(
            store
                .live_work_item_subscriptions_for_source("ten_a", "thr_src_1", Some(cutoff))
                .unwrap()
                .is_empty()
        );
        let subs = store.list_work_item_subscriptions("ten_a", "wi_1").unwrap();
        assert_eq!(subs[0].drop_reason.as_deref(), Some("idle"));
    }

    #[test]
    fn a_round_starts_once_and_consumes_the_inputs_it_quotes() {
        let store = SQLiteStore::new(&temp_dir("start")).unwrap();
        let at = Utc::now();
        store
            .create_work_item(&item("ten_a", "wi_1", at), &round("ten_a", "wi_1", at))
            .unwrap();
        let mut next = round("ten_a", "wi_1", at);
        next.seq = 2;
        next.request_id = "wi_1_r2".to_string();
        // Round 1 still runs: nothing starts.
        assert!(!store.start_work_item_round(&next, 10, false).unwrap());
        assert!(
            store
                .settle_work_item_round("ten_a", "wi_1", 1, &settled("open", None))
                .unwrap()
        );
        assert_eq!(
            store
                .append_work_item_input(&input("ten_a", "wi_1", 0, true))
                .unwrap(),
            1
        );
        assert_eq!(
            store
                .append_work_item_input(&input("ten_a", "wi_1", 0, true))
                .unwrap(),
            2
        );
        assert!(
            store
                .append_work_item_input(&input("ten_b", "wi_1", 0, true))
                .is_err(),
            "an input was appended to another tenant's item"
        );
        // Only input 1 was read for the query; input 2 stays pending.
        assert!(store.start_work_item_round(&next, 1, false).unwrap());
        assert!(
            !store.start_work_item_round(&next, 2, false).unwrap(),
            "the same round started twice"
        );
        let inputs = store.list_work_item_inputs("ten_a", "wi_1").unwrap();
        assert_eq!(inputs[0].state, "consumed");
        assert_eq!(inputs[0].round_seq, Some(2));
        assert_eq!(inputs[1].state, "pending");
        let (read, activity) = store.get_work_item("ten_a", "wi_1").unwrap().unwrap();
        assert_eq!(read.seq, 2);
        assert_eq!(activity.last_round_state.as_deref(), Some("running"));
        assert_eq!(
            store
                .count_work_item_rounds_since("ten_a", "wi_1", at - chrono::Duration::hours(1))
                .unwrap(),
            2
        );
    }

    #[test]
    fn a_stop_is_final_and_a_round_still_running_cannot_undo_it() {
        let store = SQLiteStore::new(&temp_dir("stop")).unwrap();
        let at = Utc::now();
        store
            .create_work_item_with_owner(
                &item("ten_a", "wi_1", at),
                &round("ten_a", "wi_1", at),
                &owner("ten_a", "wi_1", "thr_src_1", at),
            )
            .unwrap()
            .unwrap();
        assert!(
            !store
                .end_work_item_finally("ten_a", "wi_1", "revoked", at)
                .unwrap(),
            "an open item was revoked"
        );
        assert!(
            !store
                .end_work_item_finally("ten_b", "wi_1", "stopped", at)
                .unwrap(),
            "another tenant stopped the item"
        );
        assert!(
            store
                .end_work_item_finally("ten_a", "wi_1", "stopped", at)
                .unwrap()
        );
        assert!(
            !store
                .end_work_item_finally("ten_a", "wi_1", "stopped", at)
                .unwrap()
        );
        // The round that was running reports progress: the item stays stopped.
        assert!(
            store
                .settle_work_item_round("ten_a", "wi_1", 1, &settled("open", None))
                .unwrap()
        );
        let (read, _) = store.get_work_item("ten_a", "wi_1").unwrap().unwrap();
        assert_eq!(read.state, "ended");
        assert_eq!(read.end_reason.as_deref(), Some("stopped"));
        assert!(!store.reopen_work_item("ten_a", "wi_1", at).unwrap());
        assert!(
            store
                .live_work_item_subscriptions_for_source("ten_a", "thr_src_1", None)
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn only_an_item_that_ended_on_its_report_reopens_or_is_revoked() {
        let store = SQLiteStore::new(&temp_dir("reopen")).unwrap();
        let at = Utc::now();
        store
            .create_work_item(&item("ten_a", "wi_1", at), &round("ten_a", "wi_1", at))
            .unwrap();
        assert!(!store.reopen_work_item("ten_a", "wi_1", at).unwrap());
        store
            .settle_work_item_round("ten_a", "wi_1", 1, &settled("ended", Some("dropped")))
            .unwrap();
        assert!(!store.reopen_work_item("ten_b", "wi_1", at).unwrap());
        assert!(store.reopen_work_item("ten_a", "wi_1", at).unwrap());
        let (read, _) = store.get_work_item("ten_a", "wi_1").unwrap().unwrap();
        assert_eq!((read.state.as_str(), read.end_reason), ("open", None));

        store
            .create_work_item(&item("ten_a", "wi_2", at), &round("ten_a", "wi_2", at))
            .unwrap();
        store
            .settle_work_item_round("ten_a", "wi_2", 1, &settled("ended", Some("completed")))
            .unwrap();
        assert!(
            store
                .end_work_item_finally("ten_a", "wi_2", "revoked", at)
                .unwrap()
        );
        assert!(!store.reopen_work_item("ten_a", "wi_2", at).unwrap());
        assert!(
            store
                .end_work_item_finally("ten_a", "wi_2", "bogus", at)
                .is_err()
        );
    }

    fn reply(tenant_id: &str, item_id: &str, key: &str, body: &str) -> WorkItemOutboxRecord {
        let at = Utc::now();
        WorkItemOutboxRecord {
            item_id: item_id.to_string(),
            seq: 0,
            tenant_id: tenant_id.to_string(),
            conversation_key: key.to_string(),
            connector_id: "slack-main".to_string(),
            conversation_id: "C1".to_string(),
            channel_id: "C1".to_string(),
            peer_id: "C1".to_string(),
            reply_to_external_message_id: "m1".to_string(),
            participation_gated: true,
            round_seq: Some(1),
            source_ref: "delivery_1".to_string(),
            body: body.to_string(),
            state: "queued".to_string(),
            attempts: 0,
            next_attempt_at: at,
            last_error: None,
            drop_reason: None,
            external_message_id: String::new(),
            created_at: at,
            updated_at: at,
            sent_at: None,
            dropped_at: None,
        }
    }

    fn heads(store: &SQLiteStore, now: DateTime<Utc>) -> Vec<(String, i64)> {
        store
            .due_work_item_outbox(now, 100)
            .unwrap()
            .into_iter()
            .map(|row| (row.item_id, row.seq))
            .collect()
    }

    /// Slice 3: one conversation's rows go out in order; a failed head
    /// holds its successors until its retry is due and it is sent or
    /// dropped; another conversation and another item never wait.
    #[test]
    fn an_outbox_row_waits_for_every_earlier_row_of_its_conversation() {
        let store = SQLiteStore::new(&temp_dir("outbox_order")).unwrap();
        let at = Utc::now();
        for id in ["wi_1", "wi_2"] {
            store
                .create_work_item(&item("ten_a", id, at), &round("ten_a", id, at))
                .unwrap();
        }
        let seqs = store
            .enqueue_work_item_outbox(&[
                reply("ten_a", "wi_1", "thr_src_1", "first"),
                reply("ten_a", "wi_1", "thr_src_1", "second"),
                reply("ten_a", "wi_1", "thr_src_2", "elsewhere"),
            ])
            .unwrap();
        assert_eq!(seqs, vec![1, 2, 3]);
        store
            .enqueue_work_item_outbox(&[reply("ten_a", "wi_2", "thr_src_1", "other item")])
            .unwrap();
        let now = Utc::now() + chrono::Duration::milliseconds(1);
        assert_eq!(
            heads(&store, now),
            vec![
                ("wi_1".to_string(), 1),
                ("wi_1".to_string(), 3),
                ("wi_2".to_string(), 1)
            ]
        );

        // The head fails: it is retried in 5 s and its successor waits.
        let retry_at = now + chrono::Duration::seconds(5);
        assert_eq!(
            store
                .mark_work_item_outbox_failed("ten_a", "wi_1", 1, "timeout", retry_at, now)
                .unwrap(),
            Some(1)
        );
        assert_eq!(
            heads(&store, now),
            vec![("wi_1".to_string(), 3), ("wi_2".to_string(), 1)]
        );
        assert_eq!(
            store
                .work_item_outbox_head("ten_a", "wi_1", "thr_src_1")
                .unwrap()
                .map(|row| (row.seq, row.state, row.attempts)),
            Some((1, "failed".to_string(), 1))
        );
        let later = heads(&store, retry_at);
        assert!(later.contains(&("wi_1".to_string(), 1)));
        assert!(
            !later.contains(&("wi_1".to_string(), 2)),
            "a row behind an unsent one is never offered"
        );

        // Three failures in a row make the item a delivery problem.
        for _ in 0..2 {
            store
                .mark_work_item_outbox_failed("ten_a", "wi_1", 1, "timeout", retry_at, now)
                .unwrap();
        }
        let (_, activity) = store.get_work_item("ten_a", "wi_1").unwrap().unwrap();
        assert!(activity.outbox_problem);
        let (_, other) = store.get_work_item("ten_a", "wi_2").unwrap().unwrap();
        assert!(!other.outbox_problem);

        // Sent at last: the successor is next, and the problem is gone.
        assert!(
            store
                .mark_work_item_outbox_sent("ten_a", "wi_1", 1, "ts_1", retry_at)
                .unwrap()
        );
        assert!(
            !store
                .mark_work_item_outbox_sent("ten_a", "wi_1", 1, "ts_1", retry_at)
                .unwrap(),
            "a sent row is final"
        );
        assert!(heads(&store, retry_at).contains(&("wi_1".to_string(), 2)));
        let (_, activity) = store.get_work_item("ten_a", "wi_1").unwrap().unwrap();
        assert!(!activity.outbox_problem);

        // A drop is final too, and unblocks what follows.
        assert!(
            store
                .drop_work_item_outbox("ten_a", "wi_1", 2, "withdrawn", None, retry_at)
                .unwrap()
        );
        assert!(
            !store
                .drop_work_item_outbox("ten_a", "wi_1", 2, "withdrawn", None, retry_at)
                .unwrap()
        );
        let rows = store.list_work_item_outbox("ten_a", "wi_1").unwrap();
        assert_eq!(rows[0].state, "sent");
        assert_eq!(rows[0].external_message_id, "ts_1");
        assert_eq!(rows[0].last_error.as_deref(), Some("timeout"));
        assert!(rows[0].sent_at.is_some());
        assert_eq!(rows[1].state, "dropped");
        assert_eq!(rows[1].drop_reason.as_deref(), Some("withdrawn"));
        assert!(rows[1].dropped_at.is_some());
        assert!(rows[2].participation_gated);
        assert_eq!(rows[2].round_seq, Some(1));
        assert!(
            store
                .work_item_outbox_head("ten_a", "wi_1", "thr_src_1")
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn another_tenants_outbox_is_out_of_reach() {
        let store = SQLiteStore::new(&temp_dir("outbox_tenant")).unwrap();
        let at = Utc::now();
        store
            .create_work_item(&item("ten_a", "wi_1", at), &round("ten_a", "wi_1", at))
            .unwrap();
        assert!(
            store
                .enqueue_work_item_outbox(&[
                    reply("ten_a", "wi_1", "thr_src_1", "mine"),
                    reply("ten_b", "wi_1", "thr_src_1", "theirs"),
                ])
                .is_err(),
            "a row for another tenant's item was queued"
        );
        assert!(
            store
                .list_work_item_outbox("ten_a", "wi_1")
                .unwrap()
                .is_empty(),
            "a refused batch writes nothing"
        );
        store
            .enqueue_work_item_outbox(&[reply("ten_a", "wi_1", "thr_src_1", "mine")])
            .unwrap();
        assert!(
            store
                .list_work_item_outbox("ten_b", "wi_1")
                .unwrap()
                .is_empty()
        );
        assert!(
            store
                .work_item_outbox_head("ten_b", "wi_1", "thr_src_1")
                .unwrap()
                .is_none()
        );
        assert!(
            !store
                .mark_work_item_outbox_sent("ten_b", "wi_1", 1, "x", at)
                .unwrap()
        );
        assert_eq!(
            store
                .mark_work_item_outbox_failed("ten_b", "wi_1", 1, "x", at, at)
                .unwrap(),
            None
        );
        assert!(
            !store
                .drop_work_item_outbox("ten_b", "wi_1", 1, "withdrawn", None, at)
                .unwrap()
        );
    }

    /// Slice 3 upgrade: a v11 database keeps its items and subscriptions
    /// and gains the outbox.
    #[test]
    fn a_v11_database_gains_the_outbox_on_upgrade() {
        let dir = temp_dir("upgrade_v11");
        let at = Utc::now();
        {
            let store = SQLiteStore::new_at_version(&dir, 11).unwrap();
            store
                .create_work_item_with_owner(
                    &item("ten_a", "wi_1", at),
                    &round("ten_a", "wi_1", at),
                    &owner("ten_a", "wi_1", "thr_src_1", at),
                )
                .unwrap()
                .unwrap();
            assert!(
                store.list_work_item_outbox("ten_a", "wi_1").is_err(),
                "v11 has no outbox"
            );
        }
        let store = SQLiteStore::new(&dir).unwrap();
        assert_eq!(
            store.schema_version().unwrap(),
            crate::CURRENT_SCHEMA_VERSION
        );
        let (_, activity) = store.get_work_item("ten_a", "wi_1").unwrap().unwrap();
        assert!(!activity.outbox_problem);
        assert_eq!(
            store
                .list_work_item_subscriptions("ten_a", "wi_1")
                .unwrap()
                .len(),
            1
        );
        assert!(
            store
                .list_work_item_outbox("ten_a", "wi_1")
                .unwrap()
                .is_empty()
        );
    }

    /// An item whose round 1 a previous process started at `at` and never
    /// settled, having consumed one input and been steered another.
    fn interrupted(store: &SQLiteStore, item_id: &str, at: DateTime<Utc>) {
        store
            .create_work_item(&item("ten_a", item_id, at), &round("ten_a", item_id, at))
            .unwrap();
        for (seq, state) in [(1, "consumed"), (2, "steered")] {
            store
                .insert_work_item_input(&WorkItemInputRecord {
                    state: state.to_string(),
                    round_seq: Some(1),
                    ..input("ten_a", item_id, seq, true)
                })
                .unwrap();
        }
    }

    fn note(item_id: &str) -> WorkItemInputRecord {
        WorkItemInputRecord {
            kind: "recovery".to_string(),
            body: "The daemon restarted.".to_string(),
            ..input("ten_a", item_id, 0, false)
        }
    }

    #[test]
    fn an_interrupted_round_is_claimed_once_and_never_interrupted_under_its_rerun() {
        let store = SQLiteStore::new(&temp_dir("recovery_claim")).unwrap();
        let before = Utc::now() - chrono::Duration::minutes(5);
        interrupted(&store, "wi_1", before);
        let boot = Utc::now();
        let listed = store.list_interrupted_work_item_rounds(boot, 10).unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].request_id, "wi_1_r1");
        // Only rounds started before the cutoff are a previous process's.
        assert!(
            store
                .list_interrupted_work_item_rounds(before, 10)
                .unwrap()
                .is_empty()
        );
        assert!(
            store
                .claim_work_item_round_recovery("ten_b", "wi_1", 1, boot, &note("wi_1"), boot)
                .unwrap()
                .is_none(),
            "another tenant claimed the round"
        );

        let claimed_at = Utc::now();
        assert_eq!(
            store
                .claim_work_item_round_recovery("ten_a", "wi_1", 1, boot, &note("wi_1"), claimed_at)
                .unwrap(),
            Some(3)
        );
        assert!(
            store
                .claim_work_item_round_recovery("ten_a", "wi_1", 1, boot, &note("wi_1"), Utc::now())
                .unwrap()
                .is_none(),
            "a second scan claimed the same round"
        );
        assert!(
            !store
                .interrupt_work_item_round("ten_a", "wi_1", 1, boot, true, Utc::now())
                .unwrap(),
            "a claimed round was interrupted under its rerun"
        );
        assert!(
            store
                .list_interrupted_work_item_rounds(boot, 10)
                .unwrap()
                .is_empty()
        );
        let rounds = store.list_work_item_rounds("ten_a", "wi_1").unwrap();
        assert_eq!(rounds[0].state, "running");
        assert_eq!(
            rounds[0].started_at.timestamp_micros(),
            claimed_at.timestamp_micros()
        );
        let inputs = store.list_work_item_inputs("ten_a", "wi_1").unwrap();
        assert_eq!(inputs[2].kind, "recovery");
        assert_eq!(inputs[2].state, "consumed");
        assert_eq!(inputs[2].round_seq, Some(1));
        assert_eq!(
            store
                .count_work_item_round_recoveries("ten_a", "wi_1", 1)
                .unwrap(),
            1
        );
    }

    #[test]
    fn an_interrupted_round_not_rerun_fails_and_gives_back_its_inputs() {
        let store = SQLiteStore::new(&temp_dir("recovery_interrupt")).unwrap();
        interrupted(&store, "wi_1", Utc::now() - chrono::Duration::minutes(5));
        let boot = Utc::now();
        assert!(
            store
                .interrupt_work_item_round("ten_a", "wi_1", 1, boot, true, Utc::now())
                .unwrap()
        );
        assert!(
            !store
                .interrupt_work_item_round("ten_a", "wi_1", 1, boot, true, Utc::now())
                .unwrap(),
            "interrupted twice"
        );
        let round = &store.list_work_item_rounds("ten_a", "wi_1").unwrap()[0];
        assert_eq!(round.state, "failed");
        assert_eq!(round.outcome.as_deref(), Some("interrupted"));
        assert_eq!(round.error_code.as_deref(), Some("interrupted"));
        assert!(round.finished_at.is_some());
        let (_, activity) = store.get_work_item("ten_a", "wi_1").unwrap().unwrap();
        assert_eq!(activity.last_round_state.as_deref(), Some("failed"));
        assert_eq!(activity.pending_inputs, 2, "both inputs are pending again");
        assert!(
            store
                .claim_work_item_round_recovery("ten_a", "wi_1", 1, boot, &note("wi_1"), boot)
                .unwrap()
                .is_none()
        );

        // Without releasing, the inputs stay the round's.
        interrupted(&store, "wi_2", Utc::now() - chrono::Duration::minutes(5));
        assert!(
            store
                .interrupt_work_item_round("ten_a", "wi_2", 1, boot, false, Utc::now())
                .unwrap()
        );
        let (_, activity) = store.get_work_item("ten_a", "wi_2").unwrap().unwrap();
        assert_eq!(activity.pending_inputs, 0);
    }

    #[test]
    fn a_requests_checkpoints_are_found_without_the_turn_key() {
        let store = SQLiteStore::new(&temp_dir("recovery_checkpoints")).unwrap();
        let at = Utc::now();
        for (key, request) in [("k1", "wi_1_r1"), ("k2", "wi_1_r2")] {
            store
                .put_turn_checkpoint(&crate::TurnCheckpointRecord {
                    turn_key: key.to_string(),
                    tenant_id: "ten_a".to_string(),
                    thread_id: "thr_wi_1".to_string(),
                    request_id: request.to_string(),
                    checkpoint_json: "{}".to_string(),
                    created_at: at,
                    updated_at: at,
                })
                .unwrap();
        }
        let found = store
            .turn_checkpoints_for_request("ten_a", "thr_wi_1", "wi_1_r1")
            .unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].turn_key, "k1");
        assert!(
            store
                .turn_checkpoints_for_request("ten_b", "thr_wi_1", "wi_1_r1")
                .unwrap()
                .is_empty()
        );
    }
}
