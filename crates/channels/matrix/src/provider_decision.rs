//! Port of daemon/internal/connectors/matrix/provider_decision.go: the phase-52
//! provider decision that selects Matrix and rejects WhatsApp fallback.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::is_unset_time;
use crate::types::CONNECTOR_KIND;

/// Go `ProviderDecision`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderDecision {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub selected_provider: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub rejected_provider: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub decision_owner: String,
    pub decision_date: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub hosted_viability_evidence: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub provider_risk_evidence: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub unsupported_boundaries: Vec<String>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub conformance_implications: String,
    pub unsafe_matrix_dependency: bool,
}

/// Go sentinel errors `ErrDecisionOwnerRequired`, `ErrWhatsAppOutOfScope`,
/// and `ErrUnsafeMatrixDependency`.
#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum ProviderDecisionError {
    #[error("provider decision owner is required")]
    DecisionOwnerRequired,
    #[error("whatsapp is out of scope for phase 52")]
    WhatsappOutOfScope,
    #[error("matrix implementation depends on unsupported hosted behavior")]
    UnsafeMatrixDependency,
}

/// Go `Phase52ProviderDecision`.
#[must_use]
pub fn phase52_provider_decision(owner: &str, when: DateTime<Utc>) -> ProviderDecision {
    let when = if is_unset_time(&when) {
        Utc::now()
    } else {
        when
    };
    ProviderDecision {
        selected_provider: CONNECTOR_KIND.to_string(),
        rejected_provider: "whatsapp".to_string(),
        decision_owner: owner.trim().to_string(),
        decision_date: when,
        hosted_viability_evidence: "Matrix client-server API supports tenant-provided bot accounts on tenant-selected homeservers.".to_string(),
        provider_risk_evidence: "WhatsApp remains rejected for hosted-safe operation in phase 52.".to_string(),
        unsupported_boundaries: vec![
            "whatsapp".to_string(),
            "kuraagent_hosted_homeserver".to_string(),
            "matrix_account_provisioning".to_string(),
            "encrypted_rooms".to_string(),
            "e2ee_key_session_management".to_string(),
        ],
        conformance_implications: "Matrix consumes the shared channel connector conformance contract.".to_string(),
        unsafe_matrix_dependency: false,
    }
}

/// Go `ValidateProviderDecision`.
pub fn validate_provider_decision(
    decision: &ProviderDecision,
) -> Result<(), ProviderDecisionError> {
    if decision.decision_owner.trim().is_empty() {
        return Err(ProviderDecisionError::DecisionOwnerRequired);
    }
    if decision.selected_provider.trim() != CONNECTOR_KIND
        || decision.rejected_provider.trim() != "whatsapp"
    {
        return Err(ProviderDecisionError::WhatsappOutOfScope);
    }
    if decision.unsafe_matrix_dependency {
        return Err(ProviderDecisionError::UnsafeMatrixDependency);
    }
    Ok(())
}
