//! Port of `daemon/internal/delivery/manager.go`: the delivery manager. The manager owns the
//! delivery targets/preferences/outcomes ledger in the SQLite store, resolves the active
//! preference and target for each emitted outcome, and fans out to the registered adapters.
//!
//! Go notes carried into the port:
//!
//! - The Go zero-value nil-manager / nil-store guards (`delivery manager is not configured`)
//!   cannot occur: the Rust `Manager` requires a store and a bus at construction.
//! - Go validates `targetKind == ""` and `scopeKind == ""`; the Rust enums have no empty
//!   variant (defaults: `TargetKind::ConnectorRoute`, `PreferenceScopeKind::UserDefault`), so
//!   those checks are dropped.
//! - Go zero `time.Time` is represented by the chrono UNIX epoch (the Rust `DateTime`
//!   default); `is_unset_time` mirrors the store crate convention.
//! - The store is shared as `Arc<Mutex<SQLiteStore>>` so the detached retry/window threads
//!   (Go goroutines) can touch the ledger; rusqlite `Connection` is Send but not Sync.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Utc};
use kura_events::{Bus, Event, Resource, Scope};
use kura_store::SQLiteStore;
use kura_store::delivery::{
    DeliveryAttemptRecord, DeliveryOutcomeFilter, DeliveryOutcomeRecord, DeliveryPreferenceRecord,
    DeliverySummaryWindowRecord, DeliveryTargetRecord,
};
use parking_lot::Mutex;
use serde_json::{Map, Value, json};

use crate::adapters::{ChannelDeliveryHooks, DeliveryAdapter};
use crate::{
    DeliveryAttempt, DeliveryMode, DeliveryOutcome, DeliveryPreference, DeliveryTarget,
    OutcomeFilter, OutcomeInput, OutcomeStatus, PreferenceScopeKind, ResultClass, SummaryWindow,
    TargetKind, TargetStatus,
};

#[derive(Debug, thiserror::Error, Clone, PartialEq, Eq)]
pub enum DeliveryError {
    #[error("delivery manager is not configured")]
    NotConfigured,
    #[error("targetId is required")]
    TargetIdRequired,
    #[error("displayName is required")]
    DisplayNameRequired,
    #[error("preferenceId is required")]
    PreferenceIdRequired,
    #[error("{0}")]
    Store(String),
    #[error("{0}")]
    Other(String),
}

/// Retry/digest configuration (Go `maxAttempts`/`baseRetryDelay`/`maxRetryDelay`).
#[derive(Debug, Clone, Copy)]
pub(crate) struct DeliveryConfig {
    pub(crate) max_attempts: i64,
    pub(crate) base_retry_delay: Duration,
    pub(crate) max_retry_delay: Duration,
}

impl Default for DeliveryConfig {
    fn default() -> Self {
        DeliveryConfig {
            max_attempts: 3,
            base_retry_delay: Duration::from_secs(5),
            max_retry_delay: Duration::from_secs(60),
        }
    }
}

#[derive(Default)]
pub(crate) struct DeliverySchedules {
    pub(crate) retry_scheduled: HashMap<String, ()>,
    pub(crate) window_scheduled: HashMap<String, ()>,
}

/// Shared manager state. Methods take `self: &Arc<Self>` so detached retry/window threads
/// (which only hold the `Arc`) can drive the same logic.
pub(crate) struct ManagerInner {
    pub(crate) environment_scope: String,
    pub(crate) event_bus: Bus,
    pub(crate) store: Arc<Mutex<SQLiteStore>>,
    pub(crate) adapters: Mutex<Vec<Arc<dyn DeliveryAdapter>>>,
    pub(crate) hooks: Option<Arc<dyn ChannelDeliveryHooks>>,
    pub(crate) schedules: Mutex<DeliverySchedules>,
    pub(crate) config: Mutex<DeliveryConfig>,
}

/// The delivery manager (port of `Manager`). Cloneable handle over shared inner state; all
/// operations are synchronous.
#[derive(Clone)]
pub struct Manager {
    pub(crate) inner: Arc<ManagerInner>,
}

impl Manager {
    /// Creates a manager with no channel/thread hooks (the no-op defaults apply).
    #[must_use]
    pub fn new(
        environment_scope: &str,
        event_bus: Bus,
        store: Arc<Mutex<SQLiteStore>>,
        adapters: Vec<Arc<dyn DeliveryAdapter>>,
    ) -> Self {
        Self::with_hooks(environment_scope, event_bus, store, None, adapters)
    }

    /// Creates a manager with optional channel/thread store hooks (see [`ChannelDeliveryHooks`]).
    #[must_use]
    pub fn with_hooks(
        environment_scope: &str,
        event_bus: Bus,
        store: Arc<Mutex<SQLiteStore>>,
        hooks: Option<Arc<dyn ChannelDeliveryHooks>>,
        adapters: Vec<Arc<dyn DeliveryAdapter>>,
    ) -> Self {
        Manager {
            inner: Arc::new(ManagerInner {
                environment_scope: environment_scope.to_string(),
                event_bus,
                store,
                adapters: Mutex::new(adapters),
                hooks,
                schedules: Mutex::new(DeliverySchedules::default()),
                config: Mutex::new(DeliveryConfig::default()),
            }),
        }
    }

    /// Port of `ConfigureForTesting`: applies non-zero knobs only, mirroring Go.
    pub fn configure_for_testing(
        &self,
        max_attempts: i64,
        base_retry_delay: Duration,
        max_retry_delay: Duration,
    ) {
        let mut config = self.inner.config.lock();
        if max_attempts > 0 {
            config.max_attempts = max_attempts;
        }
        if base_retry_delay > Duration::ZERO {
            config.base_retry_delay = base_retry_delay;
        }
        if max_retry_delay > Duration::ZERO {
            config.max_retry_delay = max_retry_delay;
        }
    }

    /// Port of `Store()`: the shared SQLite store handle.
    #[must_use]
    pub fn store(&self) -> Arc<Mutex<SQLiteStore>> {
        Arc::clone(&self.inner.store)
    }

    /// Registers an adapter (port of appending to `adapters`). Safe to call while retry
    /// threads are active.
    pub fn register_adapter(&self, adapter: Arc<dyn DeliveryAdapter>) {
        self.inner.adapters.lock().push(adapter);
    }

    // ---- targets ----

    /// Port of `CreateTarget`.
    pub fn create_target(&self, target: DeliveryTarget) -> Result<DeliveryTarget, DeliveryError> {
        self.inner.create_target(target)
    }

    /// Port of `ListTargets`.
    pub fn list_targets(&self) -> Result<Vec<DeliveryTarget>, DeliveryError> {
        self.inner.list_targets()
    }

    /// Port of `GetTarget`: returns `(target, found)`.
    pub fn get_target(&self, target_id: &str) -> Result<(DeliveryTarget, bool), DeliveryError> {
        self.inner.get_target(target_id)
    }

    /// Port of `UpdateTargetStatus`.
    pub fn update_target_status(
        &self,
        target_id: &str,
        status: crate::TargetStatus,
    ) -> Result<(DeliveryTarget, bool), DeliveryError> {
        self.inner.update_target_status(target_id, status)
    }

    // ---- preferences ----

    /// Port of `UpsertPreference`.
    pub fn upsert_preference(
        &self,
        pref: DeliveryPreference,
    ) -> Result<DeliveryPreference, DeliveryError> {
        self.inner.upsert_preference(pref)
    }

    /// Port of `ListPreferences`.
    pub fn list_preferences(&self) -> Result<Vec<DeliveryPreference>, DeliveryError> {
        self.inner.list_preferences()
    }

    /// Port of `GetPreference`.
    pub fn get_preference(
        &self,
        preference_id: &str,
    ) -> Result<(DeliveryPreference, bool), DeliveryError> {
        self.inner.get_preference(preference_id)
    }

    // ---- outcomes ----

    /// Port of `EmitOutcome`.
    pub fn emit_outcome(&self, input: OutcomeInput) -> Result<DeliveryOutcome, DeliveryError> {
        self.inner.emit_outcome(input)
    }

    /// Port of `ListOutcomes`.
    pub fn list_outcomes(
        &self,
        filter: OutcomeFilter,
    ) -> Result<Vec<DeliveryOutcome>, DeliveryError> {
        self.inner.list_outcomes(&filter)
    }

    /// Port of `GetOutcome`.
    pub fn get_outcome(&self, delivery_id: &str) -> Result<(DeliveryOutcome, bool), DeliveryError> {
        self.inner.get_outcome(delivery_id)
    }

    // ---- summary windows ----

    /// Port of `ListSummaryWindows`.
    pub fn list_summary_windows(&self) -> Result<Vec<SummaryWindow>, DeliveryError> {
        self.inner.list_summary_windows()
    }

    /// Port of `GetSummaryWindow`.
    pub fn get_summary_window(
        &self,
        summary_window_id: &str,
    ) -> Result<(SummaryWindow, bool), DeliveryError> {
        self.inner.get_summary_window(summary_window_id)
    }
}

impl ManagerInner {
    // ---- targets ----

    pub(crate) fn create_target(
        self: &Arc<Self>,
        mut target: DeliveryTarget,
    ) -> Result<DeliveryTarget, DeliveryError> {
        let now = Utc::now();
        if target.target_id.trim().is_empty() {
            return Err(DeliveryError::TargetIdRequired);
        }
        if target.display_name.trim().is_empty() {
            return Err(DeliveryError::DisplayNameRequired);
        }
        if target.environment_scope.is_empty() {
            target.environment_scope = self.environment_scope.clone();
        }
        if is_unset_time(&target.created_at) {
            target.created_at = now;
        }
        target.updated_at = now;
        target.supports_immediate = true;
        if target.target_kind == TargetKind::TestSink {
            target.supports_digest = true;
        }
        self.store
            .lock()
            .upsert_delivery_target(&DeliveryTargetRecord {
                target_id: target.target_id.clone(),
                environment_scope: target.environment_scope.clone(),
                target_kind: target.target_kind.as_str().to_string(),
                status: target.status.as_str().to_string(),
                updated_at: target.updated_at,
                document: must_marshal(&target),
            })
            .map_err(DeliveryError::Store)?;
        self.publish_event(
            "delivery.target_registered",
            Resource {
                kind: "delivery_target".to_string(),
                id: target.target_id.clone(),
            },
            payload_map(json!({
                "targetId": target.target_id,
                "targetKind": target.target_kind,
                "environmentScope": target.environment_scope,
                "status": target.status,
            })),
        )?;
        Ok(target)
    }

    pub(crate) fn list_targets(&self) -> Result<Vec<DeliveryTarget>, DeliveryError> {
        let records = self
            .store
            .lock()
            .list_delivery_targets(&self.environment_scope)
            .map_err(DeliveryError::Store)?;
        let mut items = Vec::with_capacity(records.len());
        for record in &records {
            items.push(unmarshal_document(&record.document)?);
        }
        Ok(items)
    }

    pub(crate) fn get_target(
        &self,
        target_id: &str,
    ) -> Result<(DeliveryTarget, bool), DeliveryError> {
        let record = self
            .store
            .lock()
            .get_delivery_target(&self.environment_scope, target_id)
            .map_err(DeliveryError::Store)?;
        match record {
            Some(record) => Ok((unmarshal_document(&record.document)?, true)),
            None => Ok((DeliveryTarget::default(), false)),
        }
    }

    pub(crate) fn update_target_status(
        &self,
        target_id: &str,
        status: TargetStatus,
    ) -> Result<(DeliveryTarget, bool), DeliveryError> {
        let (mut target, ok) = self.get_target(target_id)?;
        if !ok {
            return Ok((target, false));
        }
        target.status = status;
        let now = Utc::now();
        target.updated_at = now;
        self.store
            .lock()
            .upsert_delivery_target(&DeliveryTargetRecord {
                target_id: target.target_id.clone(),
                environment_scope: target.environment_scope.clone(),
                target_kind: target.target_kind.as_str().to_string(),
                status: target.status.as_str().to_string(),
                updated_at: target.updated_at,
                document: must_marshal(&target),
            })
            .map_err(DeliveryError::Store)?;
        self.publish_event(
            "delivery.target_status_changed",
            Resource {
                kind: "delivery_target".to_string(),
                id: target.target_id.clone(),
            },
            payload_map(json!({
                "targetId": target.target_id,
                "targetKind": target.target_kind,
                "environmentScope": target.environment_scope,
                "status": target.status,
            })),
        )?;
        Ok((target, true))
    }

    // ---- preferences ----

    pub(crate) fn upsert_preference(
        &self,
        mut pref: DeliveryPreference,
    ) -> Result<DeliveryPreference, DeliveryError> {
        let now = Utc::now();
        if pref.preference_id.trim().is_empty() {
            return Err(DeliveryError::PreferenceIdRequired);
        }
        if pref.environment_scope.is_empty() {
            pref.environment_scope = self.environment_scope.clone();
        }
        if is_unset_time(&pref.created_at) {
            pref.created_at = now;
        }
        pref.active = true;
        pref.updated_at = now;
        self.store
            .lock()
            .upsert_delivery_preference(&DeliveryPreferenceRecord {
                preference_id: pref.preference_id.clone(),
                environment_scope: pref.environment_scope.clone(),
                scope_kind: pref.scope_kind.as_str().to_string(),
                integration_id: pref.integration_id.clone(),
                active: pref.active,
                updated_at: pref.updated_at,
                document: must_marshal(&pref),
            })
            .map_err(DeliveryError::Store)?;
        self.publish_event(
            "delivery.preference_updated",
            Resource {
                kind: "delivery_preference".to_string(),
                id: pref.preference_id.clone(),
            },
            payload_map(json!({
                "preferenceId": pref.preference_id,
                "environmentScope": pref.environment_scope,
                "scopeKind": pref.scope_kind,
                "integrationId": pref.integration_id,
            })),
        )?;
        Ok(pref)
    }

    pub(crate) fn list_preferences(&self) -> Result<Vec<DeliveryPreference>, DeliveryError> {
        let records = self
            .store
            .lock()
            .list_delivery_preferences(&self.environment_scope)
            .map_err(DeliveryError::Store)?;
        let mut items = Vec::with_capacity(records.len());
        for record in &records {
            items.push(unmarshal_document(&record.document)?);
        }
        Ok(items)
    }

    pub(crate) fn get_preference(
        &self,
        preference_id: &str,
    ) -> Result<(DeliveryPreference, bool), DeliveryError> {
        let record = self
            .store
            .lock()
            .get_delivery_preference(&self.environment_scope, preference_id)
            .map_err(DeliveryError::Store)?;
        match record {
            Some(record) => Ok((unmarshal_document(&record.document)?, true)),
            None => Ok((DeliveryPreference::default(), false)),
        }
    }

    // ---- outcomes ----

    pub(crate) fn emit_outcome(
        self: &Arc<Self>,
        input: OutcomeInput,
    ) -> Result<DeliveryOutcome, DeliveryError> {
        let existing = self.list_outcomes(&OutcomeFilter {
            source_kind: input.source_kind.clone(),
            source_id: input.source_id.clone(),
            ..OutcomeFilter::default()
        })?;
        if let Some(first) = existing.first() {
            return Ok(first.clone());
        }
        let (pref, target, mode, suppression_reason) =
            self.resolve_preference(&input.integration_id, input.result_class)?;
        let now = Utc::now();
        let mut outcome = DeliveryOutcome {
            delivery_id: new_delivery_id(),
            environment_scope: self.environment_scope.clone(),
            source_kind: input.source_kind,
            source_id: input.source_id,
            run_id: input.run_id,
            workflow_id: input.workflow_id,
            schedule_id: input.schedule_id,
            schedule_attempt_id: input.schedule_attempt_id,
            integration_id: input.integration_id,
            result_class: input.result_class,
            mode,
            status: OutcomeStatus::Pending,
            chosen_target_id: target.target_id.clone(),
            preference_id: pref.preference_id.clone(),
            payload_preview: input.payload_preview,
            suppression_reason,
            created_at: now,
            updated_at: now,
            ..DeliveryOutcome::default()
        };
        self.store_outcome(&outcome)?;
        self.publish_outcome_created(&outcome)?;
        match outcome.mode {
            DeliveryMode::Suppressed => {
                outcome.status = OutcomeStatus::Suppressed;
                outcome.finalized_at = Some(now);
                outcome.updated_at = now;
                self.store_outcome(&outcome)?;
                self.publish_outcome_status_changed(&outcome)?;
                self.attach_attempts(outcome)
            }
            DeliveryMode::Digest => self.queue_digest_outcome(outcome, &pref, &target),
            _ => self.dispatch_immediate(outcome, &target),
        }
    }

    pub(crate) fn list_outcomes(
        &self,
        filter: &OutcomeFilter,
    ) -> Result<Vec<DeliveryOutcome>, DeliveryError> {
        let records = self
            .store
            .lock()
            .list_delivery_outcomes(
                &self.environment_scope,
                &DeliveryOutcomeFilter {
                    source_kind: filter.source_kind.clone(),
                    source_id: filter.source_id.clone(),
                    run_id: filter.run_id.clone(),
                    workflow_id: filter.workflow_id.clone(),
                    schedule_id: filter.schedule_id.clone(),
                    integration_id: filter.integration_id.clone(),
                    status: filter
                        .status
                        .map(|s| s.as_str().to_string())
                        .unwrap_or_default(),
                    target_id: filter.target_id.clone(),
                },
            )
            .map_err(DeliveryError::Store)?;
        let mut items = Vec::with_capacity(records.len());
        for record in &records {
            let outcome: DeliveryOutcome = unmarshal_document(&record.document)?;
            items.push(self.attach_attempts(outcome)?);
        }
        Ok(items)
    }

    pub(crate) fn get_outcome(
        &self,
        delivery_id: &str,
    ) -> Result<(DeliveryOutcome, bool), DeliveryError> {
        let record = self
            .store
            .lock()
            .get_delivery_outcome(&self.environment_scope, delivery_id)
            .map_err(DeliveryError::Store)?;
        match record {
            Some(record) => {
                let outcome: DeliveryOutcome = unmarshal_document(&record.document)?;
                Ok((self.attach_attempts(outcome)?, true))
            }
            None => Ok((DeliveryOutcome::default(), false)),
        }
    }

    // ---- summary windows ----

    pub(crate) fn list_summary_windows(&self) -> Result<Vec<SummaryWindow>, DeliveryError> {
        let records = self
            .store
            .lock()
            .list_delivery_summary_windows(&self.environment_scope)
            .map_err(DeliveryError::Store)?;
        let mut items = Vec::with_capacity(records.len());
        for record in &records {
            items.push(unmarshal_document(&record.document)?);
        }
        Ok(items)
    }

    pub(crate) fn get_summary_window(
        &self,
        summary_window_id: &str,
    ) -> Result<(SummaryWindow, bool), DeliveryError> {
        let record = self
            .store
            .lock()
            .get_delivery_summary_window(&self.environment_scope, summary_window_id)
            .map_err(DeliveryError::Store)?;
        match record {
            Some(record) => Ok((unmarshal_document(&record.document)?, true)),
            None => Ok((SummaryWindow::default(), false)),
        }
    }

    // ---- preference resolution ----

    pub(crate) fn resolve_preference(
        &self,
        integration_id: &str,
        result_class: ResultClass,
    ) -> Result<(DeliveryPreference, DeliveryTarget, DeliveryMode, String), DeliveryError> {
        let prefs = self.list_preferences()?;
        let mut selected = DeliveryPreference::default();
        for pref in &prefs {
            if pref.active
                && !integration_id.is_empty()
                && pref.scope_kind == PreferenceScopeKind::IntegrationOverride
                && pref.integration_id == integration_id
            {
                selected = pref.clone();
                break;
            }
        }
        if selected.preference_id.is_empty() {
            for pref in &prefs {
                if pref.active && pref.scope_kind == PreferenceScopeKind::UserDefault {
                    selected = pref.clone();
                    break;
                }
            }
        }
        if selected.preference_id.is_empty() {
            return Ok((
                DeliveryPreference::default(),
                DeliveryTarget::default(),
                DeliveryMode::Suppressed,
                "no active delivery preference".to_string(),
            ));
        }
        if let Some(reason) = suppression_reason(&selected, result_class) {
            return Ok((
                selected,
                DeliveryTarget::default(),
                DeliveryMode::Suppressed,
                reason,
            ));
        }
        let target_id = selected
            .preferred_targets_by_class
            .get(&result_class)
            .map(|s| s.trim().to_string())
            .unwrap_or_default();
        if target_id.is_empty() {
            return Ok((
                selected,
                DeliveryTarget::default(),
                DeliveryMode::Suppressed,
                "no target configured for result class".to_string(),
            ));
        }
        let (target, ok) = self.get_target(&target_id)?;
        if !ok {
            return Ok((
                selected,
                DeliveryTarget::default(),
                DeliveryMode::Suppressed,
                "configured target is missing".to_string(),
            ));
        }
        if target.status != TargetStatus::Active {
            return Ok((selected, target, DeliveryMode::Immediate, String::new()));
        }
        if result_class == ResultClass::RoutineSuccess
            && matches!(
                selected
                    .summary_policy
                    .as_ref()
                    .map(|p| p.routine_success_mode),
                Some(Some(DeliveryMode::Digest))
            )
        {
            return Ok((selected, target, DeliveryMode::Digest, String::new()));
        }
        Ok((selected, target, DeliveryMode::Immediate, String::new()))
    }

    // ---- adapter selection ----

    pub(crate) fn adapter_for(&self, kind: TargetKind) -> Option<Arc<dyn DeliveryAdapter>> {
        let adapters = self.adapters.lock();
        for adapter in adapters.iter() {
            if adapter.supports(kind) {
                return Some(Arc::clone(adapter));
            }
        }
        None
    }

    // ---- channel/thread store hooks ----

    /// Port of `connectorDeliveryDisabled`: only consulted for connector-route targets with a
    /// non-empty connector binding. Without channel hooks (deferred), delivery is enabled.
    pub(crate) fn connector_delivery_disabled(
        &self,
        target: &DeliveryTarget,
    ) -> Result<bool, DeliveryError> {
        if target.target_kind != TargetKind::ConnectorRoute || target.connector_binding.is_none() {
            return Ok(false);
        }
        let connector_id = target
            .connector_binding
            .as_ref()
            .map(|b| b.connector_id.trim().to_string())
            .unwrap_or_default();
        if connector_id.is_empty() {
            return Ok(false);
        }
        match &self.hooks {
            Some(hooks) => hooks
                .connector_delivery_disabled(&connector_id)
                .map_err(DeliveryError::Store),
            None => Ok(false),
        }
    }

    /// Port of `recordChannelBackgroundDeliveryOutcome`: no-op unless channel hooks are
    /// installed (the channel-management store domain is deferred).
    pub(crate) fn record_channel_background_delivery_outcome(
        &self,
        outcome: &DeliveryOutcome,
        target: &DeliveryTarget,
        reason_code: &str,
    ) -> Result<(), DeliveryError> {
        if target.target_kind != TargetKind::ConnectorRoute || target.connector_binding.is_none() {
            return Ok(());
        }
        let connector_id = target
            .connector_binding
            .as_ref()
            .map(|b| b.connector_id.trim().to_string())
            .unwrap_or_default();
        if connector_id.is_empty() {
            return Ok(());
        }
        if let Some(hooks) = &self.hooks {
            hooks
                .record_background_delivery_outcome(outcome, target, reason_code)
                .map_err(DeliveryError::Store)?;
        }
        Ok(())
    }

    /// Port of `recordThreadDeliveryProjection`: skipped when the outcome carries no run id;
    /// otherwise delegated to the channel hooks (deferred no-op).
    pub(crate) fn record_thread_delivery_projection(
        &self,
        outcome: &DeliveryOutcome,
        reason_code: &str,
    ) -> Result<(), DeliveryError> {
        if outcome.delivery_id.is_empty() || outcome.run_id.is_empty() {
            return Ok(());
        }
        if let Some(hooks) = &self.hooks {
            hooks
                .record_thread_delivery_projection(outcome, reason_code)
                .map_err(DeliveryError::Store)?;
        }
        Ok(())
    }

    // ---- persistence helpers ----

    pub(crate) fn attach_attempts(
        &self,
        mut outcome: DeliveryOutcome,
    ) -> Result<DeliveryOutcome, DeliveryError> {
        let records = self
            .store
            .lock()
            .list_delivery_attempts(&outcome.delivery_id)
            .map_err(DeliveryError::Store)?;
        outcome.attempts = Vec::with_capacity(records.len());
        for record in &records {
            outcome.attempts.push(unmarshal_document(&record.document)?);
        }
        Ok(outcome)
    }

    pub(crate) fn store_outcome(&self, outcome: &DeliveryOutcome) -> Result<(), DeliveryError> {
        self.store
            .lock()
            .upsert_delivery_outcome(&DeliveryOutcomeRecord {
                delivery_id: outcome.delivery_id.clone(),
                environment_scope: outcome.environment_scope.clone(),
                source_kind: outcome.source_kind.clone(),
                source_id: outcome.source_id.clone(),
                run_id: outcome.run_id.clone(),
                workflow_id: outcome.workflow_id.clone(),
                schedule_id: outcome.schedule_id.clone(),
                integration_id: outcome.integration_id.clone(),
                status: outcome.status.as_str().to_string(),
                chosen_target_id: outcome.chosen_target_id.clone(),
                preference_id: outcome.preference_id.clone(),
                summary_window_id: outcome.summary_window_id.clone(),
                updated_at: outcome.updated_at,
                document: must_marshal(outcome),
            })
            .map_err(DeliveryError::Store)
    }

    pub(crate) fn store_attempt(&self, attempt: &DeliveryAttempt) -> Result<(), DeliveryError> {
        self.store
            .lock()
            .upsert_delivery_attempt(&DeliveryAttemptRecord {
                attempt_id: attempt.attempt_id.clone(),
                delivery_id: attempt.delivery_id.clone(),
                attempt_number: attempt.attempt_number,
                target_id: attempt.target_id.clone(),
                status: attempt.status.as_str().to_string(),
                next_retry_at: attempt.next_retry_at,
                document: must_marshal(attempt),
            })
            .map_err(DeliveryError::Store)
    }

    pub(crate) fn store_window(&self, window: &SummaryWindow) -> Result<(), DeliveryError> {
        self.store
            .lock()
            .upsert_delivery_summary_window(&DeliverySummaryWindowRecord {
                summary_window_id: window.summary_window_id.clone(),
                environment_scope: window.environment_scope.clone(),
                target_id: window.target_id.clone(),
                preference_id: window.preference_id.clone(),
                status: window.status.as_str().to_string(),
                window_ends_at: window.window_ends_at,
                updated_at: window.updated_at,
                document: must_marshal(window),
            })
            .map_err(DeliveryError::Store)
    }

    // ---- event publishing ----

    pub(crate) fn publish_outcome_created(
        &self,
        outcome: &DeliveryOutcome,
    ) -> Result<(), DeliveryError> {
        self.record_thread_delivery_projection(outcome, "delivery.outcome_created")?;
        self.publish_event(
            "delivery.outcome_created",
            Resource {
                kind: "delivery".to_string(),
                id: outcome.delivery_id.clone(),
            },
            payload_map(json!({
                "deliveryId": outcome.delivery_id,
                "sourceKind": outcome.source_kind,
                "sourceId": outcome.source_id,
                "runId": outcome.run_id,
                "workflowId": outcome.workflow_id,
                "scheduleId": outcome.schedule_id,
                "scheduleAttemptId": outcome.schedule_attempt_id,
                "integrationId": outcome.integration_id,
                "resultClass": outcome.result_class,
                "mode": outcome.mode,
                "status": outcome.status,
                "chosenTargetId": outcome.chosen_target_id,
                "suppressionReason": outcome.suppression_reason,
            })),
        )
    }

    pub(crate) fn publish_attempt_recorded(
        &self,
        outcome: &DeliveryOutcome,
        attempt: &DeliveryAttempt,
    ) -> Result<(), DeliveryError> {
        self.publish_event(
            "delivery.attempt_recorded",
            Resource {
                kind: "delivery".to_string(),
                id: outcome.delivery_id.clone(),
            },
            payload_map(json!({
                "sourceKind": outcome.source_kind,
                "sourceId": outcome.source_id,
                "runId": outcome.run_id,
                "workflowId": outcome.workflow_id,
                "scheduleId": outcome.schedule_id,
                "scheduleAttemptId": outcome.schedule_attempt_id,
                "integrationId": outcome.integration_id,
                "deliveryId": outcome.delivery_id,
                "attemptId": attempt.attempt_id,
                "attemptNumber": attempt.attempt_number,
                "transportKind": attempt.transport_kind,
                "status": attempt.status,
                "failureClass": attempt.failure_class,
                "nextRetryAt": attempt.next_retry_at,
                "connectorMessageDeliveryId": attempt.connector_message_delivery_id,
            })),
        )
    }

    pub(crate) fn publish_outcome_status_changed(
        &self,
        outcome: &DeliveryOutcome,
    ) -> Result<(), DeliveryError> {
        self.record_thread_delivery_projection(outcome, "delivery.outcome_status_changed")?;
        self.publish_event(
            "delivery.outcome_status_changed",
            Resource {
                kind: "delivery".to_string(),
                id: outcome.delivery_id.clone(),
            },
            payload_map(json!({
                "sourceKind": outcome.source_kind,
                "sourceId": outcome.source_id,
                "runId": outcome.run_id,
                "workflowId": outcome.workflow_id,
                "scheduleId": outcome.schedule_id,
                "scheduleAttemptId": outcome.schedule_attempt_id,
                "integrationId": outcome.integration_id,
                "deliveryId": outcome.delivery_id,
                "resultClass": outcome.result_class,
                "mode": outcome.mode,
                "status": outcome.status,
                "chosenTargetId": outcome.chosen_target_id,
                "suppressionReason": outcome.suppression_reason,
            })),
        )
    }

    /// Port of `publishEvent`: builds the delivery event, publishes on the bus, and appends
    /// it to the event ledger. Scope fields are derived from the payload like Go.
    pub(crate) fn publish_event(
        &self,
        name: &str,
        resource: Resource,
        payload: Map<String, Value>,
    ) -> Result<(), DeliveryError> {
        let mut scope = Scope::default();
        if let Some(Value::String(v)) = payload.get("runId") {
            if !v.is_empty() {
                scope.run_id = v.clone();
            }
        }
        if let Some(Value::String(v)) = payload.get("workflowId") {
            if !v.is_empty() {
                scope.workflow_id = v.clone();
            }
        }
        if let Some(Value::String(v)) = payload.get("scheduleId") {
            if !v.is_empty() {
                scope.schedule_id = v.clone();
            }
        }
        if let Some(Value::String(v)) = payload.get("scheduleAttemptId") {
            if !v.is_empty() {
                scope.schedule_attempt_id = v.clone();
            }
        }
        let event = Event {
            environment_scope: self.environment_scope.clone(),
            category: "delivery".to_string(),
            name: name.to_string(),
            occurred_at: Utc::now(),
            scope,
            resource,
            payload,
            ..Event::default()
        };
        let published = self.event_bus.publish(event);
        self.store
            .lock()
            .append_event(&published)
            .map_err(|e| DeliveryError::Store(format!("append delivery event {name}: {e}")))?;
        Ok(())
    }
}

/// Go `mustMarshal`: documents are JSON; a marshal failure is a programming error.
fn must_marshal<T: serde::Serialize>(value: &T) -> String {
    serde_json::to_string(value).expect("delivery document must marshal")
}

/// Go `unmarshalDocument`: parse a stored document back into the domain type.
fn unmarshal_document<T: serde::de::DeserializeOwned>(document: &str) -> Result<T, DeliveryError> {
    serde_json::from_str(document)
        .map_err(|e| DeliveryError::Store(format!("unmarshal delivery document: {e}")))
}

pub(crate) fn payload_map(value: Value) -> Map<String, Value> {
    value.as_object().cloned().unwrap_or_default()
}

/// Go zero `time.Time` is the chrono UNIX epoch in this port (repo convention).
fn is_unset_time(dt: &DateTime<Utc>) -> bool {
    dt.timestamp() == 0 && dt.timestamp_subsec_nanos() == 0
}

/// Go `nonEmpty`: first argument with non-blank content, else the fallback.
pub(crate) fn non_empty(a: &str, fallback: &str) -> String {
    if a.trim().is_empty() {
        fallback.to_string()
    } else {
        a.to_string()
    }
}

/// Go `suppressionReason` receiver on DeliveryPreference.
fn suppression_reason(pref: &DeliveryPreference, result_class: ResultClass) -> Option<String> {
    let policy = pref.suppression_policy.clone().unwrap_or_default();
    match result_class {
        ResultClass::RoutineSuccess => {
            if policy.suppress_routine_success {
                Some("routine success suppressed by policy".to_string())
            } else {
                None
            }
        }
        ResultClass::Urgent => {
            if policy.suppress_urgent {
                Some("urgent result suppressed by policy".to_string())
            } else {
                None
            }
        }
        ResultClass::Failure => {
            if policy.suppress_failure {
                Some("failure result suppressed by policy".to_string())
            } else {
                None
            }
        }
    }
}

pub(crate) fn new_delivery_id() -> String {
    format!("delivery_{}", random_suffix())
}

pub(crate) fn new_attempt_id() -> String {
    format!("delivery_attempt_{}", random_suffix())
}

pub(crate) fn new_summary_window_id() -> String {
    format!("summary_window_{}", random_suffix())
}

/// Go `randomSuffix` (8 random bytes hex-encoded); uuid v4 supplies the entropy.
fn random_suffix() -> String {
    let hex = uuid::Uuid::new_v4().simple().to_string();
    hex[..16].to_string()
}
