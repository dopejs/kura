#![allow(unused_doc_comments)]
//! Port of `daemon/internal/triage` (Roadmap 65): the explicit-rule, memory-free inbox-triage
//! manager. Triage classifies mail messages against operator-defined rules and proposes visible
//! actions; every decision is transparent (matched rule + evidence) and a deterministic replay
//! candidate. There are no learned preferences and no side effects are ever performed here.
//!
//! Wire compatibility with the Go package: string enums serialize as their exact snake_case
//! values (`urgent`, `needs_reply`, ...) and structs use camelCase JSON keys with the same
//! `omitempty` behavior as the Go structs, expressed as `skip_serializing_if` (matching the
//! `kura-runtime` crate convention).
//!
//! The Go `managerdoc.Store` persistence maps onto `kura_store::{put_document, list_documents}`
//! against `kura_store::SQLiteStore`. A nil store is represented by `Option<&SQLiteStore>` and
//! every persistence call is skipped while it is `None`.

use std::collections::HashMap;
use std::sync::Arc;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

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

/// The fixed set of triage classifications (Go `Classification`, FR-003). `Fyi` is the
/// `#[default]` variant so `Classification::default()` matches the Go zero-value fallback
/// (an empty classification defaults to `fyi` in `CreatePolicy`); wire values are unchanged.
string_enum!(Classification {
    Fyi => "fyi",
    Urgent => "urgent",
    NeedsReply => "needs_reply",
    Newsletter => "newsletter",
    Blocked => "blocked",
    Unsupported => "unsupported",
});

impl Classification {
    /// Go `(Classification).valid()`. The closed Rust enum only admits valid values, so this
    /// mirrors the Go guard structurally (always true here).
    fn valid(self) -> bool {
        matches!(
            self,
            Classification::Urgent
                | Classification::NeedsReply
                | Classification::Fyi
                | Classification::Newsletter
                | Classification::Blocked
                | Classification::Unsupported
        )
    }
}

/// The proposed action for a classified message (Go `Outcome`, FR-005). All outcomes are
/// proposals; none performs a side effect without explicit downstream permission.
string_enum!(Outcome {
    NoAction => "no_action",
    DraftReply => "draft_reply",
    Reminder => "reminder",
    DeliveryDigest => "delivery_digest",
});

impl Outcome {
    /// Go `(Outcome).valid()`.
    fn valid(self) -> bool {
        matches!(
            self,
            Outcome::DraftReply | Outcome::Reminder | Outcome::DeliveryDigest | Outcome::NoAction
        )
    }
}

/// A message field a rule condition matches against (Go `ConditionField`).
string_enum!(ConditionField {
    Sender => "sender",
    Subject => "subject",
    Body => "body",
    Recipient => "recipient",
});

/// How a condition compares the field to its value (Go `ConditionOperator`).
string_enum!(ConditionOperator {
    Contains => "contains",
    Equals => "equals",
    NotContains => "not_contains",
});

/// A single match predicate; all conditions in a rule AND together (Go `Condition`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Condition {
    pub field: ConditionField,
    pub operator: ConditionOperator,
    pub value: String,
}

/// Maps a set of conditions (AND) to a classification + proposed outcome. Rules are
/// operator-defined and evaluated in order; the first fully-matching rule wins (Go `Rule`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Rule {
    pub rule_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
    pub conditions: Vec<Condition>,
    pub classification: Classification,
    pub outcome: Outcome,
}

/// An ordered set of triage rules plus a default classification for unmatched messages
/// (Go `Policy`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Policy {
    pub policy_id: String,
    pub environment_scope: String,
    pub name: String,
    pub rules: Vec<Rule>,
    pub default_classification: Classification,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// The triage input projected from a mail message snapshot (no body content beyond a preview; no
/// credential material) (Go `Message`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Message {
    pub message_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub thread_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub sender: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub recipients: Vec<String>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub subject: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub body_preview: String,
}

/// Records which condition of a rule matched, making the decision transparent
/// (Go `MatchedEvidence`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MatchedEvidence {
    pub field: ConditionField,
    pub operator: ConditionOperator,
    pub value: String,
}

/// The per-message triage outcome: classification, the matched rule (empty for the default), the
/// matched evidence, and the proposed outcome. Every decision is a replay candidate
/// (Go `Decision`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Decision {
    pub message_id: String,
    pub classification: Classification,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub matched_rule_id: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub matched_evidence: Vec<MatchedEvidence>,
    pub outcome: Outcome,
    #[serde(default, skip_serializing_if = "is_false")]
    pub default_applied: bool,
    pub replay_candidate: bool,
    pub decided_at: DateTime<Utc>,
}

/// One triage evaluation of a message set against a policy (Go `Run`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Run {
    pub run_id: String,
    pub policy_id: String,
    pub environment_scope: String,
    pub message_count: i64,
    pub decisions: Vec<Decision>,
    pub created_at: DateTime<Utc>,
}

/// Manager validation/lookup failures (Go sentinel errors `ErrPolicyNotFound`,
/// `ErrInvalidRule`, `ErrInvalidPolicy`).
#[derive(Debug, thiserror::Error, Clone, PartialEq, Eq)]
pub enum TriageError {
    #[error("triage policy not found")]
    PolicyNotFound,
    #[error("triage rule is invalid")]
    InvalidRule,
    #[error("triage policy is invalid")]
    InvalidPolicy,
}

/// Document kind used for durable triage policies (Go `docKindPolicy`).
pub const DOC_KIND_POLICY: &str = "triage_policy";

#[derive(Default)]
struct ManagerInner {
    by_id: HashMap<String, Policy>,
    ids: Vec<String>,
}

/// Owns triage policies and evaluates triage runs (Go `Manager`). Policies are in-memory with
/// `restore`/`load_from_store`; runs are pure functions of (policy, messages) so they are
/// deterministic and replayable (FR-006).
pub struct Manager {
    inner: parking_lot::RwLock<ManagerInner>,
    env: String,
    docs: Option<Arc<parking_lot::Mutex<kura_store::SQLiteStore>>>,
}

impl Manager {
    /// Go `NewManager`: a manager scoped to `environment_scope` with no persistence store.
    #[must_use]
    pub fn new(environment_scope: &str) -> Self {
        Manager {
            inner: parking_lot::RwLock::new(ManagerInner::default()),
            env: environment_scope.trim().to_string(),
            docs: None,
        }
    }

    /// Go `WithStore`: installs durable persistence for triage policies and returns the manager.
    pub fn with_store(
        &mut self,
        store: Arc<parking_lot::Mutex<kura_store::SQLiteStore>>,
    ) -> &mut Self {
        self.docs = Some(store);
        self
    }

    /// Go `Restore`: reloads policies from an in-memory slice (used by callers that already
    /// hold them).
    pub fn restore(&self, policies: Vec<Policy>) {
        let mut inner = self.inner.write();
        inner.by_id.clear();
        inner.ids.clear();
        for policy in policies {
            inner.ids.push(policy.policy_id.clone());
            inner.by_id.insert(policy.policy_id.clone(), policy);
        }
    }

    /// Go `LoadFromStore`: reloads persisted triage policies from the document store on startup.
    /// A no-op when no store is installed.
    pub fn load_from_store(&self) -> Result<(), String> {
        let Some(docs) = &self.docs else {
            return Ok(());
        };
        let policies = kura_store::list_documents::<Policy>(&docs.lock(), DOC_KIND_POLICY)?;
        self.restore(policies);
        Ok(())
    }

    /// Validates and stores a policy (Go `CreatePolicy`). Classifications and outcomes must be
    /// from the fixed sets. In the Go port the empty default classification falls back to
    /// `fyi`; because the Rust `Classification` is a closed enum the caller passes
    /// `Classification::Fyi` explicitly.
    pub fn create_policy(
        &self,
        name: &str,
        rules: Vec<Rule>,
        default_classification: Classification,
    ) -> Result<Policy, TriageError> {
        let normalized = normalize_rules(rules)?;
        if !default_classification.valid() {
            return Err(TriageError::InvalidPolicy);
        }
        let now = Utc::now();
        let policy = Policy {
            policy_id: new_id("triage_policy"),
            environment_scope: self.env.clone(),
            name: name.trim().to_string(),
            rules: normalized,
            default_classification,
            created_at: now,
            updated_at: now,
        };
        {
            let mut inner = self.inner.write();
            inner.by_id.insert(policy.policy_id.clone(), policy.clone());
            inner.ids.push(policy.policy_id.clone());
        }
        if let Some(docs) = &self.docs {
            let _ = kura_store::put_document(
                &docs.lock(),
                DOC_KIND_POLICY,
                &policy.policy_id,
                &self.env,
                "",
                &policy,
            );
        }
        Ok(policy)
    }

    /// Go `GetPolicy`.
    pub fn get_policy(&self, policy_id: &str) -> Option<Policy> {
        self.inner.read().by_id.get(policy_id.trim()).cloned()
    }

    /// Go `ListPolicies` (insertion order, mirroring the `kura-runtime` manager convention).
    pub fn list_policies(&self) -> Vec<Policy> {
        let inner = self.inner.read();
        inner
            .ids
            .iter()
            .filter_map(|id| inner.by_id.get(id).cloned())
            .collect()
    }

    /// Evaluates messages against a policy and returns a triage run with one decision per message
    /// (Go `Run`).
    pub fn run(&self, policy_id: &str, messages: &[Message]) -> Result<Run, TriageError> {
        let policy = self
            .get_policy(policy_id)
            .ok_or(TriageError::PolicyNotFound)?;
        let now = Utc::now();
        let decisions = messages
            .iter()
            .map(|msg| evaluate(&policy, msg, now))
            .collect();
        Ok(Run {
            run_id: new_id("triage_run"),
            policy_id: policy.policy_id,
            environment_scope: policy.environment_scope,
            message_count: messages.len() as i64,
            decisions,
            created_at: now,
        })
    }
}

/// Applies the first fully-matching rule (deterministic order); when none match the default
/// classification is applied with a no-action outcome. The decision records the matched rule +
/// evidence so the classification is transparent (FR-004), and is always a replay candidate
/// (FR-006) (Go `evaluate`).
fn evaluate(policy: &Policy, msg: &Message, now: DateTime<Utc>) -> Decision {
    for rule in &policy.rules {
        if let Some(evidence) = match_rule(rule, msg) {
            return Decision {
                message_id: msg.message_id.clone(),
                classification: rule.classification,
                matched_rule_id: rule.rule_id.clone(),
                matched_evidence: evidence,
                outcome: rule.outcome,
                default_applied: false,
                replay_candidate: true,
                decided_at: now,
            };
        }
    }
    Decision {
        message_id: msg.message_id.clone(),
        classification: policy.default_classification,
        matched_rule_id: String::new(),
        matched_evidence: Vec::new(),
        outcome: Outcome::NoAction,
        default_applied: true,
        replay_candidate: true,
        decided_at: now,
    }
}

/// Reports whether every condition matches (AND); an empty condition set is a catch-all
/// (Go `matchRule`).
fn match_rule(rule: &Rule, msg: &Message) -> Option<Vec<MatchedEvidence>> {
    let mut evidence = Vec::with_capacity(rule.conditions.len());
    for cond in &rule.conditions {
        if !match_condition(cond, msg) {
            return None;
        }
        evidence.push(MatchedEvidence {
            field: cond.field,
            operator: cond.operator,
            value: cond.value.clone(),
        });
    }
    Some(evidence)
}

/// Go `matchCondition`.
fn match_condition(cond: &Condition, msg: &Message) -> bool {
    let want = cond.value.trim().to_lowercase();
    match cond.field {
        ConditionField::Sender => compare_string(cond.operator, &msg.sender, &want),
        ConditionField::Subject => compare_string(cond.operator, &msg.subject, &want),
        ConditionField::Body => compare_string(cond.operator, &msg.body_preview, &want),
        ConditionField::Recipient => compare_list(cond.operator, &msg.recipients, &want),
    }
}

/// Go `compareString`.
fn compare_string(op: ConditionOperator, field: &str, want: &str) -> bool {
    let have = field.trim().to_lowercase();
    match op {
        ConditionOperator::Contains => !want.is_empty() && have.contains(want),
        ConditionOperator::Equals => have == want,
        ConditionOperator::NotContains => want.is_empty() || !have.contains(want),
    }
}

/// Go `compareList`.
fn compare_list(op: ConditionOperator, fields: &[String], want: &str) -> bool {
    match op {
        ConditionOperator::Contains => fields
            .iter()
            .any(|f| !want.is_empty() && f.trim().to_lowercase().contains(want)),
        ConditionOperator::Equals => fields.iter().any(|f| f.trim().to_lowercase() == want),
        ConditionOperator::NotContains => fields
            .iter()
            .all(|f| want.is_empty() || !f.trim().to_lowercase().contains(want)),
    }
}

/// Validates + normalizes rules: fixed classification/outcome sets, valid field/operator pairs,
/// empty-outcome defaults to `no_action`, empty rule ids generated (Go `normalizeRules`).
fn normalize_rules(rules: Vec<Rule>) -> Result<Vec<Rule>, TriageError> {
    let mut out = Vec::with_capacity(rules.len());
    for rule in rules {
        if !rule.classification.valid() {
            return Err(TriageError::InvalidRule);
        }
        let outcome = if rule.outcome == Outcome::default() {
            Outcome::NoAction
        } else {
            rule.outcome
        };
        if !outcome.valid() {
            return Err(TriageError::InvalidRule);
        }
        for cond in &rule.conditions {
            if !valid_field(cond.field) || !valid_operator(cond.operator) {
                return Err(TriageError::InvalidRule);
            }
        }
        let mut rule = rule;
        rule.outcome = outcome;
        if rule.rule_id.trim().is_empty() {
            rule.rule_id = new_id("triage_rule");
        }
        out.push(rule);
    }
    Ok(out)
}

/// Go `validField`.
fn valid_field(field: ConditionField) -> bool {
    matches!(
        field,
        ConditionField::Sender
            | ConditionField::Subject
            | ConditionField::Body
            | ConditionField::Recipient
    )
}

/// Go `validOperator`.
fn valid_operator(op: ConditionOperator) -> bool {
    matches!(
        op,
        ConditionOperator::Contains | ConditionOperator::Equals | ConditionOperator::NotContains
    )
}

/// Go `newID`: `prefix` + 16 hex chars of random bytes. The reference `kura-runtime` crate
/// derives the same shape from a v4 UUID.
#[must_use]
fn new_id(prefix: &str) -> String {
    let hex = Uuid::new_v4().simple().to_string();
    format!("{prefix}_{}", &hex[..16])
}

#[must_use]
fn is_false(v: &bool) -> bool {
    !*v
}
