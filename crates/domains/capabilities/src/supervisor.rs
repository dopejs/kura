//! Capability supervision (port of `supervisor.go`): an in-memory registry of supervised
//! child-process capabilities with health/failure/restart state and a circuit-break after
//! repeated failures.

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Supervisor validation/lookup failures (Go sentinel errors).
#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum SupervisorError {
    #[error("capability id is required")]
    CapabilityIdRequired,
    #[error("capability kind is required")]
    CapabilityKindRequired,
    #[error("capability not found")]
    CapabilityNotFound,
    #[error("invalid capability health status")]
    InvalidCapabilityHealth,
    #[error("capability failure reason is required")]
    CapabilityFailureRequired,
}

/// Lifecycle status of a supervised capability.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Registered,
    Healthy,
    Degraded,
    BackingOff,
    Failed,
}

impl Status {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Status::Registered => "registered",
            Status::Healthy => "healthy",
            Status::Degraded => "degraded",
            Status::BackingOff => "backing_off",
            Status::Failed => "failed",
        }
    }
}

impl std::fmt::Display for Status {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A supervised capability and its operational state.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Capability {
    pub capability_id: String,
    pub kind: String,
    pub display_name: String,
    pub status: Status,
    pub failure_count: i64,
    pub restart_count: i64,
    pub backoff_seconds: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_restart_at: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_restart_at: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_heartbeat_at: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub last_failure_reason: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Input to [`Supervisor::register`].
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RegisterInput {
    pub capability_id: String,
    pub kind: String,
    pub display_name: String,
}

/// Input to [`Supervisor::report_health`].
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReportHealthInput {
    pub status: Status,
}

/// Input to [`Supervisor::report_failure`].
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReportFailureInput {
    pub reason: String,
}

/// Failure threshold after which a capability circuit-breaks to `Failed`.
const FAILURE_THRESHOLD: i64 = 5;

/// Maximum backoff between restarts, in seconds.
const MAX_BACKOFF_SECONDS: i64 = 300;

#[derive(Default)]
struct Inner {
    by_id: HashMap<String, Capability>,
    order: Vec<String>,
}

/// Thread-safe registry of supervised capabilities.
pub struct Supervisor {
    inner: RwLock<Inner>,
}

impl Default for Supervisor {
    fn default() -> Self {
        Supervisor::new()
    }
}

impl Supervisor {
    #[must_use]
    pub fn new() -> Self {
        Supervisor {
            inner: RwLock::new(Inner::default()),
        }
    }

    /// Registers a capability, returning it and whether it was newly created.
    pub fn register(&self, input: RegisterInput) -> Result<(Capability, bool), SupervisorError> {
        if input.capability_id.is_empty() {
            return Err(SupervisorError::CapabilityIdRequired);
        }
        if input.kind.is_empty() {
            return Err(SupervisorError::CapabilityKindRequired);
        }

        let mut inner = self.inner.write();
        let now = Utc::now();
        if let Some(existing) = inner.by_id.get_mut(&input.capability_id) {
            existing.kind = input.kind;
            existing.display_name = input.display_name;
            existing.updated_at = now;
            let capability = existing.clone();
            return Ok((capability, false));
        }

        let capability = Capability {
            capability_id: input.capability_id.clone(),
            kind: input.kind,
            display_name: input.display_name,
            status: Status::Registered,
            failure_count: 0,
            restart_count: 0,
            backoff_seconds: 0,
            next_restart_at: None,
            last_restart_at: None,
            last_heartbeat_at: None,
            last_failure_reason: String::new(),
            created_at: now,
            updated_at: now,
        };
        inner
            .by_id
            .insert(input.capability_id.clone(), capability.clone());
        inner.order.push(input.capability_id);
        Ok((capability, true))
    }

    /// Lists capabilities in registration order.
    #[must_use]
    pub fn list(&self) -> Vec<Capability> {
        let inner = self.inner.read();
        inner
            .order
            .iter()
            .filter_map(|id| inner.by_id.get(id))
            .cloned()
            .collect()
    }

    /// Returns a capability by id.
    #[must_use]
    pub fn get(&self, capability_id: &str) -> Option<Capability> {
        self.inner.read().by_id.get(capability_id).cloned()
    }

    /// Records a healthy/degraded heartbeat, resetting failure state.
    pub fn report_health(
        &self,
        capability_id: &str,
        input: ReportHealthInput,
    ) -> Result<Capability, SupervisorError> {
        if input.status != Status::Healthy && input.status != Status::Degraded {
            return Err(SupervisorError::InvalidCapabilityHealth);
        }
        let mut inner = self.inner.write();
        let capability = inner
            .by_id
            .get_mut(capability_id)
            .ok_or(SupervisorError::CapabilityNotFound)?;
        let now = Utc::now();
        capability.status = input.status;
        capability.failure_count = 0;
        capability.backoff_seconds = 0;
        capability.next_restart_at = None;
        capability.last_heartbeat_at = Some(now);
        capability.updated_at = now;
        Ok(capability.clone())
    }

    /// Records a failure, advancing backoff and circuit-breaking after the threshold.
    pub fn report_failure(
        &self,
        capability_id: &str,
        input: ReportFailureInput,
    ) -> Result<Capability, SupervisorError> {
        if input.reason.is_empty() {
            return Err(SupervisorError::CapabilityFailureRequired);
        }
        let mut inner = self.inner.write();
        let capability = inner
            .by_id
            .get_mut(capability_id)
            .ok_or(SupervisorError::CapabilityNotFound)?;
        let now = Utc::now();
        capability.failure_count += 1;
        capability.last_failure_reason = input.reason;
        capability.updated_at = now;

        if capability.failure_count >= FAILURE_THRESHOLD {
            capability.status = Status::Failed;
            capability.backoff_seconds = 0;
            capability.next_restart_at = None;
            return Ok(capability.clone());
        }

        let backoff = 5i64 * (1i64 << ((capability.failure_count - 1) as u32));
        let backoff = backoff.min(MAX_BACKOFF_SECONDS);
        capability.status = Status::BackingOff;
        capability.backoff_seconds = backoff;
        capability.next_restart_at = Some(now + chrono::Duration::seconds(backoff));
        Ok(capability.clone())
    }

    /// Marks a capability for restart, resetting backoff.
    pub fn restart(&self, capability_id: &str) -> Result<Capability, SupervisorError> {
        let mut inner = self.inner.write();
        let capability = inner
            .by_id
            .get_mut(capability_id)
            .ok_or(SupervisorError::CapabilityNotFound)?;
        let now = Utc::now();
        capability.status = Status::Registered;
        capability.restart_count += 1;
        capability.backoff_seconds = 0;
        capability.next_restart_at = None;
        capability.last_restart_at = Some(now);
        capability.updated_at = now;
        Ok(capability.clone())
    }

    /// Replaces the registry with persisted state (restart recovery).
    pub fn restore(&self, items: Vec<Capability>) {
        let mut inner = self.inner.write();
        inner.by_id = HashMap::with_capacity(items.len());
        inner.order = Vec::with_capacity(items.len());
        for item in items {
            let id = item.capability_id.clone();
            inner.order.push(id.clone());
            inner.by_id.insert(id, item);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capability_supervisor_lifecycle() {
        let supervisor = Supervisor::new();

        let (capability, created) = supervisor
            .register(RegisterInput {
                capability_id: "shell".to_string(),
                kind: "exec".to_string(),
                display_name: "Shell".to_string(),
            })
            .expect("register");
        assert!(created, "expected first register to create capability");
        assert_eq!(capability.status, Status::Registered);

        let capability = supervisor
            .report_health(
                &capability.capability_id,
                ReportHealthInput {
                    status: Status::Healthy,
                },
            )
            .expect("report health");
        assert_eq!(capability.status, Status::Healthy);

        let capability = supervisor
            .report_failure(
                &capability.capability_id,
                ReportFailureInput {
                    reason: "worker crashed".to_string(),
                },
            )
            .expect("report failure");
        assert_eq!(capability.status, Status::BackingOff);

        let capability = supervisor
            .restart(&capability.capability_id)
            .expect("restart");
        assert_eq!(capability.status, Status::Registered);
        assert_eq!(capability.restart_count, 1);
    }

    #[test]
    fn capability_supervisor_fails_after_repeated_failures() {
        let supervisor = Supervisor::new();
        let (mut capability, _) = supervisor
            .register(RegisterInput {
                capability_id: "browser".to_string(),
                kind: "browser".to_string(),
                display_name: String::new(),
            })
            .expect("register");

        for _ in 0..5 {
            capability = supervisor
                .report_failure(
                    &capability.capability_id,
                    ReportFailureInput {
                        reason: "crash loop".to_string(),
                    },
                )
                .expect("report failure");
        }
        assert_eq!(capability.status, Status::Failed);
    }
}
