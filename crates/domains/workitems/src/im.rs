//! The IM hook (work-items.md §5.2): a message on a conversation an item
//! owns wakes the item instead of starting an ordinary turn.
//!
//! Only the person who asked for the work (the mandate's sender, on the
//! mandate's connector) wakes it. Anyone else's message on that
//! conversation falls through to the ordinary path, gated by participation
//! as before; slice 2 creates no context subscriptions, so it is not
//! recorded on the item either.
//!
//! | The owner's message to an item that is | Effect |
//! |---|---|
//! | open, sender still allowed | input; steers the running round, or runs the next one inline |
//! | open, sender no longer allowed | falls through; the item is not cancelled (§5.4) |
//! | ended on its own report | revalidate: pass → reopen and wake; fail → revoked, falls through |
//! | stopped or revoked | falls through (its subscription is dropped already) |
//!
//! Any failure of the hook's own falls through, so the person still gets
//! the ordinary answer (§11).
//!
//! What the item says back (the round's report, the steered notice) goes
//! through its outbox (`outbox.rs`, slice 3): queued in order for the
//! conversation, then sent at once through the loop's own sender; what
//! fails is retried by the sweeper.

use std::sync::Arc;

use chrono::{Duration, Utc};
use kura_im::{ImTurnOrigin, WorkItemDelivery, WorkItemHook, WorkItemInbound, WorkItemRoute};
use kura_threads::ConversationShape;
use serde_json::json;

use crate::driver::{DriveStop, DrivenRound};
use crate::manager::{Manager, WorkItemError, log_change};
use crate::mandate::{ImContext, im_authority};
use crate::outbox::ReplyBinding;
use crate::types::{EndReason, InputKind, ItemState, SubscriptionRole, WorkItem};
use crate::wake::InputPath;

/// The work-items feature as the IM loop sees it.
pub struct ImHook {
    manager: Arc<Manager>,
}

impl ImHook {
    #[must_use]
    pub fn new(manager: Arc<Manager>) -> Self {
        Self { manager }
    }
}

impl WorkItemHook for ImHook {
    fn route_inbound(&self, inbound: &WorkItemInbound<'_>) -> WorkItemRoute {
        match self.manager.route_im(inbound) {
            Ok(route) => route,
            Err(err) => {
                eprintln!(
                    "[kura] work item: tenant={} connector={} message {} falls through: {err}",
                    inbound.tenant_id, inbound.connector.connector_id, inbound.delivery_id
                );
                WorkItemRoute::FallThrough
            }
        }
    }

    fn turn_started(&self, origin: &ImTurnOrigin) {
        self.manager.register_turn_origin(origin);
    }

    fn turn_finished(&self, tenant_id: &str, run_id: &str) {
        self.manager.unregister_turn_origin(tenant_id, run_id);
    }
}

/// What the person reads in the conversation after a round: its summary,
/// and its question when it asks one.
#[must_use]
pub fn round_reply(round: &DrivenRound) -> String {
    match (&round.report, &round.error_code) {
        (Some(report), _) => {
            let mut reply = report.summary.trim().to_string();
            // The approval wins over the report's own question, as it does
            // for the item's attention (slice 5).
            if let Some(approval) = &round.waits_on {
                reply.push_str(&format!(
                    "\n\nThis needs a person's approval first: {}. Approve or reject approval {} \
                     and the work goes on.",
                    approval.question.trim(),
                    approval.approval_id
                ));
            } else if let Some(attention) = &report.attention {
                reply.push_str("\n\n");
                reply.push_str(attention.question.trim());
                if !attention.options.is_empty() {
                    reply.push_str(&format!("\nOptions: {}", attention.options.join(", ")));
                }
            }
            reply
        }
        (None, Some(code)) => format!(
            "The work item's round could not finish ({code}). It will try again on your next \
             message."
        ),
        (None, None) => String::new(),
    }
}

impl Manager {
    pub(crate) fn route_im(
        &self,
        inbound: &WorkItemInbound<'_>,
    ) -> Result<WorkItemRoute, WorkItemError> {
        let tenant_id = inbound.tenant_id.trim();
        if tenant_id.is_empty() {
            return Ok(WorkItemRoute::FallThrough);
        }
        // Every inbound message of every connector passes here: the common
        // case, a conversation nothing follows, is answered on a reader
        // without taking the writer lock.
        let followed = self
            .store
            .read()
            .live_work_item_subscriptions_for_source(tenant_id, &inbound.source_key, None)
            .map_err(WorkItemError::Store)?;
        if followed.is_empty() {
            return Ok(WorkItemRoute::FallThrough);
        }
        let idle_before = (self.config.subscription_idle_days > 0)
            .then(|| Utc::now() - Duration::days(i64::from(self.config.subscription_idle_days)));
        let owner = self
            .store
            .write()
            .live_work_item_subscriptions_for_source(tenant_id, &inbound.source_key, idle_before)
            .map_err(WorkItemError::Store)?
            .into_iter()
            .find(|sub| sub.role == SubscriptionRole::Owner.as_str());
        let Some(owner) = owner else {
            return Ok(WorkItemRoute::FallThrough);
        };
        let item = self.get_summary(tenant_id, &owner.item_id)?;
        let author = inbound.inbound.author_id.trim();
        let sender = &item.mandate.sender;
        if author.is_empty()
            || author != sender.author_id
            || inbound.connector.connector_id != sender.connector_id
        {
            return Ok(WorkItemRoute::FallThrough);
        }
        let conversation_id = kura_im::inbound_channel_or_conversation_id(inbound.inbound);
        let im = ImContext {
            connector: inbound.connector,
            tenant_id,
            conversation_id: &conversation_id,
            author_id: author,
            shape: inbound.shape,
        };
        let resumed = match (item.state, item.end_reason) {
            (ItemState::Open, _) => {
                if let Err(reason) = im_authority(&self.store.read(), &im)? {
                    eprintln!(
                        "[kura] work item: tenant={} item={} the requester's message falls \
                         through: {}",
                        item.tenant_id,
                        item.item_id,
                        reason.as_str()
                    );
                    return Ok(WorkItemRoute::FallThrough);
                }
                false
            }
            (ItemState::Ended, Some(EndReason::Completed | EndReason::Dropped)) => {
                if let Err(reason) = self.revalidate(&item, Some(&im))? {
                    self.revoke(&item, reason)?;
                    return Ok(WorkItemRoute::FallThrough);
                }
                if !self.reopen(&item)? {
                    eprintln!(
                        "[kura] work item: tenant={} item={} cannot reopen (open-items cap); \
                         the message falls through",
                        item.tenant_id, item.item_id
                    );
                    return Ok(WorkItemRoute::FallThrough);
                }
                true
            }
            _ => return Ok(WorkItemRoute::FallThrough),
        };
        let (input_seq, path) = self.add_input(
            &item.tenant_id,
            &item.item_id,
            InputKind::Message,
            true,
            &inbound.inbound.content,
            &inbound.delivery_id,
        )?;
        if let Err(err) = self.store.write().touch_work_item_subscription(
            &item.tenant_id,
            &owner.subscription_id,
            Utc::now(),
        ) {
            eprintln!(
                "[kura] work item: tenant={} item={} touching subscription failed: {err}",
                item.tenant_id, item.item_id
            );
        }
        let woken = self.get_summary(&item.tenant_id, &item.item_id)?;
        if resumed {
            log_change(&woken, "ended", woken.stage, "resumed by the requester");
        }
        let mut payload = json!({
            "inputSeq": input_seq,
            "kind": InputKind::Message.as_str(),
            "path": path.as_str(),
            "resumed": resumed,
        });
        if let InputPath::Steered { round_seq } = path {
            payload["roundSeq"] = json!(round_seq);
        }
        self.publish(&woken, "woken", payload);
        let binding = ReplyBinding {
            conversation_key: inbound.source_key.clone(),
            connector_id: inbound.connector.connector_id.clone(),
            conversation_id: conversation_id.clone(),
            channel_id: inbound.inbound.channel_id.clone(),
            peer_id: inbound.inbound.peer_id.clone(),
            reply_to_external_message_id: kura_im::reply_to_external_message_id(inbound.inbound),
            participation_gated: matches!(
                inbound.shape,
                ConversationShape::Group | ConversationShape::Room
            ) && kura_im::connector_enforces_group_room_participation_policy(
                inbound.connector,
            ),
            source_ref: inbound.delivery_id.clone(),
        };
        if let InputPath::Steered { .. } = path {
            let replies = vec![(None, "Added to the work item's running round.".to_string())];
            return Ok(self.reply(&item, &binding, replies, "work_item_steered", inbound));
        }

        let drive = self.drive(&item.tenant_id, &item.item_id, None);
        let mut replies: Vec<(Option<i64>, String)> = drive
            .rounds
            .iter()
            .map(|round| (Some(round.seq), round_reply(round)))
            .filter(|(_, reply)| !reply.is_empty())
            .collect();
        if drive.rounds.is_empty() {
            match &drive.stopped {
                Some(DriveStop::Throttled { code }) => replies.push((
                    None,
                    format!(
                        "Your message reached the work item \"{}\", but its next round could \
                         not start now ({code}). It runs when the item is woken again.",
                        item.title
                    ),
                )),
                Some(DriveStop::HourlyCap { limit }) => replies.push((
                    None,
                    format!(
                        "Your message reached the work item \"{}\". It has run its {limit} \
                         rounds for this hour; your message waits until the item is woken \
                         again.",
                        item.title
                    ),
                )),
                // A round of the item is running elsewhere and will read
                // the message when it settles.
                None => {}
            }
        }
        let reason_code = if drive.rounds.is_empty() {
            "work_item_queued"
        } else {
            "work_item_round"
        };
        Ok(self.reply(&item, &binding, replies, reason_code, inbound))
    }

    /// Queues the replies to the message the item took in its outbox and
    /// sends them now through the loop's own sender (§8). If they cannot
    /// even be queued, they go back to the loop to send inline, as slice 2
    /// did, so the person still reads them.
    fn reply(
        &self,
        item: &WorkItem,
        binding: &ReplyBinding,
        replies: Vec<(Option<i64>, String)>,
        reason_code: &str,
        inbound: &WorkItemInbound<'_>,
    ) -> WorkItemRoute {
        let texts: Vec<String> = replies.iter().map(|(_, text)| text.clone()).collect();
        match self.enqueue_replies(&item.tenant_id, &item.item_id, binding, &replies) {
            Ok(_) => {
                let flushed = self.flush_conversation(
                    &item.tenant_id,
                    &item.item_id,
                    &binding.conversation_key,
                    &binding.connector_id,
                    inbound.replies,
                );
                WorkItemRoute::Claimed {
                    replies: Vec::new(),
                    reason_code: reason_code.to_string(),
                    delivered: WorkItemDelivery {
                        texts,
                        sent_message_ids: flushed.sent_message_ids,
                        waiting: flushed.waiting,
                    },
                }
            }
            Err(err) => {
                eprintln!(
                    "[kura] work item: tenant={} item={} queueing replies failed, sent inline \
                     without retry: {err}",
                    item.tenant_id, item.item_id
                );
                WorkItemRoute::Claimed {
                    replies: texts,
                    reason_code: reason_code.to_string(),
                    delivered: WorkItemDelivery::default(),
                }
            }
        }
    }
}
