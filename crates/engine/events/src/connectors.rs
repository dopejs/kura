//! Connector event name constants (port of `connectors.go`).

pub const CONNECTOR_EVENT_MATRIX_SETUP_VALIDATED: &str = "connector.matrix_setup_validated";
pub const CONNECTOR_EVENT_MATRIX_SMOKE_EVIDENCE_RECORDED: &str =
    "connector.matrix_smoke_evidence_recorded";
pub const CONNECTOR_EVENT_ROUTE_OUTCOME_RECORDED: &str = "connector.route_outcome_recorded";
pub const CONNECTOR_EVENT_INBOUND_DUPLICATE_DETECTED: &str = "connector.inbound_duplicate_detected";
pub const CONNECTOR_EVENT_REPLY_SENT: &str = "connector.reply_sent";
pub const CONNECTOR_EVENT_REPLY_FAILED: &str = "connector.reply_failed";
pub const CONNECTOR_EVENT_FOREGROUND_REPLY_FAILED: &str = "connector.foreground_reply_failed";
pub const CONNECTOR_EVENT_DELIVERY_SEPARATION_RECORDED: &str =
    "connector.delivery_separation_recorded";
pub const CONNECTOR_EVENT_DIAGNOSTIC_STATE_CHANGED: &str = "connector.diagnostic_state_changed";
pub const CONNECTOR_EVENT_DIAGNOSTIC_REDACTION_FAILED: &str =
    "connector.diagnostic_redaction_failed";
pub const CONNECTOR_EVENT_CONNECTOR_DIAGNOSTIC_RECORDED: &str = "connector.diagnostic_recorded";
pub const CONNECTOR_EVENT_CONNECTOR_SETUP_REPAIR_REQUIRED: &str = "connector.setup_repair_required";
pub const CONNECTOR_EVENT_MANAGEMENT_REDACTION_FAILED: &str =
    "connector.management_redaction_failed";
pub const CONNECTOR_EVENT_SUPPORT_EVIDENCE_GENERATED: &str =
    "connector.management_support_evidence_generated";
pub const CONNECTOR_EVENT_MANAGEMENT_RETENTION_APPLIED: &str =
    "connector.management_retention_applied";

/// Go: `MatrixConnectorEventNames` — the subset of connector events the Matrix
/// connector emits (Roadmap 52 evidence).
pub const MATRIX_CONNECTOR_EVENT_NAMES: [&str; 10] = [
    CONNECTOR_EVENT_MATRIX_SETUP_VALIDATED,
    CONNECTOR_EVENT_ROUTE_OUTCOME_RECORDED,
    CONNECTOR_EVENT_INBOUND_DUPLICATE_DETECTED,
    CONNECTOR_EVENT_REPLY_SENT,
    CONNECTOR_EVENT_REPLY_FAILED,
    CONNECTOR_EVENT_FOREGROUND_REPLY_FAILED,
    CONNECTOR_EVENT_DELIVERY_SEPARATION_RECORDED,
    CONNECTOR_EVENT_DIAGNOSTIC_STATE_CHANGED,
    CONNECTOR_EVENT_DIAGNOSTIC_REDACTION_FAILED,
    CONNECTOR_EVENT_MATRIX_SMOKE_EVIDENCE_RECORDED,
];
