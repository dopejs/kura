//! The outbox (work-items.md §8, slice 3): what an item says to a
//! conversation, sent in order and retried until it is sent or dropped.
//!
//! - **Order.** For one (item, conversation key) a row is sent only once
//!   every earlier row is `sent` or `dropped`; other items and other
//!   conversations never wait. At most one sender works on one (item,
//!   conversation) at a time ([`Outbox`]'s claims; the daemon is one
//!   process, §11).
//! - **Two ways out, one path.** The IM hook queues the replies of the
//!   message it took and sends them at once through the connector loop's
//!   own reply sender, lent for that message: the person reads them when
//!   slice 2 sent them inline. Whatever that could not send is the
//!   sweeper's: one thread per daemon, sending due rows through the reply
//!   senders the connector runtimes registered at boot
//!   ([`Manager::register_reply_sender`]). Its first pass is the boot
//!   sweep.
//! - **Retries** back off 5 s doubling to 300 s, persisted as
//!   `next_attempt_at`, so a restart resumes them where they were. After 3
//!   failures in a row the item shows `person/delivery`; the row keeps
//!   being retried and its successors keep waiting. A person's next
//!   message on the conversation retries the head at once.
//! - **Drops**, only for: `connector_gone` (a gated group or room the
//!   route policy no longer receives, checked before every attempt);
//!   `conversation_gone` / `forbidden` (the platform's own permanent error
//!   codes, recognised in the sender's error text: senders return plain
//!   strings, so anything not recognised is retried); or a person's
//!   withdrawal. A drop is final and emits `work_item.outbox_dropped`.
//! - **At least once.** A crash after the platform accepted a message but
//!   before `sent` is written sends it again. A body over the platform's
//!   length is sent in parts; a failure after some parts resends them all.
//!
//! Bodies are stored and shown on the tenant's item read, as inputs are.
//! Events and logs carry seqs, states and codes only, and a transport's
//! error text is never stored or logged: it can carry a credential-bearing
//! URL (the Telegram Bot API puts the token in the path).

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Weak};
use std::time::Duration as StdDuration;

use chrono::{DateTime, Duration, Utc};
use kura_im::ReplySender;
use kura_imtypes::{DeliveryDirection, DeliveryStatus, MessageRecord, OutboundReply};
use kura_store::channel_management::{route_policy_allows_conversation, route_policy_is_valid};
use kura_store::work_items::{OUTBOX_PROBLEM_ATTEMPTS, WorkItemOutboxRecord};
use parking_lot::{Condvar, Mutex, RwLock};
use serde_json::json;

use crate::manager::{Manager, WorkItemError};
use crate::types::{OutboxDropReason, OutboxRow, OutboxState, WorkItem};

/// The first retry waits this long; each failure in a row doubles it, up
/// to [`OUTBOX_MAX_BACKOFF_SECS`].
pub const OUTBOX_FIRST_BACKOFF_SECS: i64 = 5;
pub const OUTBOX_MAX_BACKOFF_SECS: i64 = 300;

/// Heads one sweep reads at most.
const SWEEP_BATCH: usize = 256;
/// Passes one sweep makes at most: a pass that sends a head makes its
/// successor the next pass's head.
const SWEEP_MAX_PASSES: usize = 32;
/// The sweeper's longest sleep, and how far ahead a sweep looks for the
/// next row due.
const SWEEP_IDLE: StdDuration = StdDuration::from_secs(30);
/// Its shortest, so a row it may not take now (the IM hook is sending
/// that conversation) cannot make it spin.
const SWEEP_MIN_WAIT: StdDuration = StdDuration::from_millis(250);

/// The `last_error` codes. Never the transport's text.
const ERROR_SEND_FAILED: &str = "send_failed";
const ERROR_RECEIVE_CHECK_FAILED: &str = "receive_check_failed";

/// How long a row waits after `failures` failed attempts in a row.
#[must_use]
pub fn outbox_backoff(failures: i64) -> Duration {
    let doublings = u32::try_from(failures.saturating_sub(1).clamp(0, 16)).unwrap_or(16);
    Duration::seconds(
        OUTBOX_FIRST_BACKOFF_SECS
            .saturating_mul(1_i64 << doublings)
            .min(OUTBOX_MAX_BACKOFF_SECS),
    )
}

/// Where a reply goes, copied from the message it answers, so a retry goes
/// exactly where the first attempt went.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ReplyBinding {
    /// With the item, the order key. For IM, the conversation's
    /// `thr_src_<hash>` id.
    pub conversation_key: String,
    pub connector_id: String,
    /// The conversation as the connector names it.
    pub conversation_id: String,
    pub channel_id: String,
    pub peer_id: String,
    pub reply_to_external_message_id: String,
    /// A group or room whose connector enforces participation: before each
    /// attempt, the route policy must still receive the conversation.
    pub participation_gated: bool,
    /// The inbound delivery the reply answers.
    pub source_ref: String,
}

/// What a flush through a lent sender did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Flushed {
    pub sent_message_ids: Vec<String>,
    /// Rows of the conversation still unsent; the sweeper retries them.
    pub waiting: usize,
}

/// The outbox's process state: the registered senders, the conversations
/// being sent to now, and the sweeper.
#[derive(Default)]
pub(crate) struct Outbox {
    senders: RwLock<HashMap<String, Arc<dyn ReplySender>>>,
    /// (item, conversation key) a sender is working on now.
    sending: Mutex<HashSet<(String, String)>>,
    signal: Arc<Signal>,
    sweeper: Mutex<Option<std::thread::JoinHandle<()>>>,
    /// Connectors a due row waits on without a sender, logged once each.
    unsendable_logged: Mutex<HashSet<String>>,
}

#[derive(Default)]
struct Signal {
    state: Mutex<SignalState>,
    wake: Condvar,
}

#[derive(Default)]
struct SignalState {
    stop: bool,
    poked: bool,
}

/// One sender's hold on an (item, conversation); released on drop.
struct Claim<'a> {
    sending: &'a Mutex<HashSet<(String, String)>>,
    key: (String, String),
}

impl Drop for Claim<'_> {
    fn drop(&mut self) {
        self.sending.lock().remove(&self.key);
    }
}

impl Outbox {
    fn claim(&self, item_id: &str, conversation_key: &str) -> Option<Claim<'_>> {
        let key = (item_id.to_string(), conversation_key.to_string());
        if !self.sending.lock().insert(key.clone()) {
            return None;
        }
        Some(Claim {
            sending: &self.sending,
            key,
        })
    }

    /// Wakes the sweeper to look again now.
    pub(crate) fn poke(&self) {
        self.signal.state.lock().poked = true;
        self.signal.wake.notify_all();
    }
}

/// How one attempt on a row ended.
enum Attempt {
    Sent(String),
    Retry(DateTime<Utc>),
    Dropped,
    /// The row was no longer unsent when the attempt was recorded.
    Gone,
}

impl Manager {
    /// The sender the outbox sends `connector_id`'s rows through when no
    /// connector loop lends one (the sweeper). Registered by the daemon for
    /// every connector runtime it starts; a later registration replaces an
    /// earlier one.
    pub fn register_reply_sender(&self, connector_id: &str, sender: Arc<dyn ReplySender>) {
        let connector_id = connector_id.trim();
        if connector_id.is_empty() {
            return;
        }
        self.outbox
            .senders
            .write()
            .insert(connector_id.to_string(), sender);
        self.outbox.unsendable_logged.lock().remove(connector_id);
        eprintln!("[kura] work item: outbox sends to connector {connector_id}");
        self.outbox.poke();
    }

    /// Queues replies to `binding`'s conversation as the item's next outbox
    /// rows, in order, each with the round it reports (if any). Empty texts
    /// are skipped. Returns the rows' seqs.
    pub fn enqueue_replies(
        &self,
        tenant_id: &str,
        item_id: &str,
        binding: &ReplyBinding,
        replies: &[(Option<i64>, String)],
    ) -> Result<Vec<i64>, WorkItemError> {
        if binding.conversation_key.trim().is_empty() || binding.connector_id.trim().is_empty() {
            return Err(WorkItemError::Invalid(
                "a reply needs a conversation and a connector".to_string(),
            ));
        }
        let now = Utc::now();
        let rows: Vec<WorkItemOutboxRecord> = replies
            .iter()
            .filter(|(_, text)| !text.trim().is_empty())
            .map(|(round_seq, text)| WorkItemOutboxRecord {
                item_id: item_id.to_string(),
                seq: 0,
                tenant_id: tenant_id.to_string(),
                conversation_key: binding.conversation_key.clone(),
                connector_id: binding.connector_id.clone(),
                conversation_id: binding.conversation_id.clone(),
                channel_id: binding.channel_id.clone(),
                peer_id: binding.peer_id.clone(),
                reply_to_external_message_id: binding.reply_to_external_message_id.clone(),
                participation_gated: binding.participation_gated,
                round_seq: *round_seq,
                source_ref: binding.source_ref.clone(),
                body: text.clone(),
                state: OutboxState::Queued.as_str().to_string(),
                attempts: 0,
                next_attempt_at: now,
                last_error: None,
                drop_reason: None,
                external_message_id: String::new(),
                created_at: now,
                updated_at: now,
                sent_at: None,
                dropped_at: None,
            })
            .collect();
        if rows.is_empty() {
            return Ok(Vec::new());
        }
        let seqs = self
            .store
            .write()
            .enqueue_work_item_outbox(&rows)
            .map_err(WorkItemError::Store)?;
        for (seq, row) in seqs.iter().zip(&rows) {
            let round = row
                .round_seq
                .map_or_else(String::new, |round| format!(", round {round}"));
            eprintln!(
                "[kura] work item: tenant={tenant_id} item={item_id} outbox {seq} -> queued \
                 (connector {}{round})",
                row.connector_id
            );
        }
        Ok(seqs)
    }

    /// Sends the item's unsent rows to one conversation now, in order,
    /// through `lent` (the connector loop's sender for that conversation),
    /// whether their retry is due or not: the person just wrote there. Stops
    /// at the first row that is not sent; the sweeper retries it.
    pub fn flush_conversation(
        &self,
        tenant_id: &str,
        item_id: &str,
        conversation_key: &str,
        connector_id: &str,
        lent: &dyn ReplySender,
    ) -> Flushed {
        let mut flushed = Flushed::default();
        if let Some(_claim) = self.outbox.claim(item_id, conversation_key) {
            loop {
                let head = match self.store.read().work_item_outbox_head(
                    tenant_id,
                    item_id,
                    conversation_key,
                ) {
                    Ok(Some(head)) if head.connector_id == connector_id => head,
                    Ok(_) => break,
                    Err(err) => {
                        log_outbox_failure(tenant_id, item_id, &err);
                        break;
                    }
                };
                match self.attempt(&head, lent, Utc::now()) {
                    Attempt::Sent(id) => flushed.sent_message_ids.push(id),
                    Attempt::Dropped | Attempt::Gone => {}
                    Attempt::Retry(_) => break,
                }
            }
        }
        // Without the claim the sweeper is sending this conversation now;
        // what is left is its to finish.
        flushed.waiting = self
            .store
            .read()
            .list_work_item_outbox(tenant_id, item_id)
            .map(|rows| {
                rows.iter()
                    .filter(|row| row.conversation_key == conversation_key && is_unsent(row))
                    .count()
            })
            .unwrap_or(0);
        if flushed.waiting > 0 {
            self.outbox.poke();
        }
        flushed
    }

    /// One sweep as of `now`: every due row whose connector has a
    /// registered sender, each only once every earlier row of its
    /// conversation is sent or dropped. Returns when the next row is due
    /// within the sweeper's horizon. The sweeper thread runs this; a test
    /// runs it directly, as of a later time, instead of waiting out a
    /// backoff.
    pub fn sweep_outbox(&self, now: DateTime<Utc>) -> Option<DateTime<Utc>> {
        let horizon = Duration::from_std(SWEEP_IDLE).unwrap_or_else(|_| Duration::seconds(30));
        for _ in 0..SWEEP_MAX_PASSES {
            let heads = match self
                .store
                .read()
                .due_work_item_outbox(now + horizon, SWEEP_BATCH)
            {
                Ok(heads) => heads,
                Err(err) => {
                    eprintln!("[kura] work item: outbox sweep failed: {err}");
                    return Some(now + Duration::seconds(OUTBOX_FIRST_BACKOFF_SECS));
                }
            };
            let mut progressed = false;
            let mut next_due = None;
            for row in heads {
                let sender = self.outbox.senders.read().get(&row.connector_id).cloned();
                let Some(sender) = sender else {
                    self.log_unsendable(&row);
                    continue;
                };
                if row.next_attempt_at > now {
                    next_due = earliest(next_due, row.next_attempt_at);
                    continue;
                }
                let Some(_claim) = self.outbox.claim(&row.item_id, &row.conversation_key) else {
                    // The IM hook is sending this conversation; it pokes
                    // the sweeper if it leaves anything unsent.
                    continue;
                };
                // Read again under the claim: the hook may have sent it.
                let head = match self.store.read().work_item_outbox_head(
                    &row.tenant_id,
                    &row.item_id,
                    &row.conversation_key,
                ) {
                    Ok(Some(head)) => head,
                    Ok(None) => continue,
                    Err(err) => {
                        log_outbox_failure(&row.tenant_id, &row.item_id, &err);
                        continue;
                    }
                };
                if head.seq != row.seq || head.next_attempt_at > now {
                    progressed = true;
                    continue;
                }
                match self.attempt(&head, sender.as_ref(), now) {
                    Attempt::Sent(_) | Attempt::Dropped | Attempt::Gone => progressed = true,
                    Attempt::Retry(at) => next_due = earliest(next_due, at),
                }
            }
            if !progressed {
                return next_due;
            }
        }
        // Still sending after every pass: look again at once.
        Some(now)
    }

    /// Starts the daemon's one outbox sweeper. Its first pass, right away,
    /// is the boot sweep: rows a previous run left queued go out (or are
    /// retried when due). Starting it twice does nothing.
    pub fn start_outbox_sweeper(self: &Arc<Self>) -> std::io::Result<()> {
        let mut slot = self.outbox.sweeper.lock();
        if slot.is_some() {
            return Ok(());
        }
        self.outbox.signal.state.lock().stop = false;
        let manager = Arc::downgrade(self);
        let signal = self.outbox.signal.clone();
        let handle = std::thread::Builder::new()
            .name("kura-work-item-outbox".to_string())
            .spawn(move || sweeper_loop(&manager, &signal))?;
        *slot = Some(handle);
        eprintln!("[kura] work item: outbox sweeper started");
        Ok(())
    }

    /// Stops the sweeper and waits for its current pass. Idempotent.
    pub fn stop_outbox_sweeper(&self) {
        {
            let mut state = self.outbox.signal.state.lock();
            state.stop = true;
        }
        self.outbox.signal.wake.notify_all();
        let handle = self.outbox.sweeper.lock().take();
        if let Some(handle) = handle {
            if handle.thread().id() != std::thread::current().id() {
                let _ = handle.join();
            }
        }
    }

    /// A person withdraws a reply not yet sent (§8): it is dropped as
    /// `withdrawn` and its successors go on. Refused while a sender is
    /// working on its conversation, or once it is sent or dropped.
    pub fn withdraw_outbox(
        &self,
        tenant_id: &str,
        item_id: &str,
        seq: i64,
    ) -> Result<WorkItem, WorkItemError> {
        let (tenant_id, item_id) = (tenant_id.trim(), item_id.trim());
        // Not found for another tenant's item, as every read.
        self.get_summary(tenant_id, item_id)?;
        let row = self
            .store
            .read()
            .list_work_item_outbox(tenant_id, item_id)
            .map_err(WorkItemError::Store)?
            .into_iter()
            .find(|row| row.seq == seq)
            .ok_or(WorkItemError::OutboxRowNotFound(seq))?;
        let settled = |row: &WorkItemOutboxRecord| -> WorkItemError {
            WorkItemError::OutboxRowSettled {
                seq,
                state: OutboxState::parse(&row.state).map_or("settled", OutboxState::as_str),
            }
        };
        if !is_unsent(&row) {
            return Err(settled(&row));
        }
        let dropped = {
            // Held across the write, so no sender claims the conversation
            // between the check and the drop.
            let sending = self.outbox.sending.lock();
            if sending.contains(&(row.item_id.clone(), row.conversation_key.clone())) {
                return Err(WorkItemError::OutboxRowSending(seq));
            }
            self.store
                .write()
                .drop_work_item_outbox(
                    tenant_id,
                    item_id,
                    seq,
                    OutboxDropReason::Withdrawn.as_str(),
                    None,
                    Utc::now(),
                )
                .map_err(WorkItemError::Store)?
        };
        if !dropped {
            return Err(settled(&row));
        }
        self.dropped(&row, OutboxDropReason::Withdrawn);
        // Its successors may go now.
        self.outbox.poke();
        self.get(tenant_id, item_id)
    }

    /// Tries one row through `sender`. The caller holds the row's claim.
    fn attempt(
        &self,
        row: &WorkItemOutboxRecord,
        sender: &dyn ReplySender,
        now: DateTime<Utc>,
    ) -> Attempt {
        if row.participation_gated {
            match self.still_receives(row) {
                Ok(true) => {}
                Ok(false) => return self.drop_row(row, OutboxDropReason::ConnectorGone),
                Err(err) => {
                    log_outbox_failure(&row.tenant_id, &row.item_id, &err);
                    return self.retry(row, ERROR_RECEIVE_CHECK_FAILED, now);
                }
            }
        }
        let max_len = sender.reply_progressor().map_or(0, |progressor| {
            progressor.reply_capabilities().max_message_length
        });
        let mut external_id = String::new();
        for part in kura_im::split_reply_content(&row.body, max_len) {
            let mut record = transcript_record(row, &part);
            self.write_transcript(&record);
            let sent = sender.send_reply(OutboundReply {
                connector_id: row.connector_id.clone(),
                channel_id: row.channel_id.clone(),
                content: part,
                reply_to_external_message_id: row.reply_to_external_message_id.clone(),
            });
            record.updated_at = Utc::now();
            let failure = match sent {
                Ok(sent) => {
                    record.status = DeliveryStatus::Replied;
                    record.external_message_id = sent.external_message_id.clone();
                    external_id = sent.external_message_id;
                    None
                }
                Err(err) => {
                    record.status = DeliveryStatus::Failed;
                    record.error = kura_im::safe_reply_failure_reason(None);
                    Some(err)
                }
            };
            record.foreground_outcome_status =
                kura_im::foreground_reply_outcome_status(record.status).to_string();
            self.write_transcript(&record);
            if let Some(err) = failure {
                return match classify_send_error(&err) {
                    Some(reason) => self.drop_row(row, reason),
                    None => self.retry(row, ERROR_SEND_FAILED, now),
                };
            }
        }
        self.mark_sent(row, &external_id)
    }

    /// Whether the connector still receives a gated conversation: a valid
    /// route policy that allows it, as the IM gate and the mandate check
    /// read it (`mandate.rs`).
    fn still_receives(&self, row: &WorkItemOutboxRecord) -> Result<bool, String> {
        let policy = self
            .store
            .read()
            .get_channel_route_policy(&row.tenant_id, &row.connector_id)?;
        Ok(policy
            .filter(route_policy_is_valid)
            .is_some_and(|policy| route_policy_allows_conversation(&policy, &row.conversation_id)))
    }

    fn mark_sent(&self, row: &WorkItemOutboxRecord, external_id: &str) -> Attempt {
        // Bound first: the writer guard must be gone before anything below
        // reads the store again.
        let marked = self.store.write().mark_work_item_outbox_sent(
            &row.tenant_id,
            &row.item_id,
            row.seq,
            external_id,
            Utc::now(),
        );
        match marked {
            Ok(true) => {
                metric("sent");
                eprintln!(
                    "[kura] work item: tenant={} item={} outbox {} {} -> sent (after {} failed \
                     attempts)",
                    row.tenant_id, row.item_id, row.seq, row.state, row.attempts
                );
                Attempt::Sent(external_id.to_string())
            }
            Ok(false) => {
                eprintln!(
                    "[kura] work item: tenant={} item={} outbox {} was sent but had left the \
                     queue meanwhile",
                    row.tenant_id, row.item_id, row.seq
                );
                Attempt::Gone
            }
            Err(err) => {
                // Sent, but not recorded: it is sent again (at least once).
                log_outbox_failure(&row.tenant_id, &row.item_id, &err);
                Attempt::Retry(Utc::now() + outbox_backoff(row.attempts + 1))
            }
        }
    }

    fn retry(&self, row: &WorkItemOutboxRecord, code: &str, now: DateTime<Utc>) -> Attempt {
        let next_at = now + outbox_backoff(row.attempts + 1);
        let marked = self.store.write().mark_work_item_outbox_failed(
            &row.tenant_id,
            &row.item_id,
            row.seq,
            code,
            next_at,
            Utc::now(),
        );
        match marked {
            Ok(Some(failures)) => {
                metric("retry");
                eprintln!(
                    "[kura] work item: tenant={} item={} outbox {} {} -> failed ({code}, {} in a \
                     row; next attempt in {}s)",
                    row.tenant_id,
                    row.item_id,
                    row.seq,
                    row.state,
                    failures,
                    (next_at - now).num_seconds()
                );
                if failures == OUTBOX_PROBLEM_ATTEMPTS {
                    eprintln!(
                        "[kura] work item: tenant={} item={} outbox {} failed {failures} times in \
                         a row; the item shows person/delivery and the row keeps retrying",
                        row.tenant_id, row.item_id, row.seq
                    );
                }
                Attempt::Retry(next_at)
            }
            Ok(None) => Attempt::Gone,
            Err(err) => {
                log_outbox_failure(&row.tenant_id, &row.item_id, &err);
                Attempt::Retry(next_at)
            }
        }
    }

    fn drop_row(&self, row: &WorkItemOutboxRecord, reason: OutboxDropReason) -> Attempt {
        let dropped = self.store.write().drop_work_item_outbox(
            &row.tenant_id,
            &row.item_id,
            row.seq,
            reason.as_str(),
            Some(reason.as_str()),
            Utc::now(),
        );
        match dropped {
            Ok(true) => {
                self.dropped(row, reason);
                Attempt::Dropped
            }
            Ok(false) => Attempt::Gone,
            Err(err) => {
                log_outbox_failure(&row.tenant_id, &row.item_id, &err);
                Attempt::Retry(Utc::now() + outbox_backoff(row.attempts + 1))
            }
        }
    }

    /// The metric, the log line and the event of a drop.
    fn dropped(&self, row: &WorkItemOutboxRecord, reason: OutboxDropReason) {
        metric("dropped");
        eprintln!(
            "[kura] work item: tenant={} item={} outbox {} {} -> dropped ({})",
            row.tenant_id,
            row.item_id,
            row.seq,
            row.state,
            reason.as_str()
        );
        match self.get_summary(&row.tenant_id, &row.item_id) {
            Ok(item) => {
                let mut payload = json!({
                    "outboxSeq": row.seq,
                    "reason": reason.as_str(),
                    "attempts": row.attempts,
                    "connectorId": row.connector_id,
                });
                if let Some(round) = row.round_seq {
                    payload["roundSeq"] = json!(round);
                }
                self.publish(&item, "outbox_dropped", payload);
            }
            Err(err) => log_outbox_failure(&row.tenant_id, &row.item_id, &err.to_string()),
        }
    }

    fn write_transcript(&self, record: &MessageRecord) {
        if let Err(err) = self.store.write().upsert_connector_message(record) {
            eprintln!(
                "[kura] work item: tenant={} recording outbound message {} failed: {err}",
                record.tenant_id, record.delivery_id
            );
        }
    }

    fn log_unsendable(&self, row: &WorkItemOutboxRecord) {
        if self
            .outbox
            .unsendable_logged
            .lock()
            .insert(row.connector_id.clone())
        {
            eprintln!(
                "[kura] work item: outbox rows for connector {} wait: no sender is registered \
                 for it (it is not running here); a message on the conversation retries them",
                row.connector_id
            );
        }
    }
}

fn sweeper_loop(manager: &Weak<Manager>, signal: &Signal) {
    loop {
        if signal.state.lock().stop {
            return;
        }
        let Some(strong) = manager.upgrade() else {
            return;
        };
        let swept = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            strong.sweep_outbox(Utc::now())
        }));
        drop(strong);
        let next = swept.unwrap_or_else(|_| {
            eprintln!("[kura] work item: an outbox sweep panicked; sweeping again shortly");
            None
        });
        let wait = next
            .map(|at| (at - Utc::now()).to_std().unwrap_or(StdDuration::ZERO))
            .unwrap_or(SWEEP_IDLE)
            .clamp(SWEEP_MIN_WAIT, SWEEP_IDLE);
        let mut state = signal.state.lock();
        if state.stop {
            return;
        }
        if !state.poked {
            signal.wake.wait_for(&mut state, wait);
        }
        state.poked = false;
    }
}

/// A send error the platform means for good, by its own error code in the
/// sender's text; `None` (retry) for everything else. The senders return
/// strings, so this is a match on documented platform codes, not on a
/// type: an unknown permanent failure is retried until a person withdraws
/// the row.
///
/// - Slack Web API: `channel_not_found`, `is_archived` (gone);
///   `not_in_channel`, `restricted_action`, `access_denied` (forbidden).
/// - Telegram Bot API: `chat not found`, `group chat was deleted` (gone);
///   `bot was blocked by the user`, `bot was kicked`, `bot is not a member`
///   (forbidden).
/// - Discord REST: `Unknown Channel` (gone); `Missing Access`, `Missing
///   Permissions` (forbidden). Whether the transport keeps the response
///   body's message is unverified.
/// - Matrix: `M_FORBIDDEN` (forbidden).
#[must_use]
pub(crate) fn classify_send_error(error: &str) -> Option<OutboxDropReason> {
    const GONE: &[&str] = &[
        "channel_not_found",
        "is_archived",
        "chat not found",
        "group chat was deleted",
        "unknown channel",
    ];
    const FORBIDDEN: &[&str] = &[
        "not_in_channel",
        "restricted_action",
        "access_denied",
        "bot was blocked by the user",
        "bot was kicked",
        "bot is not a member",
        "missing access",
        "missing permissions",
        "m_forbidden",
    ];
    let error = error.to_ascii_lowercase();
    if GONE.iter().any(|code| error.contains(code)) {
        Some(OutboxDropReason::ConversationGone)
    } else if FORBIDDEN.iter().any(|code| error.contains(code)) {
        Some(OutboxDropReason::Forbidden)
    } else {
        None
    }
}

pub(crate) fn row_view(record: WorkItemOutboxRecord) -> Result<OutboxRow, WorkItemError> {
    let state = OutboxState::parse(&record.state).map_err(WorkItemError::Store)?;
    let unsent = matches!(state, OutboxState::Queued | OutboxState::Failed);
    Ok(OutboxRow {
        seq: record.seq,
        conversation_key: record.conversation_key,
        connector_id: record.connector_id,
        conversation_id: record.conversation_id,
        round_seq: record.round_seq,
        body: record.body,
        state,
        attempts: record.attempts,
        next_attempt_at: unsent.then_some(record.next_attempt_at),
        last_error: record.last_error,
        drop_reason: record
            .drop_reason
            .as_deref()
            .map(OutboxDropReason::parse)
            .transpose()
            .map_err(WorkItemError::Store)?,
        external_message_id: record.external_message_id,
        created_at: record.created_at,
        sent_at: record.sent_at,
        dropped_at: record.dropped_at,
    })
}

fn is_unsent(row: &WorkItemOutboxRecord) -> bool {
    row.state == OutboxState::Queued.as_str() || row.state == OutboxState::Failed.as_str()
}

fn earliest(current: Option<DateTime<Utc>>, at: DateTime<Utc>) -> Option<DateTime<Utc>> {
    Some(current.map_or(at, |current| current.min(at)))
}

fn metric(result: &str) {
    kura_telemetry::metrics::registry().inc(
        kura_telemetry::metrics::WORK_ITEM_OUTBOX_TOTAL,
        &[("result", result)],
    );
}

/// The connector transcript keeps each part sent, as the IM loop's own
/// reply path does: the conversation's history does not depend on who sent.
fn transcript_record(row: &WorkItemOutboxRecord, content: &str) -> MessageRecord {
    let now = Utc::now();
    MessageRecord {
        delivery_id: kura_im::new_delivery_id(),
        tenant_id: row.tenant_id.clone(),
        connector_id: row.connector_id.clone(),
        direction: DeliveryDirection::Outbound,
        channel_id: row.channel_id.clone(),
        peer_id: row.peer_id.clone(),
        content: content.to_string(),
        status: DeliveryStatus::Processing,
        foreground_outcome_status: kura_im::foreground_reply_outcome_status(
            DeliveryStatus::Processing,
        )
        .to_string(),
        response_to_delivery_id: row.source_ref.clone(),
        reply_to_external_message_id: row.reply_to_external_message_id.clone(),
        created_at: now,
        updated_at: now,
        ..MessageRecord::default()
    }
}

fn log_outbox_failure(tenant_id: &str, item_id: &str, message: &str) {
    eprintln!("[kura] work item: tenant={tenant_id} item={item_id} outbox: {message}");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_backoff_doubles_from_5s_to_300s() {
        let secs: Vec<i64> = (1..=8).map(|n| outbox_backoff(n).num_seconds()).collect();
        assert_eq!(secs, vec![5, 10, 20, 40, 80, 160, 300, 300]);
        assert_eq!(outbox_backoff(0).num_seconds(), 5);
        assert_eq!(outbox_backoff(i64::MAX).num_seconds(), 300);
    }

    #[test]
    fn only_a_platforms_permanent_codes_drop_a_row() {
        for (error, reason) in [
            (
                "slack web api error: channel_not_found",
                OutboxDropReason::ConversationGone,
            ),
            (
                "slack web api error: is_archived",
                OutboxDropReason::ConversationGone,
            ),
            (
                "telegram bot api error: Bad Request: chat not found",
                OutboxDropReason::ConversationGone,
            ),
            (
                "slack web api error: not_in_channel",
                OutboxDropReason::Forbidden,
            ),
            (
                "telegram bot api error: Forbidden: bot was blocked by the user",
                OutboxDropReason::Forbidden,
            ),
            (
                "matrix client api error: M_FORBIDDEN",
                OutboxDropReason::Forbidden,
            ),
            (
                "discord: 403 Missing Permissions",
                OutboxDropReason::Forbidden,
            ),
        ] {
            assert_eq!(classify_send_error(error), Some(reason), "{error}");
        }
        for error in [
            "slack web api error: ratelimited",
            "slack web api status 503",
            "telegram bot api error: Too Many Requests: retry after 3",
            "error sending request for url (https://api.telegram.org/bot123:abc/sendMessage)",
            "connection reset",
            "",
        ] {
            assert_eq!(classify_send_error(error), None, "{error}");
        }
    }
}
