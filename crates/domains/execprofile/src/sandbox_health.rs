//! Sandbox-backed [`HealthChecker`] (wave 8 parity): projects execution-profile
//! backend health from the authoritative sandbox backend-capability view (Go
//! `sandboxExecHealth` in `daemon/internal/app/app.go`). The sandbox/policy
//! layer remains authoritative for execution permission; this only projects
//! live availability so the profile UX reflects real backend state.

use std::sync::Arc;

use kura_sandbox::BackendAvailabilityStatus;
use kura_sandbox::BackendCapabilityProfile;

use crate::ExecutionProfile;
use crate::HealthChecker;
use crate::HealthStatus;

/// Source of the sandbox backend-capability view (injectable in tests; the
/// real source is `kura_sandbox::Manager`).
pub trait SandboxCapabilitySource: Send + Sync {
    fn backend_capabilities(&self) -> Vec<BackendCapabilityProfile>;
}

impl SandboxCapabilitySource for kura_sandbox::Manager {
    fn backend_capabilities(&self) -> Vec<BackendCapabilityProfile> {
        kura_sandbox::Manager::backend_capabilities(self)
    }
}

/// Go `sandboxExecHealth`: matches the profile's backend kind against the
/// sandbox capability view (case-insensitive) and maps the availability
/// status to the execution-profile health tier. Unknown backend kinds stay
/// ready (the always-on subprocess default must not be falsely marked
/// unavailable).
pub struct SandboxHealthChecker {
    sandbox: Option<Arc<dyn SandboxCapabilitySource>>,
}

impl SandboxHealthChecker {
    #[must_use]
    pub fn new(sandbox: Option<Arc<dyn SandboxCapabilitySource>>) -> Self {
        Self { sandbox }
    }
}

impl HealthChecker for SandboxHealthChecker {
    fn health(&self, profile: &ExecutionProfile) -> (HealthStatus, String) {
        let Some(sandbox) = &self.sandbox else {
            return (HealthStatus::Ready, String::new());
        };
        for capability in sandbox.backend_capabilities() {
            if !capability
                .backend_kind
                .as_str()
                .eq_ignore_ascii_case(profile.backend_kind.as_str())
            {
                continue;
            }
            return match capability.availability_status {
                BackendAvailabilityStatus::Available => (HealthStatus::Ready, String::new()),
                BackendAvailabilityStatus::Degraded => (
                    HealthStatus::Degraded,
                    capability.availability_reason.clone(),
                ),
                BackendAvailabilityStatus::Unavailable => (
                    HealthStatus::Unavailable,
                    capability.availability_reason.clone(),
                ),
            };
        }
        // Unknown backend kind: don't falsely mark unavailable (the always-on
        // subprocess default).
        (HealthStatus::Ready, String::new())
    }
}
