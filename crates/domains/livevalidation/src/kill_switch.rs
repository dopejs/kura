//! Kill switches (port of `kill_switch.go`).

use chrono::DateTime;
use chrono::Utc;

use kura_identity::can_resolve_live_validation_reconciliation;
use kura_identity::tenantctx;

use crate::error::LiveValidationError;
use crate::ledger::LedgerOutcome;
use crate::ledger::is_terminal_ledger_outcome;
use crate::manager::Denial;
use crate::manager::Manager;
use crate::manager::first_non_empty;
use crate::store::AttemptFilter;
use crate::store::KillSwitchFilter;
use crate::store::LedgerFilter;
use crate::types::AttemptStatus;
use crate::types::GateDecision;
use crate::types::KillSwitch;
use crate::types::KillSwitchScope;

impl Manager {
    /// Port of `Manager.SetKillSwitch`.
    pub async fn set_kill_switch(
        &self,
        mut item: KillSwitch,
    ) -> Result<KillSwitch, LiveValidationError> {
        let Some(tenant_context) = tenantctx::from_context() else {
            return Err(LiveValidationError::KillSwitchPermissionDenied);
        };
        if !can_resolve_live_validation_reconciliation(&tenant_context) {
            return Err(LiveValidationError::KillSwitchPermissionDenied);
        }
        let now = self.now();
        if item.scope.is_empty() {
            item.scope = KillSwitchScope::from(KillSwitchScope::TENANT);
        }
        if item.scope == KillSwitchScope::TENANT && item.tenant_id.is_empty() {
            item.tenant_id = tenant_context.tenant_id.clone();
        }
        // A tenant's switch is that tenant's to set. Naming another tenant
        // used to be accepted, so a member allowed to reconcile in one tenant
        // could switch another tenant's validations off (an enabled switch
        // aborts its pending ones).
        if item.scope == KillSwitchScope::TENANT
            && item.tenant_id.trim() != tenant_context.tenant_id.trim()
        {
            return Err(LiveValidationError::KillSwitchPermissionDenied);
        }
        if item.kill_switch_id.is_empty() {
            item.kill_switch_id = format!(
                "live_validation_kill_switch_{}_{}",
                item.scope,
                first_non_empty([item.tenant_id.as_str(), "global"])
            );
        }
        if item.changed_by.is_empty() {
            item.changed_by = tenant_context.principal_id.clone();
        }
        if item.changed_at == DateTime::<Utc>::default() {
            item.changed_at = now;
        }
        if let Some(store) = self.store() {
            store.upsert_kill_switch(item.clone()).await?;
        }
        if item.enabled {
            let _ = self.abort_pending_for_kill_switch(&item).await;
        }
        Ok(item)
    }

    /// Port of `Manager.ListKillSwitches`.
    pub async fn list_kill_switches(
        &self,
        filter: KillSwitchFilter,
    ) -> Result<Vec<KillSwitch>, LiveValidationError> {
        let Some(store) = self.store() else {
            return Ok(Vec::new());
        };
        store.list_kill_switches(filter).await
    }

    /// Port of `Manager.AbortPendingForKillSwitch`.
    async fn abort_pending_for_kill_switch(
        &self,
        item: &KillSwitch,
    ) -> Result<(), LiveValidationError> {
        let Some(store) = self.store() else {
            return Ok(());
        };
        let attempts = store
            .list_attempts(AttemptFilter {
                tenant_id: item.tenant_id.clone(),
                ..AttemptFilter::default()
            })
            .await?;
        let now = self.now();
        for mut attempt in attempts {
            if item.scope == KillSwitchScope::TENANT && attempt.tenant_id != item.tenant_id {
                continue;
            }
            let pending = attempt.status == AttemptStatus::RUNNING
                || attempt.status == AttemptStatus::AWAITING_APPROVAL
                || attempt.status == AttemptStatus::QUEUED;
            if !pending {
                continue;
            }
            attempt.status = AttemptStatus::from(AttemptStatus::ABORTED);
            attempt.completed_at = Some(now);
            attempt.updated_at = now;
            store.upsert_attempt(attempt.clone()).await?;
            let entries = store
                .list_ledger_entries(LedgerFilter {
                    tenant_id: attempt.tenant_id.clone(),
                    validation_id: attempt.validation_id.clone(),
                    ..LedgerFilter::default()
                })
                .await?;
            for entry in entries {
                if is_terminal_ledger_outcome(&entry.outcome) {
                    continue;
                }
                store
                    .update_ledger_entry_outcome(
                        &entry.ledger_entry_id,
                        &LedgerOutcome::from(LedgerOutcome::ABORTED),
                        "live_validation.kill_switch_aborted",
                    )
                    .await?;
            }
        }
        Ok(())
    }

    /// Port of `Manager.evaluateKillSwitch`.
    pub(crate) async fn evaluate_kill_switch(
        &self,
        tenant_id: &str,
        now: DateTime<Utc>,
    ) -> Result<(GateDecision, Denial), LiveValidationError> {
        let Some(store) = self.store() else {
            return Ok((
                GateDecision {
                    allowed: true,
                    checked_at: now,
                    ..GateDecision::default()
                },
                Denial::default(),
            ));
        };
        let switches = store
            .list_kill_switches(KillSwitchFilter {
                enabled: Some(true),
                ..KillSwitchFilter::default()
            })
            .await?;
        for item in switches {
            if let Some(expires_at) = item.expires_at {
                if expires_at < now {
                    continue;
                }
            }
            let global = item.scope == KillSwitchScope::GLOBAL;
            let tenant_match = item.scope == KillSwitchScope::TENANT && item.tenant_id == tenant_id;
            if global || tenant_match {
                let reason =
                    first_non_empty([item.reason.as_str(), "live validation kill switch enabled"]);
                return Ok((
                    GateDecision {
                        allowed: false,
                        reason_code: "live_validation.kill_switch_enabled".to_string(),
                        reference: item.kill_switch_id.clone(),
                        checked_at: now,
                    },
                    Denial {
                        gate: "kill_switch".to_string(),
                        reason_code: "live_validation.kill_switch_enabled".to_string(),
                        message: reason,
                        reference: item.kill_switch_id,
                    },
                ));
            }
        }
        Ok((
            GateDecision {
                allowed: true,
                checked_at: now,
                ..GateDecision::default()
            },
            Denial::default(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use kura_identity::tenantctx;

    use crate::error::StartFailure;
    use crate::ledger::LedgerOutcome;
    use crate::manager::StartInput;
    use crate::matrix::ToolClass;
    use crate::testutil::MemStore;
    use crate::testutil::admin_context;
    use crate::testutil::fixed_clock;
    use crate::testutil::manager_with_store;
    use crate::testutil::operator_context;
    use crate::types::ApprovalMode;
    use crate::types::Attempt;
    use crate::types::AttemptStatus;
    use crate::types::KillSwitch;
    use crate::types::KillSwitchScope;
    use crate::types::SideEffectLedgerEntry;
    use crate::types::SideEffectScope;

    #[tokio::test]
    async fn set_kill_switch_aborts_pending_future_side_effects() {
        let store = Arc::new(MemStore::default());
        {
            let mut state = store.state.lock();
            state.attempts.push(Attempt {
                validation_id: "lv_1".to_string(),
                tenant_id: "ten_1".to_string(),
                status: AttemptStatus::from(AttemptStatus::RUNNING),
                ..Attempt::default()
            });
            state.ledger.push(SideEffectLedgerEntry {
                ledger_entry_id: "ledger_pending".to_string(),
                validation_id: "lv_1".to_string(),
                tenant_id: "ten_1".to_string(),
                outcome: LedgerOutcome::from(LedgerOutcome::ATTEMPTED),
                ..SideEffectLedgerEntry::default()
            });
            state.ledger.push(SideEffectLedgerEntry {
                ledger_entry_id: "ledger_done".to_string(),
                validation_id: "lv_1".to_string(),
                tenant_id: "ten_1".to_string(),
                outcome: LedgerOutcome::from(LedgerOutcome::COMPLETED),
                ..SideEffectLedgerEntry::default()
            });
        }
        let manager = manager_with_store(store.clone());

        tenantctx::scope(admin_context(), async {
            manager
                .set_kill_switch(KillSwitch {
                    scope: KillSwitchScope::from(KillSwitchScope::TENANT),
                    enabled: true,
                    reason: "containment".to_string(),
                    ..KillSwitch::default()
                })
                .await
        })
        .await
        .expect("set kill switch");

        let state = store.state.lock();
        assert_eq!(
            state.attempts[0].status,
            AttemptStatus::from(AttemptStatus::ABORTED)
        );
        assert_eq!(
            state.ledger[0].outcome,
            LedgerOutcome::from(LedgerOutcome::ABORTED)
        );
        assert_eq!(
            state.ledger[1].outcome,
            LedgerOutcome::from(LedgerOutcome::COMPLETED)
        );
    }

    #[tokio::test]
    async fn start_denies_enabled_tenant_kill_switch_before_approval() {
        let now = fixed_clock();
        let store = Arc::new(MemStore::default());
        store.state.lock().kill_switches.push(KillSwitch {
            kill_switch_id: "kill_1".to_string(),
            scope: KillSwitchScope::from(KillSwitchScope::TENANT),
            tenant_id: "ten_1".to_string(),
            enabled: true,
            reason: "contain live validation".to_string(),
            changed_by: "prn_owner".to_string(),
            changed_at: now,
            ..KillSwitch::default()
        });
        let manager = manager_with_store(store);

        let result = tenantctx::scope(operator_context(), async {
            manager
                .start(StartInput {
                    validation_id: "lv_kill".to_string(),
                    candidate_id: "candidate_1".to_string(),
                    candidate_tool_classes: vec![ToolClass::from(
                        ToolClass::DAEMON_INSPECTION_READ,
                    )],
                    requested_scope: SideEffectScope {
                        scope_id: "scope_1".to_string(),
                        included_tool_classes: vec![ToolClass::from(
                            ToolClass::DAEMON_INSPECTION_READ,
                        )],
                        approval_mode: ApprovalMode::from(ApprovalMode::SCOPE_LEVEL),
                        declared_by: "prn_operator".to_string(),
                        declared_at: now,
                        ..SideEffectScope::default()
                    },
                    ..StartInput::default()
                })
                .await
        })
        .await;

        let Err(StartFailure::Blocked(start_result)) = result else {
            panic!("expected blocked by kill switch, got success or other failure");
        };
        assert_eq!(start_result.denials[0].gate, "kill_switch");
        assert_eq!(start_result.attempt.approval_summary.required, 0);
    }
}
