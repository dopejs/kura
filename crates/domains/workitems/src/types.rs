//! The work item vocabulary: what is stored, and what the API shows
//! (`schemas/api/work-item-resource.schema.json`).

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Whether an item can still run rounds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ItemState {
    Open,
    Ended,
}

impl ItemState {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::Ended => "ended",
        }
    }

    pub(crate) fn parse(value: &str) -> Result<Self, String> {
        match value {
            "open" => Ok(Self::Open),
            "ended" => Ok(Self::Ended),
            other => Err(format!("unknown work item state {other:?}")),
        }
    }
}

/// Why an item ended. `completed` and `dropped` come from a round's report;
/// `revoked` and `stopped` arrive with revalidation and the stop route.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EndReason {
    Completed,
    Dropped,
    Revoked,
    Stopped,
}

impl EndReason {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Completed => "completed",
            Self::Dropped => "dropped",
            Self::Revoked => "revoked",
            Self::Stopped => "stopped",
        }
    }

    pub(crate) fn parse(value: &str) -> Result<Self, String> {
        match value {
            "completed" => Ok(Self::Completed),
            "dropped" => Ok(Self::Dropped),
            "revoked" => Ok(Self::Revoked),
            "stopped" => Ok(Self::Stopped),
            other => Err(format!("unknown work item end reason {other:?}")),
        }
    }
}

/// Where an open item stands (§4.1). Derived on every read, never stored.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Stage {
    #[serde(rename = "person/delivery")]
    PersonDelivery,
    #[serde(rename = "person/approval")]
    PersonApproval,
    #[serde(rename = "person/answer")]
    PersonAnswer,
    #[serde(rename = "person/blocked")]
    PersonBlocked,
    #[serde(rename = "person/failed")]
    PersonFailed,
    #[serde(rename = "queued")]
    Queued,
    #[serde(rename = "working")]
    Working,
    #[serde(rename = "idle")]
    Idle,
}

impl Stage {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::PersonDelivery => "person/delivery",
            Self::PersonApproval => "person/approval",
            Self::PersonAnswer => "person/answer",
            Self::PersonBlocked => "person/blocked",
            Self::PersonFailed => "person/failed",
            Self::Queued => "queued",
            Self::Working => "working",
            Self::Idle => "idle",
        }
    }
}

/// How a round's report says the work stands (§3.4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReportStatus {
    Progress,
    Waiting,
    Ready,
    Blocked,
    Failed,
    Dropped,
}

impl ReportStatus {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Progress => "progress",
            Self::Waiting => "waiting",
            Self::Ready => "ready",
            Self::Blocked => "blocked",
            Self::Failed => "failed",
            Self::Dropped => "dropped",
        }
    }

    /// The end this status brings, if it ends the item.
    #[must_use]
    pub fn ends_with(self) -> Option<EndReason> {
        match self {
            Self::Ready => Some(EndReason::Completed),
            Self::Dropped => Some(EndReason::Dropped),
            _ => None,
        }
    }
}

/// Why a recorded report is not the model's own. Set by the daemon only.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReportReason {
    /// The round's reply did not conform to the report schema, even after
    /// the turn's correction round; recorded as `blocked`.
    ReportUnreadable,
}

/// The one question a person must answer for the work to go on.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Attention {
    pub question: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub options: Vec<String>,
}

/// A round's report as recorded.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Report {
    pub status: ReportStatus,
    pub summary: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attention: Option<Attention>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<ReportReason>,
}

/// What a mandate rests on (§5.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Basis {
    Api,
    ImMention,
    ImDirect,
    Rule,
}

impl Basis {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Api => "api",
            Self::ImMention => "im_mention",
            Self::ImDirect => "im_direct",
            Self::Rule => "rule",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Strength {
    Explicit,
    Inferred,
}

impl Strength {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Explicit => "explicit",
            Self::Inferred => "inferred",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MatchedRule {
    pub rule_id: String,
    pub version: u32,
}

/// Who asked for the work.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Sender {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub connector_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub author_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub principal_id: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MandateSourceKind {
    Api,
    Im,
    Webhook,
}

/// Where the request came from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MandateSource {
    pub kind: MandateSourceKind,
    /// The HTTP request id, the inbound message id or the webhook delivery.
    #[serde(rename = "ref", default, skip_serializing_if = "String::is_empty")]
    pub reference: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub conversation: String,
}

/// Why the agent may do this work (§3.5). Written once with the item and
/// never updated; a resume checks it again by its basis.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Mandate {
    pub basis: Basis,
    pub strength: Strength,
    /// Empty until work rules exist (slice 6). An empty hash never takes
    /// the revalidation fast path.
    pub policy_hash: String,
    pub matched_rules: Vec<MatchedRule>,
    pub sender: Sender,
    pub source: MandateSource,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub receiving_policy_id: String,
    pub scope_text: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RoundState {
    Running,
    /// Ended with a report, or with one recorded as unreadable.
    Settled,
    /// Ended without a report: the turn failed or was refused.
    Failed,
}

impl RoundState {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Running => "running",
            Self::Settled => "settled",
            Self::Failed => "failed",
        }
    }

    pub(crate) fn parse(value: &str) -> Result<Self, String> {
        match value {
            "running" => Ok(Self::Running),
            "settled" => Ok(Self::Settled),
            "failed" => Ok(Self::Failed),
            other => Err(format!("unknown work item round state {other:?}")),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InputKind {
    Message,
    Event,
    Approval,
    PolicyNotice,
    Recovery,
    Text,
}

impl InputKind {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Message => "message",
            Self::Event => "event",
            Self::Approval => "approval",
            Self::PolicyNotice => "policy_notice",
            Self::Recovery => "recovery",
            Self::Text => "text",
        }
    }

    pub(crate) fn parse(value: &str) -> Result<Self, String> {
        match value {
            "message" => Ok(Self::Message),
            "event" => Ok(Self::Event),
            "approval" => Ok(Self::Approval),
            "policy_notice" => Ok(Self::PolicyNotice),
            "recovery" => Ok(Self::Recovery),
            "text" => Ok(Self::Text),
            other => Err(format!("unknown work item input kind {other:?}")),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InputState {
    Pending,
    Steered,
    Consumed,
}

impl InputState {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Steered => "steered",
            Self::Consumed => "consumed",
        }
    }

    pub(crate) fn parse(value: &str) -> Result<Self, String> {
        match value {
            "pending" => Ok(Self::Pending),
            "steered" => Ok(Self::Steered),
            "consumed" => Ok(Self::Consumed),
            other => Err(format!("unknown work item input state {other:?}")),
        }
    }
}

/// One round: an ordinary chat turn.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Round {
    pub seq: i64,
    pub request_id: String,
    pub state: RoundState,
    pub query_text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outcome: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub report: Option<Report>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_code: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn_id: Option<String>,
    pub started_at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub finished_at: Option<DateTime<Utc>>,
}

/// Something said to or happening to an item.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Input {
    pub seq: i64,
    pub kind: InputKind,
    pub author_is_person: bool,
    pub body: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub source_ref: String,
    pub state: InputState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub round_seq: Option<i64>,
    pub created_at: DateTime<Utc>,
}

/// A work item as the API shows it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkItem {
    pub item_id: String,
    pub tenant_id: String,
    pub title: String,
    pub state: ItemState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end_reason: Option<EndReason>,
    /// Absent once the item has ended.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stage: Option<Stage>,
    pub thread_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub provider: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub model: String,
    pub seq: i64,
    pub mandate: Mandate,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_report: Option<Report>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attention: Option<Attention>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    /// On a single-item read only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rounds: Option<Vec<Round>>,
    /// On a single-item read only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inputs: Option<Vec<Input>>,
    /// On a single-item read only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subscriptions: Option<Vec<Subscription>>,
}

/// What a subscription grants (§4, §5.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SubscriptionRole {
    /// The person who asked may wake the item from this source (D2: at
    /// most one live owner per source).
    Owner,
    /// Inputs only; grants no authority. Nothing creates one in slice 2.
    Context,
}

impl SubscriptionRole {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Owner => "owner",
            Self::Context => "context",
        }
    }

    pub(crate) fn parse(value: &str) -> Result<Self, String> {
        match value {
            "owner" => Ok(Self::Owner),
            "context" => Ok(Self::Context),
            other => Err(format!("unknown work item subscription role {other:?}")),
        }
    }
}

/// A source the item follows, live or dropped.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Subscription {
    pub subscription_id: String,
    /// `im` in slice 2.
    pub source_kind: String,
    /// Matched by equality. For IM, the conversation's `thr_src_` id.
    pub source_key: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub connector_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub conversation_id: String,
    pub role: SubscriptionRole,
    pub created_at: DateTime<Utc>,
    pub last_event_at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dropped_at: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub drop_reason: Option<String>,
}

/// Why a mandate no longer holds (§5.4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RevokeReason {
    /// The connector the request came through is disabled or failing.
    ConnectorDisabled,
    /// The route policy no longer lets the connector receive that
    /// conversation (or there is no valid policy any more).
    ConversationNotAllowed,
    /// The sender no longer passes the route policy's sender allowlist.
    SenderNotAllowed,
    /// The principal is no longer an active member of the tenant.
    PrincipalNotMember,
    /// The basis cannot be revalidated in this slice (`rule`, slice 6).
    BasisUnsupported,
}

impl RevokeReason {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ConnectorDisabled => "connector_disabled",
            Self::ConversationNotAllowed => "conversation_not_allowed",
            Self::SenderNotAllowed => "sender_not_allowed",
            Self::PrincipalNotMember => "principal_not_member",
            Self::BasisUnsupported => "basis_unsupported",
        }
    }
}

/// `entries.work-items.config` in the plugin profile.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default, deny_unknown_fields)]
pub struct WorkItemsConfig {
    /// Off by default: with `false` nothing of this feature runs and its
    /// routes answer 404 as if they did not exist.
    pub enabled: bool,
    /// Open items one tenant may hold (D7); `0` means unbounded.
    pub max_open_items: usize,
    /// A subscription that saw no event for this many days is dropped as
    /// `idle` (D7, §11); `0` means never.
    pub subscription_idle_days: u32,
    /// Rounds one item may start in any rolling hour (D7, §11); `0` means
    /// unbounded. Inputs over the cap stay queued until the item is woken
    /// again.
    pub max_rounds_per_item_per_hour: u32,
}

impl Default for WorkItemsConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            max_open_items: 50,
            subscription_idle_days: 14,
            max_rounds_per_item_per_hour: 20,
        }
    }
}
