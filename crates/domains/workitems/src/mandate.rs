//! Whether a mandate still holds (work-items.md §5.4).
//!
//! Checked by basis, never by a fresh evaluation of the work. The fast path
//! (the tenant's policy hash equals the mandate's) is never taken in slice
//! 2: the hash stays empty until work rules exist (slice 6), and an empty
//! hash never takes it.

use kura_connectors::{Connector, Status};
use kura_store::SQLiteStore;
use kura_store::channel_management::{
    route_policy_allows_conversation, route_policy_allows_sender, route_policy_is_valid,
};
use kura_threads::ConversationShape;

use crate::manager::{Manager, WorkItemError};
use crate::types::{Basis, RevokeReason, WorkItem};

/// The IM side of a check: the connector a message arrived on, and who sent
/// it where.
pub(crate) struct ImContext<'a> {
    pub connector: &'a Connector,
    pub tenant_id: &'a str,
    pub conversation_id: &'a str,
    pub author_id: &'a str,
    pub shape: ConversationShape,
}

/// Whether the sender may still direct work in that conversation: the
/// connector is enabled and, for a group or a room whose connector enforces
/// participation, the route policy still receives the conversation and
/// still allows the sender. That is the allowlist step of
/// `evaluate_participation` with the mention requirement waived: exactly
/// what the ordinary path would let through, minus the mention. A direct
/// message has no allowlist step on the ordinary path, so none here.
pub(crate) fn im_authority(
    store: &SQLiteStore,
    im: &ImContext<'_>,
) -> Result<Result<(), RevokeReason>, WorkItemError> {
    if matches!(
        im.connector.status,
        Status::Disabled | Status::Failed | Status::BackingOff
    ) {
        return Ok(Err(RevokeReason::ConnectorDisabled));
    }
    let gated = matches!(im.shape, ConversationShape::Group | ConversationShape::Room)
        && kura_im::connector_enforces_group_room_participation_policy(im.connector);
    if !gated {
        return Ok(Ok(()));
    }
    let policy = store
        .get_channel_route_policy(im.tenant_id, &im.connector.connector_id)
        .map_err(WorkItemError::Store)?;
    let Some(policy) = policy.filter(route_policy_is_valid) else {
        return Ok(Err(RevokeReason::ConversationNotAllowed));
    };
    if !route_policy_allows_conversation(&policy, im.conversation_id) {
        return Ok(Err(RevokeReason::ConversationNotAllowed));
    }
    if !route_policy_allows_sender(&policy, im.author_id) {
        return Ok(Err(RevokeReason::SenderNotAllowed));
    }
    Ok(Ok(()))
}

impl Manager {
    /// Revalidates the mandate of an ended item before it resumes (§5.4).
    /// `im` is the message that would resume it, for the `im_*` bases.
    pub(crate) fn revalidate(
        &self,
        item: &WorkItem,
        im: Option<&ImContext<'_>>,
    ) -> Result<Result<(), RevokeReason>, WorkItemError> {
        let verdict = match item.mandate.basis {
            Basis::Api => {
                if self.principal_is_member(&item.tenant_id, &item.mandate.sender.principal_id)? {
                    Ok(())
                } else {
                    Err(RevokeReason::PrincipalNotMember)
                }
            }
            Basis::ImMention | Basis::ImDirect => match im {
                Some(im) if im.connector.connector_id == item.mandate.sender.connector_id => {
                    im_authority(&self.store.read(), im)?
                }
                // Only a message on the mandate's own connector can resume
                // an IM item; anything else has nothing to check against.
                _ => Err(RevokeReason::ConnectorDisabled),
            },
            // Rules arrive with slice 6; nothing can hold this basis yet.
            Basis::Rule => Err(RevokeReason::BasisUnsupported),
        };
        if verdict.is_ok() {
            kura_telemetry::metrics::registry().inc(
                kura_telemetry::metrics::WORK_ITEM_MANDATE_CHECKS_TOTAL,
                &[("result", "pass")],
            );
        }
        Ok(verdict)
    }
}
