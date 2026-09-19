//! Integration-adapter runtime bridge (port of `adapter_runtime.go`): bridges an
//! integration adapter's RPC client into the capability supervisor, gating readiness on
//! the contract-version handshake and projecting adapter health for observability.

use std::sync::Arc;

use kura_adapterrpc::{CONTRACT_VERSION, Client, Error};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::supervisor::{
    RegisterInput, ReportFailureInput, ReportHealthInput, Status, Supervisor, SupervisorError,
};

/// The capability kind for supervised integration adapter processes (Roadmap 59).
pub const KIND_INTEGRATION_ADAPTER: &str = "integration_adapter";

/// Integration-facing readiness derived from supervisor status.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Readiness {
    Pending,
    Ready,
    Unavailable,
}

impl Readiness {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Readiness::Pending => "pending",
            Readiness::Ready => "ready",
            Readiness::Unavailable => "unavailable",
        }
    }
}

/// Failure of [`AdapterRuntime::restart`]: supervisor or adapter error.
#[derive(Debug, Error)]
pub enum RuntimeError {
    #[error(transparent)]
    Supervisor(#[from] SupervisorError),
    #[error(transparent)]
    Adapter(#[from] Error),
}

/// Bridges an adapter RPC client into the capability supervisor.
pub struct AdapterRuntime {
    sup: Arc<Supervisor>,
    capability_id: String,
    domain: String,
    contract_version: String,
    client: Arc<Client>,
}

/// Registers the adapter as a capability and returns its runtime. Call [`AdapterRuntime::probe`]
/// to run the readiness handshake and begin reporting health.
#[must_use]
pub fn start_adapter_runtime(
    sup: Arc<Supervisor>,
    capability_id: String,
    domain: String,
    client: Arc<Client>,
) -> AdapterRuntime {
    let _ = sup.register(RegisterInput {
        capability_id: capability_id.clone(),
        kind: KIND_INTEGRATION_ADAPTER.to_string(),
        display_name: format!("{domain} integration adapter"),
    });
    AdapterRuntime {
        sup,
        capability_id,
        domain,
        contract_version: CONTRACT_VERSION.to_string(),
        client,
    }
}

impl AdapterRuntime {
    /// Runs the contract-version readiness/heartbeat handshake and updates supervisor
    /// state. A contract mismatch or unreachable adapter reports failure, driving backoff
    /// and circuit-break.
    pub fn probe(&self) -> Result<(), Error> {
        if let Err(err) = self.client.ready() {
            let _ = self.sup.report_failure(
                &self.capability_id,
                ReportFailureInput {
                    reason: err.to_string(),
                },
            );
            return Err(err);
        }
        let _ = self.sup.report_health(
            &self.capability_id,
            ReportHealthInput {
                status: Status::Healthy,
            },
        );
        Ok(())
    }

    /// Asks the supervisor to restart the adapter (resetting backoff) and re-probes.
    pub fn restart(&self) -> Result<(), RuntimeError> {
        self.sup.restart(&self.capability_id)?;
        Ok(self.probe()?)
    }

    #[must_use]
    pub fn capability_id(&self) -> &str {
        &self.capability_id
    }

    /// Maps supervisor status to integration readiness. A circuit-broken adapter
    /// (`Status::Failed`) is reported unavailable.
    #[must_use]
    pub fn readiness(&self) -> Readiness {
        let Some(capability) = self.sup.get(&self.capability_id) else {
            return Readiness::Unavailable;
        };
        match capability.status {
            Status::Healthy => Readiness::Ready,
            Status::Failed => Readiness::Unavailable,
            _ => Readiness::Pending,
        }
    }

    /// Whether the adapter should receive operations.
    #[must_use]
    pub fn available(&self) -> bool {
        self.readiness() == Readiness::Ready
    }

    /// Projects current state onto the adapter-health event surface.
    #[must_use]
    pub fn health_event(&self) -> AdapterHealthEvent {
        let capability = self.sup.get(&self.capability_id);
        AdapterHealthEvent {
            capability_id: self.capability_id.clone(),
            domain: self.domain.clone(),
            status: capability
                .as_ref()
                .map(|c| c.status.as_str().to_string())
                .unwrap_or_default(),
            readiness: self.readiness().as_str().to_string(),
            restart_count: capability.as_ref().map(|c| c.restart_count).unwrap_or(0),
            failure_count: capability.as_ref().map(|c| c.failure_count).unwrap_or(0),
            contract_version: self.contract_version.clone(),
            reason: capability
                .map(|c| c.last_failure_reason)
                .unwrap_or_default(),
        }
    }
}

/// Adapter health projection for observability/event fan-out.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AdapterHealthEvent {
    pub capability_id: String,
    pub domain: String,
    pub status: String,
    pub readiness: String,
    pub restart_count: i64,
    pub failure_count: i64,
    pub contract_version: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub reason: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use kura_adapterref::{Options, new_pipe_client, new_pipe_client_with_options};

    #[test]
    fn adapter_runtime_readiness_gate_and_observability() {
        let sup = Arc::new(Supervisor::new());
        let client = Arc::new(new_pipe_client());
        let rt = start_adapter_runtime(
            sup.clone(),
            "cap-cal".to_string(),
            "calendar".to_string(),
            client,
        );

        assert_eq!(rt.readiness(), Readiness::Pending);
        rt.probe().expect("probe");
        assert!(rt.available());
        assert_eq!(rt.readiness(), Readiness::Ready);

        let found = sup
            .list()
            .iter()
            .any(|c| c.capability_id == "cap-cal" && c.kind == KIND_INTEGRATION_ADAPTER);
        assert!(found, "adapter not visible on the capability surface");

        let ev = rt.health_event();
        assert_eq!(ev.domain, "calendar");
        assert_eq!(ev.readiness, "ready");
        assert!(!ev.contract_version.is_empty());
    }

    #[test]
    fn adapter_runtime_circuit_breaks_on_repeated_failures() {
        let sup = Arc::new(Supervisor::new());
        // A version-mismatched adapter fails every readiness handshake without crashing.
        let client = Arc::new(new_pipe_client_with_options(Options {
            contract_ver: "999".to_string(),
            ..Options::default()
        }));
        let rt = start_adapter_runtime(sup, "cap-mail".to_string(), "mail".to_string(), client);

        for _ in 0..5 {
            let _ = rt.probe();
        }
        assert!(
            !rt.available(),
            "circuit-broken adapter must not be available"
        );
        assert_eq!(rt.readiness(), Readiness::Unavailable);
    }

    #[test]
    fn adapter_runtime_restart_reprobes() {
        let sup = Arc::new(Supervisor::new());
        let client = Arc::new(new_pipe_client());
        let rt = start_adapter_runtime(
            sup.clone(),
            "cap-cal".to_string(),
            "calendar".to_string(),
            client,
        );

        rt.probe().expect("probe");
        rt.restart().expect("restart");
        assert!(rt.available());

        let cap = sup.get("cap-cal").expect("capability");
        assert!(cap.restart_count >= 1, "restart not recorded: {cap:?}");
    }
}
