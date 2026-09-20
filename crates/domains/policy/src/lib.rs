//! Port of `daemon/internal/policy`: the action-approval engine that gates
//! side-effecting operations behind a human resolution.

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};

macro_rules! string_enum {
    ($name:ident { $first:ident => $first_s:literal $(, $v:ident => $s:literal)* $(,)? }) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
        pub enum $name {
            #[default]
            #[serde(rename = $first_s)]
            $first,
            $(#[serde(rename = $s)] $v),*
        }
        impl $name {
            #[must_use]
            pub fn as_str(self) -> &'static str {
                match self {
                    $name::$first => $first_s,
                    $( $name::$v => $s ),*
                }
            }
        }
        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str(self.as_str())
            }
        }
    };
}

string_enum!(ApprovalStatus {
    Pending => "pending",
    Approved => "approved",
    Rejected => "rejected",
});

string_enum!(DecisionOutcome {
    Allowed => "allowed",
    RequiresApproval => "requires_approval",
    Approved => "approved",
    Rejected => "rejected",
});

#[derive(Debug, thiserror::Error, Clone, PartialEq, Eq)]
pub enum PolicyError {
    #[error("action is required")]
    ActionRequired,
    #[error("reason is required")]
    ReasonRequired,
    #[error("approval not found")]
    ApprovalNotFound,
    #[error("approval is not pending")]
    ApprovalNotPending,
    #[error("invalid approval resolution")]
    InvalidResolution,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Approval {
    pub approval_id: String,
    pub action: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub resource_kind: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub resource_id: String,
    pub reason: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub requested_by: String,
    pub status: ApprovalStatus,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolved_at: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub resolution: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub comment: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sandbox: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub integration_bindings: Vec<kura_integrations::BindingSummary>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Decision {
    pub decision_id: String,
    pub action: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub resource_kind: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub resource_id: String,
    pub outcome: DecisionOutcome,
    pub reason: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub approval_id: String,
    pub created_at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sandbox: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RequestApprovalInput {
    pub action: String,
    pub resource_kind: String,
    pub resource_id: String,
    pub reason: String,
    pub requested_by: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub integration_bindings: Vec<kura_integrations::BindingSummary>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResolveApprovalInput {
    pub resolution: String,
    pub comment: String,
}

#[derive(Default)]
struct Inner {
    approvals_by_id: HashMap<String, Approval>,
    approval_ids: Vec<String>,
    decisions_by_id: HashMap<String, Decision>,
    decision_ids: Vec<String>,
}

pub struct Engine {
    inner: RwLock<Inner>,
}

impl Default for Engine {
    fn default() -> Self {
        Engine::new()
    }
}

impl Engine {
    #[must_use]
    pub fn new() -> Self {
        Engine {
            inner: RwLock::new(Inner::default()),
        }
    }

    pub fn request_approval(
        &self,
        input: RequestApprovalInput,
    ) -> Result<(Approval, Decision), PolicyError> {
        if input.action.is_empty() {
            return Err(PolicyError::ActionRequired);
        }
        if input.reason.is_empty() {
            return Err(PolicyError::ReasonRequired);
        }
        let now = Utc::now();
        let approval = Approval {
            approval_id: new_approval_id(),
            action: input.action.clone(),
            resource_kind: input.resource_kind.clone(),
            resource_id: input.resource_id.clone(),
            reason: input.reason.clone(),
            requested_by: input.requested_by.clone(),
            status: ApprovalStatus::Pending,
            integration_bindings: input.integration_bindings.clone(),
            created_at: now,
            updated_at: now,
            ..Approval::default()
        };
        let decision = Decision {
            decision_id: new_decision_id(),
            action: input.action,
            resource_kind: input.resource_kind,
            resource_id: input.resource_id,
            outcome: DecisionOutcome::RequiresApproval,
            reason: input.reason,
            approval_id: approval.approval_id.clone(),
            created_at: now,
            ..Decision::default()
        };

        let mut inner = self.inner.write();
        inner
            .approvals_by_id
            .insert(approval.approval_id.clone(), approval.clone());
        inner.approval_ids.push(approval.approval_id.clone());
        inner
            .decisions_by_id
            .insert(decision.decision_id.clone(), decision.clone());
        inner.decision_ids.push(decision.decision_id.clone());
        Ok((approval, decision))
    }

    #[must_use]
    pub fn list_approvals(&self, status: Option<ApprovalStatus>) -> Vec<Approval> {
        let inner = self.inner.read();
        inner
            .approval_ids
            .iter()
            .filter_map(|id| {
                let approval = &inner.approvals_by_id[id];
                if status.is_some_and(|s| approval.status != s) {
                    None
                } else {
                    Some(approval.clone())
                }
            })
            .collect()
    }

    #[must_use]
    pub fn get_approval(&self, approval_id: &str) -> Option<Approval> {
        self.inner.read().approvals_by_id.get(approval_id).cloned()
    }

    #[must_use]
    pub fn list_decisions(&self) -> Vec<Decision> {
        let inner = self.inner.read();
        inner
            .decision_ids
            .iter()
            .map(|id| inner.decisions_by_id[id].clone())
            .collect()
    }

    pub fn resolve_approval(
        &self,
        approval_id: &str,
        input: ResolveApprovalInput,
    ) -> Result<(Approval, Decision), PolicyError> {
        let mut inner = self.inner.write();
        let mut approval = inner
            .approvals_by_id
            .get(approval_id)
            .cloned()
            .ok_or(PolicyError::ApprovalNotFound)?;
        if approval.status != ApprovalStatus::Pending {
            return Err(PolicyError::ApprovalNotPending);
        }

        let (status, outcome) = match input.resolution.as_str() {
            "approved" => (ApprovalStatus::Approved, DecisionOutcome::Approved),
            "rejected" => (ApprovalStatus::Rejected, DecisionOutcome::Rejected),
            _ => return Err(PolicyError::InvalidResolution),
        };

        let now = Utc::now();
        approval.status = status;
        approval.resolution = input.resolution;
        approval.comment = input.comment;
        approval.updated_at = now;
        approval.resolved_at = Some(now);
        inner
            .approvals_by_id
            .insert(approval.approval_id.clone(), approval.clone());

        let decision = Decision {
            decision_id: new_decision_id(),
            action: approval.action.clone(),
            resource_kind: approval.resource_kind.clone(),
            resource_id: approval.resource_id.clone(),
            outcome,
            reason: approval.reason.clone(),
            approval_id: approval.approval_id.clone(),
            created_at: now,
            ..Decision::default()
        };
        inner
            .decisions_by_id
            .insert(decision.decision_id.clone(), decision.clone());
        inner.decision_ids.push(decision.decision_id.clone());
        Ok((approval, decision))
    }

    pub fn restore(&self, approvals: Vec<Approval>, decisions: Vec<Decision>) {
        let mut inner = self.inner.write();
        inner.approvals_by_id = HashMap::with_capacity(approvals.len());
        inner.approval_ids = Vec::with_capacity(approvals.len());
        for approval in approvals {
            inner.approval_ids.push(approval.approval_id.clone());
            inner
                .approvals_by_id
                .insert(approval.approval_id.clone(), approval);
        }
        inner.decisions_by_id = HashMap::with_capacity(decisions.len());
        inner.decision_ids = Vec::with_capacity(decisions.len());
        for decision in decisions {
            inner.decision_ids.push(decision.decision_id.clone());
            inner
                .decisions_by_id
                .insert(decision.decision_id.clone(), decision);
        }
    }
}

#[must_use]
fn new_approval_id() -> String {
    format!("approval_{}", random_suffix())
}

#[must_use]
fn new_decision_id() -> String {
    format!("decision_{}", random_suffix())
}

#[must_use]
fn random_suffix() -> String {
    let uuid = uuid::Uuid::new_v4().simple().to_string();
    uuid[..16].to_string()
}
