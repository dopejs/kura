//! Restart recovery (work-items.md §6, slice 4).
//!
//! A round is a chat turn, and a turn dies with its process. At boot the
//! rounds a previous process left `running` are found and each is either
//! rerun or ended:
//!
//! | The round's item | `recoverOnBoot` | Effect |
//! |---|---|---|
//! | open, under the limits | true | rerun with the same request id, stored question and thread, so the chat layer finds the round's checkpoint: settled calls replay, a started side-effecting call reads `INTERRUPTED_CALL_OUTPUT`. No checkpoint, or one older than 24 h: the round runs fresh. Either way its instructions carry a recovery note that quotes the item's outbound log. |
//! | open | false | ended as `failed`/`interrupted`; its inputs are pending again and the item shows `person/delivery` until a person writes |
//! | open, rerun twice already, over the hourly cap, or refused admission | true | as for `false`, with that reason |
//! | stopped or revoked | either | ended as `failed`/`interrupted`; nothing reruns |
//!
//! Only one scan reruns a round: claiming it is a compare-and-set in the
//! store (the round must still be running and have started before this
//! process came up; the claim moves its start to now). A rerun round, or one
//! this process started, is never seen as left behind.
//!
//! The scan runs on a thread of its own after the daemon is assembled, with
//! at most [`RECOVERY_WORKERS`] rounds at a time, each through turn
//! admission; boot does not wait for it.

use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::atomic::Ordering;

use chrono::{Duration, Utc};
use kura_chat::Admitted;
use kura_store::work_items::{WorkItemInputRecord, WorkItemRoundRecord};
use serde_json::json;

use crate::driver::DrivenRound;
use crate::manager::{Manager, RoundTurn, WorkItemError, log_change, stage_name};
use crate::outbox::ReplyBinding;
use crate::report::{OutboundEntry, recovery_note, round_instructions};
use crate::types::{InputKind, ItemState, Stage, SubscriptionRole, WorkItem};

/// Rounds the boot scan reruns at the same time. Turn admission still
/// bounds them with every other turn; this keeps a burst of recoveries from
/// taking every slot at boot.
pub const RECOVERY_WORKERS: usize = 2;

/// Times a boot reruns one round. A round that took the process down twice
/// is not tried a third time: it waits for a person.
pub const MAX_RERUNS_PER_ROUND: i64 = 2;

/// Rounds one boot scan looks at; more are left for the next boot (and
/// logged).
const MAX_ROUNDS_PER_SCAN: usize = 1000;

/// How long a checkpoint waits for the retry that would resume it. The chat
/// service's own TTL (`kura_chat` `resume.rs`, `CHECKPOINT_TTL_HOURS`);
/// older checkpoints are pruned there, and not resumed here.
const CHECKPOINT_TTL_HOURS: i64 = 24;

/// Admission attempts for a rerun refused over the turn rate or token
/// budget, waiting 2 s, 4 s, 8 s between them.
const ADMIT_ATTEMPTS: u32 = 4;

/// How a round left behind was rerun.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecoveryMode {
    /// From its checkpoint.
    Resumed,
    /// From its question: there was no checkpoint, or it was too old.
    Fresh,
}

impl RecoveryMode {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Resumed => "resumed",
            Self::Fresh => "fresh",
        }
    }
}

/// Why a round left behind was ended instead of rerun.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotRerun {
    /// `recoverOnBoot` is false.
    RecoverOff,
    /// The item was stopped or revoked while the round ran.
    ItemEnded,
    /// Boots reran the round [`MAX_RERUNS_PER_ROUND`] times already.
    RecoveryLimit,
    /// The item started `maxRoundsPerItemPerHour` other rounds this hour.
    HourlyCap,
    /// Turn admission refused the rerun every time it was asked.
    Throttled,
}

impl NotRerun {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::RecoverOff => "recover_off",
            Self::ItemEnded => "item_ended",
            Self::RecoveryLimit => "recovery_limit",
            Self::HourlyCap => "hourly_cap",
            Self::Throttled => "throttled",
        }
    }
}

/// What the boot did with a round left behind.
#[derive(Debug, Clone, PartialEq)]
pub enum Recovery {
    Rerun {
        mode: RecoveryMode,
        /// How the rerun ended.
        round: DrivenRound,
    },
    Interrupted {
        reason: NotRerun,
    },
}

/// One round a previous process left running, and what became of it.
#[derive(Debug, Clone, PartialEq)]
pub struct RecoveredRound {
    pub tenant_id: String,
    pub item_id: String,
    pub seq: i64,
    pub recovery: Recovery,
}

impl Manager {
    /// Starts the boot recovery on a thread of its own: once per manager,
    /// whatever the caller does. `Ok(false)` when it was started already.
    pub fn start_recovery(self: &Arc<Self>) -> std::io::Result<bool> {
        if self.recovery_started.swap(true, Ordering::SeqCst) {
            return Ok(false);
        }
        let manager = Arc::clone(self);
        let spawned = std::thread::Builder::new()
            .name("kura-work-item-recovery".to_string())
            .spawn(move || {
                manager.recover_interrupted_rounds();
            });
        match spawned {
            Ok(_) => Ok(true),
            Err(err) => {
                self.recovery_started.store(false, Ordering::SeqCst);
                Err(err)
            }
        }
    }

    /// The boot scan: every round of every tenant left running by a
    /// previous process, oldest first, rerun or ended (module docs).
    /// Blocking; returns what it did to each round it handled. A round
    /// another scan claimed first is that scan's and is not listed.
    pub fn recover_interrupted_rounds(self: &Arc<Self>) -> Vec<RecoveredRound> {
        let rounds = match self
            .store
            .read()
            .list_interrupted_work_item_rounds(self.boot_at, MAX_ROUNDS_PER_SCAN)
        {
            Ok(rounds) => rounds,
            Err(err) => {
                eprintln!("[kura] work item: boot recovery could not list rounds: {err}");
                return Vec::new();
            }
        };
        eprintln!(
            "[kura] work item: boot recovery found {} round(s) left running (recoverOnBoot {})",
            rounds.len(),
            self.config.recover_on_boot
        );
        if rounds.len() == MAX_ROUNDS_PER_SCAN {
            eprintln!(
                "[kura] work item: boot recovery handles {MAX_ROUNDS_PER_SCAN} rounds; the rest \
                 wait for the next boot"
            );
        }
        if rounds.is_empty() {
            return Vec::new();
        }
        let workers = RECOVERY_WORKERS.min(rounds.len());
        let queue = parking_lot::Mutex::new(VecDeque::from(rounds));
        let done = parking_lot::Mutex::new(Vec::new());
        std::thread::scope(|scope| {
            for _ in 0..workers {
                scope.spawn(|| {
                    loop {
                        let Some(round) = queue.lock().pop_front() else {
                            break;
                        };
                        if let Some(recovered) = self.recover_round(&round) {
                            done.lock().push(recovered);
                        }
                    }
                });
            }
        });
        done.into_inner()
    }

    /// Reruns or ends one round left behind. `None` when another scan
    /// claimed it first, or the store failed (logged).
    fn recover_round(self: &Arc<Self>, round: &WorkItemRoundRecord) -> Option<RecoveredRound> {
        match self.try_recover_round(round) {
            Ok(Some(recovery)) => Some(RecoveredRound {
                tenant_id: round.tenant_id.clone(),
                item_id: round.item_id.clone(),
                seq: round.seq,
                recovery,
            }),
            Ok(None) => None,
            Err(err) => {
                eprintln!(
                    "[kura] work item: tenant={} item={} round {} boot replying failed: {err}",
                    round.tenant_id, round.item_id, round.seq
                );
                None
            }
        }
    }

    fn try_recover_round(
        self: &Arc<Self>,
        round: &WorkItemRoundRecord,
    ) -> Result<Option<Recovery>, WorkItemError> {
        let (tenant_id, item_id, seq) = (&round.tenant_id, &round.item_id, round.seq);
        let item = self.get_summary(tenant_id, item_id)?;
        if let Some(reason) = self.why_not_rerun(round, &item)? {
            return self.interrupt(round, &item, reason);
        }
        let Some(admitted) = self.admit_rerun(tenant_id, item_id, seq) else {
            return self.interrupt(round, &item, NotRerun::Throttled);
        };

        // Whether the chat layer will find the round's checkpoint. One too
        // old to resume is deleted once the round is claimed: the chat
        // layer would otherwise still load it.
        let now = Utc::now();
        let cutoff = now - Duration::hours(CHECKPOINT_TTL_HOURS);
        let checkpoints = self
            .store
            .read()
            .turn_checkpoints_for_request(tenant_id, &item.thread_id, &round.request_id)
            .map_err(WorkItemError::Store)?;
        let resumable = checkpoints.iter().any(|record| {
            record.updated_at >= cutoff
                && serde_json::from_str::<kura_core::TurnCheckpoint>(&record.checkpoint_json)
                    .is_ok_and(|checkpoint| {
                        !checkpoint.rounds.is_empty() || !checkpoint.steered.is_empty()
                    })
        });
        let mode = if resumable {
            RecoveryMode::Resumed
        } else {
            RecoveryMode::Fresh
        };

        let outbox = self
            .store
            .read()
            .list_work_item_outbox(tenant_id, item_id)
            .map_err(WorkItemError::Store)?;
        let log: Vec<OutboundEntry<'_>> = outbox
            .iter()
            .map(|row| OutboundEntry {
                seq: row.seq,
                state: &row.state,
                body: &row.body,
            })
            .collect();
        let note = recovery_note(mode == RecoveryMode::Resumed, &log);
        let note_input = WorkItemInputRecord {
            item_id: item_id.clone(),
            seq: 0,
            tenant_id: tenant_id.clone(),
            kind: InputKind::Recovery.as_str().to_string(),
            author_is_person: false,
            body: note.clone(),
            source_ref: round.request_id.clone(),
            state: "consumed".to_string(),
            round_seq: Some(seq),
            created_at: now,
        };
        let claimed = self
            .store
            .write()
            .claim_work_item_round_recovery(tenant_id, item_id, seq, self.boot_at, &note_input, now)
            .map_err(WorkItemError::Store)?;
        if claimed.is_none() {
            // Another scan has it (the slot is released with `admitted`).
            return Ok(None);
        }
        for stale in checkpoints
            .iter()
            .filter(|record| record.updated_at < cutoff)
        {
            if let Err(err) = self.store.write().delete_turn_checkpoint(&stale.turn_key) {
                eprintln!(
                    "[kura] work item: tenant={tenant_id} item={item_id} round {seq}: deleting \
                     a checkpoint too old to resume failed, the rerun may resume it: {err}"
                );
            }
        }
        kura_telemetry::metrics::registry().inc(
            kura_telemetry::metrics::WORK_ITEM_RECOVERED_TOTAL,
            &[("mode", mode.as_str())],
        );
        log_change(
            &item,
            stage_name(item.stage),
            Some(Stage::Working),
            &format!("round {seq} rerun after a restart, {}", mode.as_str()),
        );

        let turn = RoundTurn {
            tenant_id: tenant_id.clone(),
            item_id: item_id.clone(),
            seq,
            request_id: round.request_id.clone(),
            thread_id: item.thread_id.clone(),
            query_text: round.query_text.clone(),
            instructions: format!(
                "{}\n\n{note}",
                round_instructions(&item.title, &item.mandate.scope_text)
            ),
            provider: item.provider.clone(),
            model: item.model.clone(),
        };
        let rerun = self.run_round(&turn, admitted);
        // What arrived meanwhile, or waited since before the restart, runs
        // next, as after any round.
        let drive = self.drive(tenant_id, item_id, None);
        let mut replies = vec![(Some(rerun.seq), crate::im::round_reply(&rerun))];
        replies.extend(
            drive
                .rounds
                .iter()
                .map(|round| (Some(round.seq), crate::im::round_reply(round))),
        );
        self.reply_to_owner(&item, replies);
        Ok(Some(Recovery::Rerun { mode, round: rerun }))
    }

    /// Why the round must not be rerun, if it must not.
    fn why_not_rerun(
        &self,
        round: &WorkItemRoundRecord,
        item: &WorkItem,
    ) -> Result<Option<NotRerun>, WorkItemError> {
        if item.state != ItemState::Open {
            return Ok(Some(NotRerun::ItemEnded));
        }
        if !self.config.recover_on_boot {
            return Ok(Some(NotRerun::RecoverOff));
        }
        let reruns = self
            .store
            .read()
            .count_work_item_round_recoveries(&round.tenant_id, &round.item_id, round.seq)
            .map_err(WorkItemError::Store)?;
        if reruns >= MAX_RERUNS_PER_ROUND {
            return Ok(Some(NotRerun::RecoveryLimit));
        }
        let limit = self.config.max_rounds_per_item_per_hour;
        if limit > 0 {
            // The rerun counts as a start; the round's own earlier start
            // does not count twice.
            let since = Utc::now() - Duration::hours(1);
            let started = self
                .store
                .read()
                .count_work_item_rounds_since(&round.tenant_id, &round.item_id, since)
                .map_err(WorkItemError::Store)?;
            let others = started - i64::from(round.started_at >= since);
            if others >= i64::from(limit) {
                return Ok(Some(NotRerun::HourlyCap));
            }
        }
        Ok(None)
    }

    /// A turn slot for the rerun, waiting out a refusal a few times: at
    /// boot the turn rate may be spent by every recovery at once.
    fn admit_rerun(&self, tenant_id: &str, item_id: &str, seq: i64) -> Option<Admitted> {
        let chat = self.chat.as_ref()?;
        let mut wait = std::time::Duration::from_secs(2);
        for attempt in 1..=ADMIT_ATTEMPTS {
            match chat.admit(tenant_id) {
                Ok(admitted) => return Some(admitted),
                Err(err) if attempt == ADMIT_ATTEMPTS => {
                    eprintln!(
                        "[kura] work item: tenant={tenant_id} item={item_id} round {seq}: \
                         admission refused the rerun {ADMIT_ATTEMPTS} times ({})",
                        err.code()
                    );
                }
                Err(_) => {
                    std::thread::sleep(wait);
                    wait *= 2;
                }
            }
        }
        None
    }

    /// Ends the round as interrupted, without a rerun. An open item gets
    /// the round's inputs back, so the round that next runs reads them,
    /// and shows `person/delivery` until a person writes.
    fn interrupt(
        &self,
        round: &WorkItemRoundRecord,
        item: &WorkItem,
        reason: NotRerun,
    ) -> Result<Option<Recovery>, WorkItemError> {
        let applied = self
            .store
            .write()
            .interrupt_work_item_round(
                &round.tenant_id,
                &round.item_id,
                round.seq,
                self.boot_at,
                item.state == ItemState::Open,
                Utc::now(),
            )
            .map_err(WorkItemError::Store)?;
        if !applied {
            // Another scan reran or ended it first.
            return Ok(None);
        }
        let metrics = kura_telemetry::metrics::registry();
        metrics.inc(
            kura_telemetry::metrics::WORK_ITEM_RECOVERED_TOTAL,
            &[("mode", "skipped")],
        );
        metrics.inc(
            kura_telemetry::metrics::WORK_ITEM_ROUNDS_TOTAL,
            &[("outcome", "interrupted")],
        );
        let after = self.get_summary(&round.tenant_id, &round.item_id)?;
        log_change(
            &after,
            stage_name(item.stage),
            after.stage,
            &format!(
                "round {} interrupted by a restart, not rerun: {}",
                round.seq,
                reason.as_str()
            ),
        );
        let mut payload = json!({
            "roundSeq": round.seq,
            "requestId": round.request_id,
            "state": "failed",
            "outcome": "interrupted",
            "reason": reason.as_str(),
        });
        if let Some(stage) = after.stage {
            payload["stage"] = json!(stage.as_str());
        }
        self.publish(&after, "round_settled", payload);
        Ok(Some(Recovery::Interrupted { reason }))
    }

    /// Queues what rounds no IM message woke report (a rerun after a
    /// restart; slice 5: a round after an answer or an approval) for the
    /// conversation the item owns, through its outbox. No message is there
    /// to answer, so the reply goes where the item's latest
    /// reply to that conversation went. An item that never replied there
    /// (opened through the API, or interrupted on its first IM wake) has no
    /// address to reply to; its report is read through the API and events.
    pub(crate) fn reply_to_owner(&self, item: &WorkItem, replies: Vec<(Option<i64>, String)>) {
        let binding = match self.last_reply_binding(item) {
            Ok(Some(binding)) => binding,
            Ok(None) => return,
            Err(err) => {
                eprintln!(
                    "[kura] work item: tenant={} item={} finding where to reply failed: {err}",
                    item.tenant_id, item.item_id
                );
                return;
            }
        };
        match self.enqueue_replies(&item.tenant_id, &item.item_id, &binding, &replies) {
            Ok(seqs) if !seqs.is_empty() => self.outbox.poke(),
            Ok(_) => {}
            Err(err) => eprintln!(
                "[kura] work item: tenant={} item={} queueing replies to its conversation \
                 failed: {err}",
                item.tenant_id, item.item_id
            ),
        }
    }

    fn last_reply_binding(&self, item: &WorkItem) -> Result<Option<ReplyBinding>, WorkItemError> {
        let store = self.store.read();
        let owner = store
            .list_work_item_subscriptions(&item.tenant_id, &item.item_id)
            .map_err(WorkItemError::Store)?
            .into_iter()
            .find(|sub| sub.role == SubscriptionRole::Owner.as_str() && sub.dropped_at.is_none());
        let Some(owner) = owner else {
            return Ok(None);
        };
        let last = store
            .list_work_item_outbox(&item.tenant_id, &item.item_id)
            .map_err(WorkItemError::Store)?
            .into_iter()
            .rev()
            .find(|row| row.conversation_key == owner.source_key);
        Ok(last.map(|row| ReplyBinding {
            conversation_key: row.conversation_key,
            connector_id: row.connector_id,
            conversation_id: row.conversation_id,
            channel_id: row.channel_id,
            peer_id: row.peer_id,
            reply_to_external_message_id: row.reply_to_external_message_id,
            participation_gated: row.participation_gated,
            source_ref: row.source_ref,
        }))
    }
}
