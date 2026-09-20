//! Live-validation preflight quota gate (port of
//! `live_validation_preflight.go`).

use crate::denial::new_quota_state_unavailable_denial;
use crate::error::BillingError;
use crate::error::Result;
use crate::manager::Manager;
use crate::manager::ReserveInput;
use crate::manager::ReserveResult;
use crate::operation_key::live_validation_operation_key;
use crate::types::Category;

pub const LIVE_VALIDATION_PREFLIGHT_ENTRY_POINT: &str = "Roadmap 38 live-validation preflight gate";

/// Reserve one live-validation attempt at the preflight gate. Without a
/// manager, hosted tenants fail closed and non-hosted tenants pass.
pub async fn reserve_live_validation_preflight(
    manager: Option<&Manager>,
    tenant_id: &str,
    validation_id: &str,
    client_key: &str,
    hosted: bool,
) -> Result<ReserveResult> {
    let operation_key = live_validation_operation_key(tenant_id, validation_id, client_key);
    let Some(manager) = manager else {
        if hosted {
            let denial = new_quota_state_unavailable_denial(tenant_id, &operation_key);
            return Ok(ReserveResult {
                allowed: false,
                denial: Some(denial),
                failure: Some(BillingError::QuotaStateUnavailable),
                ..Default::default()
            });
        }
        return Ok(ReserveResult {
            allowed: true,
            ..Default::default()
        });
    };
    manager
        .reserve(ReserveInput {
            tenant_id: tenant_id.to_string(),
            category: Category::from(Category::LIVE_VALIDATION_ATTEMPTS),
            amount: 1,
            operation_key,
            reservation_point: LIVE_VALIDATION_PREFLIGHT_ENTRY_POINT.to_string(),
            guarded_entry_point: LIVE_VALIDATION_PREFLIGHT_ENTRY_POINT.to_string(),
            hosted,
            ..Default::default()
        })
        .await
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::fixtures::FixtureRepo;
    use crate::fixtures::TEN_FINITE;
    use crate::fixtures::fixed_now;

    #[tokio::test]
    async fn preflight_fails_closed_without_manager_when_hosted() {
        let hosted = reserve_live_validation_preflight(None, TEN_FINITE, "validation_1", "", true)
            .await
            .unwrap();
        assert!(!hosted.allowed);
        assert!(matches!(
            hosted.failure,
            Some(BillingError::QuotaStateUnavailable)
        ));
        assert!(hosted.denial.is_some());

        let dev = reserve_live_validation_preflight(None, TEN_FINITE, "validation_1", "", false)
            .await
            .unwrap();
        assert!(dev.allowed);
    }

    #[tokio::test]
    async fn preflight_reserves_with_manager() {
        let now = fixed_now();
        let repo = Arc::new(FixtureRepo::new(now));
        let manager = Manager::with_clock(repo, move || now);
        let result =
            reserve_live_validation_preflight(Some(&manager), TEN_FINITE, "validation_1", "", true)
                .await
                .unwrap();
        assert!(result.allowed, "{result:?}");
        let reservation = result.reservation.expect("reservation");
        assert_eq!(reservation.category, Category::LIVE_VALIDATION_ATTEMPTS);
        assert_eq!(
            reservation.operation_key,
            "tenant:ten_finite:live_validation:validation_1"
        );
    }
}
