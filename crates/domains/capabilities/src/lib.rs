//! Port of `daemon/internal/capabilities`: supervised child-process capability
//! registration, health/failure/restart supervision, and the integration-adapter runtime
//! bridge (Roadmap 59).

mod adapter_runtime;
mod supervisor;

pub use adapter_runtime::{
    AdapterHealthEvent, AdapterRuntime, KIND_INTEGRATION_ADAPTER, Readiness, RuntimeError,
    start_adapter_runtime,
};
pub use supervisor::{
    Capability, RegisterInput, ReportFailureInput, ReportHealthInput, Status, Supervisor,
    SupervisorError,
};
