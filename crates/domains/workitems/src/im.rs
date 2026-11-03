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

use std::sync::Arc;

use chrono::{Duration, Utc};
use kura_im::{ImTurnOrigin, WorkItemHook, WorkItemInbound, WorkItemRoute};
use serde_json::json;

use crate::driver::{DriveStop, DrivenRound};
use crate::manager::{Manager, WorkItemError, log_change};
use crate::mandate::{ImContext, im_authority};
use crate::types::{EndReason, InputKind, ItemState, SubscriptionRole};
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
            if let Some(attention) = &report.attention {
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
        if let InputPath::Steered { .. } = path {
            return Ok(WorkItemRoute::Claimed {
                replies: vec!["Added to the work item's running round.".to_string()],
                reason_code: "work_item_steered".to_string(),
            });
        }

        let drive = self.drive(&item.tenant_id, &item.item_id, None);
        let mut replies: Vec<String> = drive
            .rounds
            .iter()
            .map(round_reply)
            .filter(|reply| !reply.is_empty())
            .collect();
        if drive.rounds.is_empty() {
            match &drive.stopped {
                Some(DriveStop::Throttled { code }) => replies.push(format!(
                    "Your message reached the work item \"{}\", but its next round could not \
                     start now ({code}). It runs when the item is woken again.",
                    item.title
                )),
                Some(DriveStop::HourlyCap { limit }) => replies.push(format!(
                    "Your message reached the work item \"{}\". It has run its {limit} rounds \
                     for this hour; your message waits until the item is woken again.",
                    item.title
                )),
                // A round of the item is running elsewhere and will read
                // the message when it settles.
                None => {}
            }
        }
        Ok(WorkItemRoute::Claimed {
            replies,
            reason_code: if drive.rounds.is_empty() {
                "work_item_queued"
            } else {
                "work_item_round"
            }
            .to_string(),
        })
    }
}
