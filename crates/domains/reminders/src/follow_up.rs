//! Port of daemon/internal/reminders/follow_up.go: follow-up link staleness refresh.
//!
//! FR-006: cross-tenant existence MUST NOT leak. A run link is resolved against the
//! caller's tenant id (falling back to the bootstrapped default personal tenant); a
//! missing tenant binding is treated as "the source is not visible to the caller, so
//! the link is stale" — the same semantics the tenancy layer enforces for reads of
//! foreign tenant-owned rows.

use std::sync::Arc;

use chrono::Utc;
use kura_identity::tenantctx;
use kura_integrations::DiagnosticReasonCode;
use kura_integrations::diagnostic_failure_for_reason;
use kura_store::SQLiteStore;
use parking_lot::Mutex;

use crate::types::{FollowUpLink, FollowUpLinkKind, ReminderError};

/// Go refreshFollowUpLink: re-checks whether the linked source still exists and projects
/// staleness + a diagnostic failure when it does not. Uses real wall-clock time for the
/// LastCheckedAt stamp, exactly like the Go implementation.
pub fn refresh_follow_up_link(
    store: &Arc<Mutex<SQLiteStore>>,
    environment_scope: &str,
    link: &Option<FollowUpLink>,
) -> Result<Option<FollowUpLink>, ReminderError> {
    let Some(link) = link else {
        return Ok(None);
    };
    let mut out = link.clone();
    let now = Utc::now();
    out.last_checked_at = Some(now);

    let stale = match out.link_kind {
        FollowUpLinkKind::Run => {
            let tenant_id = resolve_tenant_id(store);
            if tenant_id.is_empty() {
                // No visible tenant binding: the source is not visible to the caller.
                true
            } else {
                let exists = store
                    .lock()
                    .run_exists_for_tenant(&out.source_id, &tenant_id)
                    .map_err(|e| {
                        ReminderError::Store(format!("refresh follow-up run link: {e}"))
                    })?;
                !exists
            }
        }
        FollowUpLinkKind::Workflow => {
            let exists = store
                .lock()
                .get_workflow_by_id(environment_scope, &out.source_id)
                .map_err(|e| ReminderError::Store(format!("refresh follow-up workflow link: {e}")))?
                .is_some();
            !exists
        }
        FollowUpLinkKind::CalendarOperation => {
            let exists = store
                .lock()
                .get_calendar_operation_by_id(environment_scope, &out.source_id)
                .map_err(|e| ReminderError::Store(format!("refresh follow-up calendar link: {e}")))?
                .is_some();
            !exists
        }
        FollowUpLinkKind::MailOperation => {
            let exists = store
                .lock()
                .get_mail_operation_by_id(environment_scope, &out.source_id)
                .map_err(|e| ReminderError::Store(format!("refresh follow-up mail link: {e}")))?
                .is_some();
            !exists
        }
    };
    out.stale = stale;
    if stale && out.source_display_state.is_empty() {
        out.source_display_state = "stale".to_string();
    }
    if stale {
        out.diagnostic_failure = Some(diagnostic_failure_for_reason(
            DiagnosticReasonCode::OperatorActionNeeded,
            now,
        ));
    }
    Ok(Some(out))
}

/// Go resolveTenantID: the caller's resolved tenant id, falling back to the bootstrapped
/// default personal tenant. Returns "" only when neither is available (pre-bootstrap
/// call paths); callers treat "" as "no visible tenant" rather than as an error, keeping
/// the follow-up projection robust on the very first daemon boot.
pub fn resolve_tenant_id(store: &Arc<Mutex<SQLiteStore>>) -> String {
    if let Some(tc) = tenantctx::from_context() {
        if !tc.tenant_id.is_empty() {
            return tc.tenant_id;
        }
    }
    store
        .lock()
        .resolve_default_tenant_binding()
        .unwrap_or_default()
}

/// Go cloneFollowUpLink: shallow copy of the link value.
#[must_use]
pub fn clone_follow_up_link(item: &Option<FollowUpLink>) -> Option<FollowUpLink> {
    item.clone()
}
