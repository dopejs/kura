//! Attention and approvals (work-items.md §7, slice 5).
//!
//! An item's attention is the one thing a person must do for its work to go
//! on. It is set only by a round: from the report's structured `attention`
//! (a question), or by the daemon when a tool call of the round gave up
//! waiting on an approval that is still pending (an approval). It is never
//! read from prose.
//!
//! | Attention | Cleared by | Then |
//! |---|---|---|
//! | question | the requester's next message, or `POST …/answer {text}` | the answer is a person's input; the next round reads it |
//! | approval | its resolution (`/v1/policy/approvals/{id}/resolve`) | an `approval` input says how; the next round runs the call once under it, if it calls it again with the same arguments |
//!
//! **No second approval system.** The policy engine stays the only place a
//! person grants a call. A round's tools remember, through
//! [`kura_policy::TurnApprovals`], which approval belongs to which call (a
//! fingerprint of its action, resource and arguments): a later call that
//! is the same waits on, or runs under, that approval instead of raising
//! another, and runs under it at most once.
//!
//! **Waking on resolve.** The route publishes `policy.approval_resolved`.
//! One watcher thread per daemon (`kura-work-item-approvals`) follows the
//! bus and, because the bus drops what a full subscriber cannot take,
//! sweeps every pending link against the engine on start (an approval
//! resolved while the daemon was down) and every 30 s.

use std::sync::Arc;
use std::time::{Duration, Instant};

use chrono::Utc;
use kura_policy::{ApprovalCall, ApprovalStatus, TurnApprovals};
use kura_store::work_item_approvals::WorkItemApprovalRecord;
use kura_store::work_items::WorkItemInputRecord;
use serde_json::json;
use sha2::{Digest, Sha256};

use crate::manager::{Manager, WorkItemError, log_change, stage_name, to_view};
use crate::types::{
    Attention, AttentionEntry, AttentionKind, InputKind, ItemState, Stage, WorkItem,
};

/// How often the watcher checks every pending link against the engine.
pub const APPROVAL_SWEEP_INTERVAL: Duration = Duration::from_secs(30);

/// Pending links one sweep checks; the rest wait for the next.
const MAX_SWEEP: usize = 500;

/// Open items the attention queue looks at, oldest first.
const MAX_QUEUE_SCAN: usize = 1000;

/// Longest answer `POST …/answer` takes, as a scope.
const MAX_ANSWER_CHARS: usize = 16 * 1024;

/// The watcher's longest block on the bus, so a stop is seen quickly.
const WATCH_TICK: Duration = Duration::from_millis(500);

/// Which call an approval is for: SHA-256 over its action, resource kind,
/// resource id and arguments. The arguments are read as JSON and written
/// back in one canonical form (keys sorted, no spacing), so the model
/// writing the same arguments differently is still the same call; empty
/// arguments are `{}`, as the MCP tool reads them.
#[must_use]
pub fn call_fingerprint(call: &ApprovalCall<'_>) -> String {
    let arguments = call.arguments.trim();
    let arguments = if arguments.is_empty() {
        "{}".to_string()
    } else {
        serde_json::from_str::<serde_json::Value>(arguments)
            .map_or_else(|_| arguments.to_string(), |value| value.to_string())
    };
    let mut hasher = Sha256::new();
    for part in [
        call.action.trim(),
        call.resource_kind.trim(),
        call.resource_id.trim(),
        arguments.as_str(),
    ] {
        hasher.update(part.as_bytes());
        hasher.update([0u8]);
    }
    let digest = hasher.finalize();
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// What a work item's round remembers about its approvals: the
/// [`TurnApprovals`] its MCP tools are built with.
struct RoundApprovals {
    manager: Arc<Manager>,
    tenant_id: String,
    item_id: String,
}

impl TurnApprovals for RoundApprovals {
    fn held(&self, call: &ApprovalCall<'_>) -> Option<String> {
        let fingerprint = call_fingerprint(call);
        match self.manager.store.read().held_work_item_approval(
            &self.tenant_id,
            &self.item_id,
            &fingerprint,
        ) {
            Ok(held) => held.map(|link| link.approval_id),
            Err(err) => {
                log_failure(
                    &self.tenant_id,
                    &self.item_id,
                    &format!("reading held approvals failed: {err}"),
                );
                None
            }
        }
    }

    fn gave_up(
        &self,
        call: &ApprovalCall<'_>,
        approval: &kura_policy::Approval,
        decision: Option<&kura_policy::Decision>,
    ) -> Option<String> {
        match self.manager.record_approval_wait(
            &self.tenant_id,
            &self.item_id,
            call,
            approval,
            decision,
        ) {
            Ok(true) => Some(
                "This work item now waits for that approval: end this round with the report \
                 status \"waiting\". When a person resolves it, the item's next round is told \
                 how; calling the tool again with the same arguments then runs it once under \
                 the approval."
                    .to_string(),
            ),
            Ok(false) => None,
            Err(err) => {
                log_failure(
                    &self.tenant_id,
                    &self.item_id,
                    &format!(
                        "recording the wait on approval {} failed: {err}",
                        approval.approval_id
                    ),
                );
                None
            }
        }
    }

    fn used(&self, approval_id: &str) {
        match self.manager.store.write().use_work_item_approval(
            &self.tenant_id,
            &self.item_id,
            approval_id,
            Utc::now(),
        ) {
            Ok(true) => {
                metric("used");
                eprintln!(
                    "[kura] work item: tenant={} item={} runs the call approval {approval_id} \
                     granted",
                    self.tenant_id, self.item_id
                );
            }
            // Not an approval the item waited on: the call was approved
            // while it was still waiting, as in any turn.
            Ok(false) => {}
            Err(err) => log_failure(
                &self.tenant_id,
                &self.item_id,
                &format!("marking approval {approval_id} used failed: {err}"),
            ),
        }
    }
}

impl Manager {
    /// The policy engine whose approvals items wait on. Without one, rounds
    /// remember no approvals and nothing wakes on a resolution.
    #[must_use]
    pub fn with_policy(mut self, policy: Arc<kura_policy::Engine>) -> Self {
        self.policy = Some(policy);
        self
    }

    /// What a turn on `thread_id` remembers about its approvals: `Some`
    /// only for a work item's round (its own `thr_wi_` thread) while the
    /// policy engine is known. Its MCP tools are built with it.
    #[must_use]
    pub fn round_approvals(
        self: &Arc<Self>,
        tenant_id: &str,
        thread_id: &str,
    ) -> Option<Arc<dyn TurnApprovals>> {
        let tenant_id = tenant_id.trim();
        let hex = thread_id.trim().strip_prefix("thr_wi_")?;
        if tenant_id.is_empty() || hex.is_empty() || self.policy.is_none() {
            return None;
        }
        Some(Arc::new(RoundApprovals {
            manager: Arc::clone(self),
            tenant_id: tenant_id.to_string(),
            item_id: format!("wi_{hex}"),
        }))
    }

    /// A round's call gave up waiting on `approval`: the open item now
    /// waits on it. The approval is written to the approvals table and
    /// bound to the item's tenant (the MCP manager keeps it in memory
    /// only), so the tenant lists and resolves it and a restart restores
    /// it. `false` when nothing was recorded: the item is not open, or the
    /// approval belongs to another tenant or item.
    pub(crate) fn record_approval_wait(
        &self,
        tenant_id: &str,
        item_id: &str,
        call: &ApprovalCall<'_>,
        approval: &kura_policy::Approval,
        decision: Option<&kura_policy::Decision>,
    ) -> Result<bool, WorkItemError> {
        let now = Utc::now();
        let store = self.store.write();
        let Some((record, _)) = store
            .get_work_item(tenant_id, item_id)
            .map_err(WorkItemError::Store)?
        else {
            return Ok(false);
        };
        if record.state != ItemState::Open.as_str() {
            return Ok(false);
        }
        store
            .upsert_approval(approval)
            .map_err(WorkItemError::Store)?;
        if let Err(err) =
            store.bind_row_tenant("approvals", "approval_id", &approval.approval_id, tenant_id)
        {
            if kura_store::SQLiteStore::is_cross_tenant_row(&err) {
                log_failure(
                    tenant_id,
                    item_id,
                    &format!(
                        "approval {} belongs to another tenant; not waiting on it",
                        approval.approval_id
                    ),
                );
                return Ok(false);
            }
            return Err(WorkItemError::Store(err));
        }
        if let Some(decision) = decision {
            store
                .upsert_decision(decision)
                .map_err(WorkItemError::Store)?;
            if let Err(err) =
                store.bind_row_tenant("decisions", "decision_id", &decision.decision_id, tenant_id)
            {
                if !kura_store::SQLiteStore::is_cross_tenant_row(&err) {
                    return Err(WorkItemError::Store(err));
                }
            }
        }
        let recorded = store
            .record_work_item_approval_wait(&WorkItemApprovalRecord {
                approval_id: approval.approval_id.clone(),
                tenant_id: tenant_id.to_string(),
                item_id: item_id.to_string(),
                round_seq: record.seq,
                call_fingerprint: call_fingerprint(call),
                question: approval.reason.clone(),
                state: "pending".to_string(),
                created_at: now,
                updated_at: now,
                resolved_at: None,
                used_at: None,
            })
            .map_err(WorkItemError::Store)?;
        drop(store);
        if recorded {
            metric("waiting");
            eprintln!(
                "[kura] work item: tenant={tenant_id} item={item_id} round {} waits on approval \
                 {}",
                record.seq, approval.approval_id
            );
        }
        Ok(recorded)
    }

    /// Reads the approval from the engine and, if it is no longer pending,
    /// applies its outcome to the item that waits on it: the link is
    /// resolved, the item's attention cleared if it was this approval, an
    /// `approval` input recorded, and the item driven on a thread of its
    /// own. Applied once; `false` when there was nothing to do.
    pub fn approval_resolved(self: &Arc<Self>, approval_id: &str) -> Result<bool, WorkItemError> {
        let approval_id = approval_id.trim();
        let Some(policy) = &self.policy else {
            return Ok(false);
        };
        let Some(link) = self
            .store
            .read()
            .work_item_approval(approval_id)
            .map_err(WorkItemError::Store)?
        else {
            return Ok(false);
        };
        if link.state != "pending" {
            return Ok(false);
        }
        let outcome = match policy.get_approval(approval_id).map(|a| a.status) {
            Some(ApprovalStatus::Pending) => return Ok(false),
            Some(ApprovalStatus::Approved) => "approved",
            Some(ApprovalStatus::Rejected) => "rejected",
            None => "gone",
        };
        self.apply_approval_outcome(&link, outcome)
    }

    fn apply_approval_outcome(
        self: &Arc<Self>,
        link: &WorkItemApprovalRecord,
        outcome: &str,
    ) -> Result<bool, WorkItemError> {
        let now = Utc::now();
        let store = self.store.write();
        let item = store
            .get_work_item(&link.tenant_id, &link.item_id)
            .map_err(WorkItemError::Store)?;
        let (input, attention_json) = match &item {
            Some((record, _)) if record.state == ItemState::Open.as_str() => {
                let input = WorkItemInputRecord {
                    item_id: link.item_id.clone(),
                    seq: 0,
                    tenant_id: link.tenant_id.clone(),
                    kind: InputKind::Approval.as_str().to_string(),
                    author_is_person: false,
                    body: outcome_body(outcome, &link.question),
                    source_ref: link.approval_id.clone(),
                    state: "pending".to_string(),
                    round_seq: None,
                    created_at: now,
                };
                let attention_json = record.attention_json.clone().filter(|json| {
                    serde_json::from_str::<Attention>(json).is_ok_and(|attention| {
                        attention.kind == AttentionKind::Approval
                            && attention.approval_id == link.approval_id
                    })
                });
                (Some(input), attention_json)
            }
            // An ended item only has its link settled: nothing wakes it.
            _ => (None, None),
        };
        let resolution = store
            .resolve_work_item_approval(
                &link.approval_id,
                outcome,
                now,
                input.as_ref(),
                attention_json.as_deref(),
            )
            .map_err(WorkItemError::Store)?;
        drop(store);
        if !resolution.applied {
            return Ok(false);
        }
        metric(outcome);
        let Some(input_seq) = resolution.input_seq else {
            eprintln!(
                "[kura] work item: tenant={} item={} approval {} {outcome}; the item has ended, \
                 nothing wakes",
                link.tenant_id, link.item_id, link.approval_id
            );
            return Ok(true);
        };
        kura_telemetry::metrics::registry().inc(
            kura_telemetry::metrics::WORK_ITEM_INPUTS_TOTAL,
            &[("kind", InputKind::Approval.as_str()), ("path", "queued")],
        );
        let after = self.get_summary(&link.tenant_id, &link.item_id)?;
        let before = if attention_json.is_some() {
            Some(Stage::PersonApproval)
        } else {
            after.stage
        };
        log_change(
            &after,
            stage_name(before),
            after.stage,
            &format!("approval {} {outcome}", link.approval_id),
        );
        if resolution.attention_cleared {
            self.publish(
                &after,
                "attention_cleared",
                json!({"by": "approval", "approvalId": link.approval_id}),
            );
        }
        self.publish(
            &after,
            "woken",
            json!({
                "inputSeq": input_seq,
                "kind": InputKind::Approval.as_str(),
                "path": "queued",
                "resumed": false,
                "approvalId": link.approval_id,
            }),
        );
        self.drive_detached(&link.tenant_id, &link.item_id);
        Ok(true)
    }

    /// Checks every pending link against the engine (oldest first, at most
    /// 500) and applies what was resolved. Returns how many were.
    pub fn sweep_approvals(self: &Arc<Self>) -> usize {
        let pending = match self.store.read().pending_work_item_approvals(MAX_SWEEP) {
            Ok(pending) => pending,
            Err(err) => {
                eprintln!("[kura] work item: listing pending approvals failed: {err}");
                return 0;
            }
        };
        let mut resolved = 0;
        for link in pending {
            match self.approval_resolved(&link.approval_id) {
                Ok(true) => resolved += 1,
                Ok(false) => {}
                Err(err) => log_failure(
                    &link.tenant_id,
                    &link.item_id,
                    &format!("applying approval {} failed: {err}", link.approval_id),
                ),
            }
        }
        resolved
    }

    /// Starts the daemon's one approval watcher (module docs): its first
    /// pass sweeps every pending link, which catches what was resolved
    /// while the daemon was down. `Ok(false)` when it runs already, or
    /// there is no policy engine to watch.
    pub fn start_approval_watcher(self: &Arc<Self>) -> std::io::Result<bool> {
        if self.policy.is_none() {
            return Ok(false);
        }
        let mut slot = self.approval_watcher.lock();
        if slot.is_some() {
            return Ok(false);
        }
        self.approval_watcher_stop
            .store(false, std::sync::atomic::Ordering::SeqCst);
        // Subscribed before the first sweep, so nothing resolved between
        // the two is missed.
        let (events, unsubscribe) = self.bus.subscribe(kura_events::Filter {
            category: "policy".to_string(),
            resource_kind: "approval".to_string(),
            ..kura_events::Filter::default()
        });
        let manager = Arc::downgrade(self);
        let handle = std::thread::Builder::new()
            .name("kura-work-item-approvals".to_string())
            .spawn(move || {
                let _unsubscribe = unsubscribe;
                watch_approvals(&manager, &events);
            })?;
        *slot = Some(handle);
        eprintln!("[kura] work item: approval watcher started");
        Ok(true)
    }

    /// Stops the watcher and waits for its current pass. Idempotent.
    pub fn stop_approval_watcher(&self) {
        self.approval_watcher_stop
            .store(true, std::sync::atomic::Ordering::SeqCst);
        let handle = self.approval_watcher.lock().take();
        if let Some(handle) = handle {
            if handle.thread().id() != std::thread::current().id() {
                let _ = handle.join();
            }
        }
    }

    /// A person answers the item's question (`POST …/answer`, §7): the
    /// question is cleared and the answer recorded as a person's input in
    /// one write, then the item is driven on a thread of its own; what its
    /// rounds report goes to the conversation it owns, through the outbox.
    /// Refused as [`WorkItemError::NotAsking`] when it asks nothing now:
    /// it ended, nothing is asked, or it waits on an approval, which is
    /// resolved through the approval and not answered.
    pub fn answer(
        self: &Arc<Self>,
        tenant_id: &str,
        item_id: &str,
        text: &str,
        source_ref: &str,
    ) -> Result<WorkItem, WorkItemError> {
        let (tenant_id, item_id, text) = (tenant_id.trim(), item_id.trim(), text.trim());
        if text.is_empty() {
            return Err(WorkItemError::Invalid("text is required".to_string()));
        }
        if text.chars().count() > MAX_ANSWER_CHARS {
            return Err(WorkItemError::Invalid(format!(
                "text is longer than {MAX_ANSWER_CHARS} characters"
            )));
        }
        let store = self.store.write();
        let (record, activity) = store
            .get_work_item(tenant_id, item_id)
            .map_err(WorkItemError::Store)?
            .ok_or(WorkItemError::NotFound)?;
        if record.state != ItemState::Open.as_str() {
            return Err(WorkItemError::NotAsking(
                "the work item has ended".to_string(),
            ));
        }
        let Some(attention_json) = record.attention_json.clone() else {
            return Err(WorkItemError::NotAsking(
                "the work item asks no question now".to_string(),
            ));
        };
        let attention: Attention = serde_json::from_str(&attention_json)
            .map_err(|err| WorkItemError::Store(format!("decode attention: {err}")))?;
        if attention.kind == AttentionKind::Approval {
            return Err(WorkItemError::NotAsking(format!(
                "the work item waits on approval {id}; resolve it with \
                 POST /v1/policy/approvals/{id}/resolve",
                id = attention.approval_id
            )));
        }
        let before = to_view(record, &activity)?;
        let input = WorkItemInputRecord {
            item_id: item_id.to_string(),
            seq: 0,
            tenant_id: tenant_id.to_string(),
            kind: InputKind::Text.as_str().to_string(),
            author_is_person: true,
            body: text.to_string(),
            source_ref: source_ref.trim().to_string(),
            state: "pending".to_string(),
            round_seq: None,
            created_at: Utc::now(),
        };
        let input_seq = store
            .answer_work_item(tenant_id, item_id, &attention_json, &input)
            .map_err(WorkItemError::Store)?
            .ok_or_else(|| {
                WorkItemError::NotAsking("the work item's question changed; read it again".into())
            })?;
        drop(store);
        kura_telemetry::metrics::registry().inc(
            kura_telemetry::metrics::WORK_ITEM_INPUTS_TOTAL,
            &[("kind", InputKind::Text.as_str()), ("path", "queued")],
        );
        let after = self.get_summary(tenant_id, item_id)?;
        log_change(
            &after,
            stage_name(before.stage),
            after.stage,
            "a person answered its question",
        );
        self.publish(&after, "attention_cleared", json!({"by": "answer"}));
        self.publish(
            &after,
            "woken",
            json!({
                "inputSeq": input_seq,
                "kind": InputKind::Text.as_str(),
                "path": "queued",
                "resumed": false,
            }),
        );
        self.drive_detached(tenant_id, item_id);
        Ok(after)
    }

    /// The attention queue (`GET /v1/work-items/attention`, §7): the
    /// tenant's open items in a `person/*` stage, the least recently
    /// updated (the longest waiting) first, each with its stage, its
    /// question or approval, and its title.
    pub fn attention_queue(&self, tenant_id: &str) -> Result<Vec<AttentionEntry>, WorkItemError> {
        let rows = self
            .store
            .read()
            .list_open_work_items_oldest_first(tenant_id.trim(), MAX_QUEUE_SCAN)
            .map_err(WorkItemError::Store)?;
        let mut entries = Vec::new();
        for (record, activity) in rows {
            let item = to_view(record, &activity)?;
            let Some(stage) = item
                .stage
                .filter(|stage| stage.as_str().starts_with("person/"))
            else {
                continue;
            };
            let attention = match stage {
                Stage::PersonAnswer | Stage::PersonApproval => item.attention.clone(),
                _ => None,
            };
            let summary = match stage {
                Stage::PersonDelivery => None,
                _ => item
                    .last_report
                    .as_ref()
                    .map(|report| report.summary.clone()),
            };
            entries.push(AttentionEntry {
                item_id: item.item_id,
                title: item.title,
                stage,
                basis: item.mandate.basis,
                attention,
                summary,
                updated_at: item.updated_at,
            });
        }
        Ok(entries)
    }

    /// Drives the item on a thread of its own, for a wake nobody waits on
    /// (an answer through the API, a resolved approval). What its rounds
    /// report goes to the conversation the item owns, through the outbox,
    /// as after a restart's rerun; an item that owns none is read through
    /// the API and events.
    pub(crate) fn drive_detached(self: &Arc<Self>, tenant_id: &str, item_id: &str) {
        let manager = Arc::clone(self);
        let (tenant, item) = (tenant_id.to_string(), item_id.to_string());
        let spawned = std::thread::Builder::new()
            .name("kura-work-item-round".to_string())
            .spawn(move || {
                let (tenant_id, item_id) = (tenant.as_str(), item.as_str());
                let drive = manager.drive(tenant_id, item_id, None);
                if let Some(stop) = &drive.stopped {
                    log_failure(
                        tenant_id,
                        item_id,
                        &format!(
                            "its next round did not start now: {stop:?}; it waits for the next wake"
                        ),
                    );
                }
                if drive.rounds.is_empty() {
                    return;
                }
                let replies: Vec<(Option<i64>, String)> = drive
                    .rounds
                    .iter()
                    .map(|round| (Some(round.seq), crate::im::round_reply(round)))
                    .filter(|(_, reply)| !reply.is_empty())
                    .collect();
                match manager.get_summary(tenant_id, item_id) {
                    Ok(item) => manager.reply_to_owner(&item, replies),
                    Err(err) => log_failure(
                        tenant_id,
                        item_id,
                        &format!("reading the item to reply failed: {err}"),
                    ),
                }
            });
        if let Err(err) = spawned {
            log_failure(
                tenant_id,
                item_id,
                &format!("spawning a round thread failed: {err}"),
            );
        }
    }
}

fn watch_approvals(
    manager: &std::sync::Weak<Manager>,
    events: &std::sync::mpsc::Receiver<kura_events::Event>,
) {
    let mut next_sweep = Instant::now();
    loop {
        let Some(this) = manager.upgrade() else {
            return;
        };
        if this
            .approval_watcher_stop
            .load(std::sync::atomic::Ordering::SeqCst)
        {
            return;
        }
        if Instant::now() >= next_sweep {
            this.sweep_approvals();
            next_sweep = Instant::now() + APPROVAL_SWEEP_INTERVAL;
        }
        drop(this);
        match events.recv_timeout(WATCH_TICK) {
            Ok(event) => {
                if event.name != "policy.approval_resolved" {
                    continue;
                }
                let Some(this) = manager.upgrade() else {
                    return;
                };
                if let Err(err) = this.approval_resolved(&event.resource.id) {
                    eprintln!(
                        "[kura] work item: applying approval {} failed: {err}",
                        event.resource.id
                    );
                }
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => return,
        }
    }
}

/// What the item's next round is told about the approval it waited on.
/// Written by the daemon (framed `from="daemon"`); the question is the
/// approval's reason, which quotes the call's arguments.
fn outcome_body(outcome: &str, question: &str) -> String {
    let question = question.trim();
    match outcome {
        "approved" => format!(
            "A person approved the tool call you asked about ({question}). It has not run yet. \
             If it is still needed, call the tool again with the same arguments: it runs once \
             under this approval."
        ),
        "rejected" => format!(
            "A person rejected the tool call you asked about ({question}). It was not run. Do \
             not try it again as it was; say in your report what this means for the work."
        ),
        _ => format!(
            "The approval for the tool call you asked about ({question}) no longer exists, so \
             the call was not run. If it is still needed, calling the tool again asks again."
        ),
    }
}

fn metric(event: &str) {
    kura_telemetry::metrics::registry().inc(
        kura_telemetry::metrics::WORK_ITEM_APPROVALS_TOTAL,
        &[("event", event)],
    );
}

fn log_failure(tenant_id: &str, item_id: &str, message: &str) {
    eprintln!("[kura] work item: tenant={tenant_id} item={item_id} {message}");
}

#[cfg(test)]
mod tests {
    use super::*;

    fn call<'a>(resource_id: &'a str, arguments: &'a str) -> ApprovalCall<'a> {
        ApprovalCall {
            action: "tool_call.execute",
            resource_kind: "mcp_tool",
            resource_id,
            arguments,
        }
    }

    #[test]
    fn the_same_call_written_differently_has_one_fingerprint() {
        let a = call_fingerprint(&call("srv:advance:chat", r#"{"b":1,"a":"x"}"#));
        assert_eq!(
            a,
            call_fingerprint(&call("srv:advance:chat", "{ \"a\": \"x\", \"b\": 1 }"))
        );
        assert_eq!(a.len(), 64);
        assert_ne!(
            a,
            call_fingerprint(&call("srv:advance:chat", r#"{"a":"y","b":1}"#))
        );
        assert_ne!(
            a,
            call_fingerprint(&call("srv:advance:cron", r#"{"a":"x","b":1}"#))
        );
        assert_eq!(
            call_fingerprint(&call("srv:ping:chat", "")),
            call_fingerprint(&call("srv:ping:chat", " {} "))
        );
        // Parts cannot run into each other.
        assert_ne!(
            call_fingerprint(&call("srv:a", "{}")),
            call_fingerprint(&ApprovalCall {
                action: "tool_call.execute",
                resource_kind: "mcp_toolsrv:a",
                resource_id: "",
                arguments: "{}",
            })
        );
    }

    fn manager() -> (Arc<Manager>, Arc<kura_events::Bus>) {
        let dir = std::env::temp_dir().join(format!(
            "kura-workitems-attention-{}",
            uuid::Uuid::now_v7().simple()
        ));
        let store = kura_store::SQLiteStore::new(dir.to_str().expect("path")).expect("store");
        let pool = Arc::new(kura_store::StorePool::writer_only(Arc::new(
            parking_lot::Mutex::new(store),
        )));
        let bus = Arc::new(kura_events::Bus::new());
        let config = crate::types::WorkItemsConfig {
            enabled: true,
            ..crate::types::WorkItemsConfig::default()
        };
        let manager = Manager::new(pool, bus.clone(), config)
            .with_policy(Arc::new(kura_policy::Engine::new()));
        (Arc::new(manager), bus)
    }

    /// Opens an item for `tenant` and settles its first round with `report`.
    fn settled(manager: &Manager, tenant: &str, report: serde_json::Value) -> WorkItem {
        let (_, turn) = manager
            .open(crate::manager::OpenRequest {
                tenant_id: tenant.to_string(),
                title: format!("Item of {tenant}"),
                scope: "Do it.".to_string(),
                ..crate::manager::OpenRequest::default()
            })
            .expect("open");
        manager
            .settle_round(
                &turn,
                crate::manager::RoundOutcome::Answered {
                    structured: Some(report),
                    turn_id: None,
                },
            )
            .expect("settle")
    }

    #[test]
    fn an_answer_clears_the_question_once_and_becomes_a_persons_input() {
        let (manager, bus) = manager();
        let item = settled(
            &manager,
            "ten_a",
            json!({"status": "blocked", "summary": "Two fixes.",
                   "attention": {"question": "Pin or patch?", "options": ["pin", "patch"]}}),
        );
        assert_eq!(item.stage, Some(Stage::PersonAnswer));
        assert_eq!(
            item.attention.as_ref().unwrap().kind,
            AttentionKind::Question
        );
        let (events, _unsubscribe) = bus.subscribe(kura_events::Filter::default());

        assert_eq!(
            manager
                .answer("ten_b", &item.item_id, "pin", "req_1")
                .unwrap_err(),
            WorkItemError::NotFound
        );
        assert!(matches!(
            manager.answer("ten_a", &item.item_id, "  ", "req_1"),
            Err(WorkItemError::Invalid(_))
        ));
        let after = manager
            .answer("ten_a", &item.item_id, " pin ", "req_1")
            .expect("answer");
        assert!(after.attention.is_none());
        // No chat service here, so the round cannot start: the answer waits.
        assert_eq!(after.stage, Some(Stage::Queued));
        let read = manager.get("ten_a", &item.item_id).unwrap();
        let input = read.inputs.unwrap().pop().unwrap();
        assert_eq!(input.kind, InputKind::Text);
        assert!(input.author_is_person);
        assert_eq!(input.body, "pin");
        assert_eq!(input.source_ref, "req_1");
        assert!(matches!(
            manager.answer("ten_a", &item.item_id, "patch", "req_2"),
            Err(WorkItemError::NotAsking(_))
        ));

        let names: Vec<String> = std::iter::from_fn(|| events.try_recv().ok())
            .map(|event| {
                let body = serde_json::to_string(&event.payload).unwrap();
                assert!(!body.contains("pin"), "events carry no answer: {body}");
                event.name
            })
            .collect();
        assert_eq!(names, ["work_item.attention_cleared", "work_item.woken"]);

        let ended = settled(
            &manager,
            "ten_a",
            json!({"status": "ready", "summary": "Done."}),
        );
        assert!(matches!(
            manager.answer("ten_a", &ended.item_id, "x", ""),
            Err(WorkItemError::NotAsking(_))
        ));
    }

    #[test]
    fn the_queue_lists_only_the_tenants_items_waiting_on_a_person_oldest_first() {
        let (manager, _) = manager();
        let asking = settled(
            &manager,
            "ten_a",
            json!({"status": "waiting", "summary": "Need a choice.",
                   "attention": {"question": "Which?"}}),
        );
        settled(
            &manager,
            "ten_a",
            json!({"status": "waiting", "summary": "Waiting for CI."}),
        );
        let blocked = settled(
            &manager,
            "ten_a",
            json!({"status": "blocked", "summary": "No access."}),
        );
        settled(
            &manager,
            "ten_b",
            json!({"status": "blocked", "summary": "Theirs."}),
        );
        let queue = manager.attention_queue("ten_a").unwrap();
        let ids: Vec<&str> = queue.iter().map(|entry| entry.item_id.as_str()).collect();
        assert_eq!(ids, [asking.item_id.as_str(), blocked.item_id.as_str()]);
        assert_eq!(queue[0].stage, Stage::PersonAnswer);
        assert_eq!(queue[0].attention.as_ref().unwrap().question, "Which?");
        assert_eq!(queue[1].stage, Stage::PersonBlocked);
        assert!(queue[1].attention.is_none());
        assert_eq!(queue[1].summary.as_deref(), Some("No access."));
        assert_eq!(manager.attention_queue("ten_b").unwrap().len(), 1);
        assert!(manager.attention_queue("ten_c").unwrap().is_empty());
    }

    #[test]
    fn a_round_that_left_an_approval_pending_waits_on_it_unless_its_report_ends_the_item() {
        let (manager, _) = manager();
        let policy = manager.policy.clone().unwrap();
        for (status, waits) in [("progress", true), ("blocked", true), ("ready", false)] {
            let (item, turn) = manager
                .open(crate::manager::OpenRequest {
                    tenant_id: "ten_a".to_string(),
                    title: "Deploy".to_string(),
                    scope: "Deploy.".to_string(),
                    ..crate::manager::OpenRequest::default()
                })
                .unwrap();
            let (approval, decision) = policy
                .request_approval(kura_policy::RequestApprovalInput {
                    action: "tool_call.execute".to_string(),
                    resource_kind: "mcp_tool".to_string(),
                    resource_id: "srv:advance:chat".to_string(),
                    reason: "run advance".to_string(),
                    requested_by: "agent".to_string(),
                    ..kura_policy::RequestApprovalInput::default()
                })
                .unwrap();
            let call = call("srv:advance:chat", "{}");
            assert!(
                manager
                    .record_approval_wait("ten_a", &item.item_id, &call, &approval, Some(&decision))
                    .unwrap()
            );
            // Another tenant cannot wait on it.
            assert!(
                !manager
                    .record_approval_wait("ten_b", &item.item_id, &call, &approval, None)
                    .unwrap()
            );
            let after = manager
                .settle_round(
                    &turn,
                    crate::manager::RoundOutcome::Answered {
                        structured: Some(json!({"status": status, "summary": "s",
                            "attention": {"question": "And then?"}})),
                        turn_id: None,
                    },
                )
                .unwrap();
            if waits {
                assert_eq!(after.stage, Some(Stage::PersonApproval), "{status}");
                let attention = after.attention.unwrap();
                assert_eq!(attention.kind, AttentionKind::Approval);
                assert_eq!(attention.approval_id, approval.approval_id);
                assert_eq!(attention.question, "run advance");
                // The report keeps the model's own question.
                assert_eq!(
                    after.last_report.unwrap().attention.unwrap().question,
                    "And then?"
                );
                assert!(matches!(
                    manager.answer("ten_a", &item.item_id, "yes", ""),
                    Err(WorkItemError::NotAsking(_))
                ));
            } else {
                assert_eq!(after.state, ItemState::Ended, "{status}");
            }
            // The approval is the tenant's now.
            let owned = manager
                .store
                .read()
                .list_approvals_for_tenant_raw("ten_a")
                .unwrap();
            assert!(
                owned
                    .iter()
                    .any(|row| row.approval_id == approval.approval_id)
            );
        }
    }

    #[test]
    fn only_a_rounds_own_thread_remembers_approvals() {
        let (manager, _) = manager();
        assert!(manager.round_approvals("ten_a", "thr_wi_0193").is_some());
        assert!(manager.round_approvals("ten_a", "thr_src_1").is_none());
        assert!(manager.round_approvals("", "thr_wi_0193").is_none());
        assert!(manager.round_approvals("ten_a", "thr_wi_").is_none());
        let dir = std::env::temp_dir().join(format!(
            "kura-workitems-attention-{}",
            uuid::Uuid::now_v7().simple()
        ));
        let store = kura_store::SQLiteStore::new(dir.to_str().unwrap()).unwrap();
        let without_policy = Arc::new(Manager::new(
            Arc::new(kura_store::StorePool::writer_only(Arc::new(
                parking_lot::Mutex::new(store),
            ))),
            Arc::new(kura_events::Bus::new()),
            crate::types::WorkItemsConfig::default(),
        ));
        assert!(
            without_policy
                .round_approvals("ten_a", "thr_wi_0193")
                .is_none()
        );
        assert!(!without_policy.start_approval_watcher().unwrap());
    }
}
