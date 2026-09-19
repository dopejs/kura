//! Connector supervisor (port of daemon/internal/connectors/supervisor.go):
//! the in-memory connector registry with health/failure/restart lifecycle,
//! tenant scoping, per-connector mutation serialization, and live-validation
//! outcome mapping.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use chrono::Utc;
use kura_livevalidation::{FakeOutcome, FakeOutcomeResult, SafetyClass, fake_outcome_result_for};
use parking_lot::{Mutex, RwLock};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::{Connector, Status};

/// Validation/lookup failures shared by the supervisor and the conformance
/// helpers. Display strings match the Go sentinel errors exactly.
#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum ConnectorsError {
    #[error("connector id is required")]
    ConnectorIdRequired,
    #[error("connector kind is required")]
    ConnectorKindRequired,
    #[error("connector not found")]
    ConnectorNotFound,
    #[error("connector is disabled")]
    ConnectorDisabled,
    #[error("invalid connector health status")]
    InvalidConnectorHealth,
    #[error("connector failure reason is required")]
    ConnectorFailureRequired,
    #[error("conformance scenario id is required")]
    ConformanceScenarioRequired,
    #[error("connector kind is required")]
    ConformanceKindRequired,
    #[error("core invariant failed")]
    CoreInvariantFailed,
    #[error("equivalent durable identity rule is required")]
    EquivalentIdentityRequired,
    #[error("connector id is required")]
    DiagnosticConnectorRequired,
    #[error("diagnostic reason code is required")]
    DiagnosticReasonRequired,
}

/// Input to [`Supervisor::register`] (Go `RegisterInput`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RegisterInput {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub tenant_id: String,
    pub connector_id: String,
    pub kind: String,
    pub display_name: String,
    /// Go `secretRefs,omitempty`: `None` corresponds to a nil slice and keeps
    /// the existing refs on re-registration; `Some(...)` replaces them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub secret_refs: Option<Vec<String>>,
}

/// Input to [`Supervisor::report_health`] (Go `ReportHealthInput`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReportHealthInput {
    pub status: Status,
}

/// Input to [`Supervisor::report_failure`] (Go `ReportFailureInput`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReportFailureInput {
    pub reason: String,
}

/// Failure count at which a connector circuit-breaks to `Failed`.
const FAILURE_THRESHOLD: i64 = 5;

/// Maximum exponential backoff between restarts, in seconds (Go `minInt(..., 300)`).
const MAX_BACKOFF_SECONDS: i64 = 300;

/// Registry state guarded by the single supervisor read/write lock.
#[derive(Default)]
struct Inner {
    by_id: HashMap<String, Connector>,
    order: Vec<String>,
    mutation_locks: HashMap<String, Arc<Mutex<()>>>,
}

/// Thread-safe connector registry (Go `Supervisor`).
pub struct Supervisor {
    inner: RwLock<Inner>,
}

impl Default for Supervisor {
    fn default() -> Self {
        Supervisor::new()
    }
}

impl Supervisor {
    /// Go `NewSupervisor`.
    #[must_use]
    pub fn new() -> Self {
        Supervisor {
            inner: RwLock::new(Inner::default()),
        }
    }

    /// Go `RunLiveValidationOutcome`: maps a fake downstream outcome to its
    /// ledger/retry semantics under the non-idempotent-mutation safety class.
    #[must_use]
    pub fn run_live_validation_outcome(&self, outcome: FakeOutcome) -> FakeOutcomeResult {
        fake_outcome_result_for(
            &outcome,
            &SafetyClass::from(SafetyClass::NON_IDEMPOTENT_MUTATION),
        )
    }

    /// Go `Register`: registers a new connector or updates an existing one.
    /// The returned `bool` reports whether the connector was newly created.
    pub fn register(&self, input: RegisterInput) -> Result<(Connector, bool), ConnectorsError> {
        if input.connector_id.is_empty() {
            return Err(ConnectorsError::ConnectorIdRequired);
        }
        if input.kind.is_empty() {
            return Err(ConnectorsError::ConnectorKindRequired);
        }

        let mut inner = self.inner.write();
        let now = Utc::now();
        if let Some(existing) = inner.by_id.get_mut(&input.connector_id) {
            existing.kind = input.kind;
            existing.display_name = input.display_name;
            if !input.tenant_id.is_empty() {
                existing.tenant_id = input.tenant_id;
            }
            if let Some(secret_refs) = input.secret_refs {
                existing.secret_refs = clean_strings(secret_refs);
            }
            existing.updated_at = now;
            return Ok((existing.clone(), false));
        }

        let connector = Connector {
            tenant_id: input.tenant_id,
            connector_id: input.connector_id.clone(),
            kind: input.kind,
            display_name: input.display_name,
            secret_refs: clean_strings(input.secret_refs.unwrap_or_default()),
            status: Status::Registered,
            created_at: now,
            updated_at: now,
            ..Connector::default()
        };
        inner
            .by_id
            .insert(connector.connector_id.clone(), connector.clone());
        inner.order.push(connector.connector_id.clone());
        Ok((connector, true))
    }

    /// Go `List`: connectors in registration order.
    #[must_use]
    pub fn list(&self) -> Vec<Connector> {
        let inner = self.inner.read();
        inner
            .order
            .iter()
            .filter_map(|id| inner.by_id.get(id))
            .cloned()
            .collect()
    }

    /// Go `ListForTenant`: connectors in registration order, filtered by tenant
    /// (an empty tenant id matches every connector).
    #[must_use]
    pub fn list_for_tenant(&self, tenant_id: &str) -> Vec<Connector> {
        let inner = self.inner.read();
        inner
            .order
            .iter()
            .filter_map(|id| inner.by_id.get(id))
            .filter(|connector| tenant_id.is_empty() || connector.tenant_id == tenant_id)
            .cloned()
            .collect()
    }

    /// Go `Get`: one connector by id.
    #[must_use]
    pub fn get(&self, connector_id: &str) -> Option<Connector> {
        self.inner.read().by_id.get(connector_id).cloned()
    }

    /// Go `GetForTenant`: one connector by id, tenant-checked when a tenant is
    /// given.
    #[must_use]
    pub fn get_for_tenant(&self, connector_id: &str, tenant_id: &str) -> Option<Connector> {
        let inner = self.inner.read();
        let connector = inner.by_id.get(connector_id)?;
        if !tenant_id.is_empty() && connector.tenant_id != tenant_id {
            return None;
        }
        Some(connector.clone())
    }

    /// Go `RequireInboundReady`: resolves the connector for inbound traffic and
    /// rejects disabled connectors.
    pub fn require_inbound_ready(
        &self,
        connector_id: &str,
        tenant_id: &str,
    ) -> Result<Connector, ConnectorsError> {
        let connector = self
            .get_for_tenant(connector_id, tenant_id)
            .ok_or(ConnectorsError::ConnectorNotFound)?;
        if connector.status == Status::Disabled {
            return Err(ConnectorsError::ConnectorDisabled);
        }
        Ok(connector)
    }

    /// Go `WithConnectorMutation`: runs the closure under the per-connector
    /// mutation lock so check-then-act sequences on one connector are serialized.
    pub fn with_connector_mutation(
        &self,
        connector_id: &str,
        f: impl FnOnce() -> Result<(), ConnectorsError>,
    ) -> Result<(), ConnectorsError> {
        let lock = self.connector_mutation_lock(connector_id);
        let _guard = lock.lock();
        f()
    }

    /// Go `connectorMutationLock`: lazily creates and returns the per-connector
    /// mutation lock. The main lock is released before the returned lock is used.
    fn connector_mutation_lock(&self, connector_id: &str) -> Arc<Mutex<()>> {
        let mut inner = self.inner.write();
        inner
            .mutation_locks
            .entry(connector_id.to_string())
            .or_insert_with(|| Arc::new(Mutex::new(())))
            .clone()
    }

    /// Go `ReportHealth`: records a healthy/degraded heartbeat, resetting the
    /// failure/backoff state.
    pub fn report_health(
        &self,
        connector_id: &str,
        input: ReportHealthInput,
    ) -> Result<Connector, ConnectorsError> {
        if input.status != Status::Healthy && input.status != Status::Degraded {
            return Err(ConnectorsError::InvalidConnectorHealth);
        }
        let mut inner = self.inner.write();
        let connector = inner
            .by_id
            .get_mut(connector_id)
            .ok_or(ConnectorsError::ConnectorNotFound)?;
        if connector.status == Status::Disabled {
            return Err(ConnectorsError::ConnectorDisabled);
        }
        let now = Utc::now();
        connector.status = input.status;
        connector.failure_count = 0;
        connector.backoff_seconds = 0;
        connector.next_restart_at = None;
        connector.last_heartbeat_at = Some(now);
        connector.updated_at = now;
        Ok(connector.clone())
    }

    /// Go `ReportFailure`: records a failure, advancing the exponential backoff
    /// and circuit-breaking to `Failed` at the threshold.
    pub fn report_failure(
        &self,
        connector_id: &str,
        input: ReportFailureInput,
    ) -> Result<Connector, ConnectorsError> {
        if input.reason.is_empty() {
            return Err(ConnectorsError::ConnectorFailureRequired);
        }
        let mut inner = self.inner.write();
        let connector = inner
            .by_id
            .get_mut(connector_id)
            .ok_or(ConnectorsError::ConnectorNotFound)?;
        if connector.status == Status::Disabled {
            return Err(ConnectorsError::ConnectorDisabled);
        }
        let now = Utc::now();
        connector.failure_count += 1;
        connector.last_failure_reason = input.reason;
        connector.updated_at = now;

        if connector.failure_count >= FAILURE_THRESHOLD {
            connector.status = Status::Failed;
            connector.backoff_seconds = 0;
            connector.next_restart_at = None;
            return Ok(connector.clone());
        }

        let backoff = min_int(
            5i64 * (1i64 << ((connector.failure_count - 1) as u32)),
            MAX_BACKOFF_SECONDS,
        );
        connector.status = Status::BackingOff;
        connector.backoff_seconds = backoff;
        connector.next_restart_at = Some(now + chrono::Duration::seconds(backoff));
        Ok(connector.clone())
    }

    /// Go `Restart`: marks a connector for restart, resetting backoff.
    pub fn restart(&self, connector_id: &str) -> Result<Connector, ConnectorsError> {
        let mut inner = self.inner.write();
        let connector = inner
            .by_id
            .get_mut(connector_id)
            .ok_or(ConnectorsError::ConnectorNotFound)?;
        if connector.status == Status::Disabled {
            return Err(ConnectorsError::ConnectorDisabled);
        }
        let now = Utc::now();
        connector.status = Status::Registered;
        connector.restart_count += 1;
        connector.backoff_seconds = 0;
        connector.next_restart_at = None;
        connector.last_restart_at = Some(now);
        connector.updated_at = now;
        Ok(connector.clone())
    }

    /// Go `Disable`: disables a connector with a reason, clearing backoff.
    pub fn disable(&self, connector_id: &str, reason: &str) -> Result<Connector, ConnectorsError> {
        let mut inner = self.inner.write();
        let connector = inner
            .by_id
            .get_mut(connector_id)
            .ok_or(ConnectorsError::ConnectorNotFound)?;
        let now = Utc::now();
        connector.status = Status::Disabled;
        connector.disabled_reason = reason.to_string();
        connector.backoff_seconds = 0;
        connector.next_restart_at = None;
        connector.updated_at = now;
        Ok(connector.clone())
    }

    /// Go `ReEnable`: re-enables a connector back to `Registered`.
    pub fn re_enable(&self, connector_id: &str) -> Result<Connector, ConnectorsError> {
        let mut inner = self.inner.write();
        let connector = inner
            .by_id
            .get_mut(connector_id)
            .ok_or(ConnectorsError::ConnectorNotFound)?;
        let now = Utc::now();
        connector.status = Status::Registered;
        connector.disabled_reason = String::new();
        connector.updated_at = now;
        Ok(connector.clone())
    }

    /// Go `Restore`: replaces the registry with persisted state; per-connector
    /// mutation locks are kept.
    pub fn restore(&self, items: Vec<Connector>) {
        let mut inner = self.inner.write();
        inner.by_id = HashMap::with_capacity(items.len());
        inner.order = Vec::with_capacity(items.len());
        for item in items {
            let id = item.connector_id.clone();
            inner.order.push(id.clone());
            inner.by_id.insert(id, item);
        }
    }
}

/// Go `cleanStrings`: trims, drops empties, and dedupes preserving first
/// occurrence order.
#[must_use]
pub fn clean_strings(values: Vec<String>) -> Vec<String> {
    let mut items = Vec::with_capacity(values.len());
    let mut seen = HashSet::new();
    for value in values {
        let trimmed = value.trim().to_string();
        if trimmed.is_empty() {
            continue;
        }
        if !seen.insert(trimmed.clone()) {
            continue;
        }
        items.push(trimmed);
    }
    items
}

/// Go `minInt`.
#[must_use]
pub fn min_int(a: i64, b: i64) -> i64 {
    if a < b { a } else { b }
}
