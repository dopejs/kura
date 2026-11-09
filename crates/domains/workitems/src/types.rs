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

/// The question a round's report asks a person (§3.4), as the model wrote
/// it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ReportAttention {
    pub question: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub options: Vec<String>,
}

/// What an item's attention waits for (§7).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AttentionKind {
    /// A person's answer to the report's question: their next message on
    /// the owner conversation, or `POST /v1/work-items/{id}/answer`.
    #[default]
    Question,
    /// A pending approval a round's tool call gave up waiting on; resolved
    /// through `/v1/policy/approvals/{id}/resolve` like any other.
    Approval,
}

impl AttentionKind {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Question => "question",
            Self::Approval => "approval",
        }
    }
}

/// The one thing a person must do for the work to go on. Set only by a
/// round: from its report's structured `attention`, or by the daemon when
/// the round's tool call left an approval pending (slice 5).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Attention {
    /// Absent in rows written before slice 5: a question.
    #[serde(default)]
    pub kind: AttentionKind,
    /// The report's question, or for an approval what it asks (its reason).
    pub question: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub options: Vec<String>,
    /// The approval waited on, for kind `approval`.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub approval_id: String,
}

impl Attention {
    /// The report's question as the item's attention.
    #[must_use]
    pub fn question(asked: &ReportAttention) -> Self {
        Self {
            kind: AttentionKind::Question,
            question: asked.question.clone(),
            options: asked.options.clone(),
            approval_id: String::new(),
        }
    }

    /// A pending approval as the item's attention.
    #[must_use]
    pub fn approval(approval_id: &str, question: &str) -> Self {
        Self {
            kind: AttentionKind::Approval,
            question: question.to_string(),
            options: Vec::new(),
            approval_id: approval_id.to_string(),
        }
    }
}

/// A round's report as recorded.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Report {
    pub status: ReportStatus,
    pub summary: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attention: Option<ReportAttention>,
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
    /// On a single-item read only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outbox: Option<Vec<OutboxRow>>,
    /// On a single-item read only (slice 5).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approvals: Option<Vec<ApprovalLink>>,
}

/// Where an approval an item waited on stands (§7, slice 5).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalLinkState {
    /// Not resolved yet; the item waits for it.
    Pending,
    /// A person approved it; the item's next round may run the call once.
    Approved,
    Rejected,
    /// The policy engine no longer has it.
    Gone,
    /// A round ran the approved call under it.
    Used,
}

impl ApprovalLinkState {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Approved => "approved",
            Self::Rejected => "rejected",
            Self::Gone => "gone",
            Self::Used => "used",
        }
    }

    pub(crate) fn parse(value: &str) -> Result<Self, String> {
        match value {
            "pending" => Ok(Self::Pending),
            "approved" => Ok(Self::Approved),
            "rejected" => Ok(Self::Rejected),
            "gone" => Ok(Self::Gone),
            "used" => Ok(Self::Used),
            other => Err(format!("unknown work item approval state {other:?}")),
        }
    }
}

/// An approval a round's tool call gave up waiting on, which the item
/// waits on (§7). The approval itself is in `/v1/policy/approvals`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ApprovalLink {
    pub approval_id: String,
    /// The round whose call last waited on it.
    pub round_seq: i64,
    pub state: ApprovalLinkState,
    /// What the person is asked: the approval's reason.
    pub question: String,
    pub created_at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolved_at: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub used_at: Option<DateTime<Utc>>,
}

/// One entry of the attention queue (`GET /v1/work-items/attention`): an
/// open item in a `person/*` stage.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AttentionEntry {
    pub item_id: String,
    pub title: String,
    pub stage: Stage,
    pub basis: Basis,
    /// The question or the approval, for `person/answer` and
    /// `person/approval`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attention: Option<Attention>,
    /// The last report's summary: why a blocked or failed item stopped.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    /// When the item last changed: roughly how long it has waited.
    pub updated_at: DateTime<Utc>,
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

/// Where an outbox row stands (§8, slice 3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OutboxState {
    /// Not tried yet.
    Queued,
    /// Tried and failed; tried again at `nextAttemptAt`.
    Failed,
    Sent,
    /// Never sent and never queued again; `dropReason` says why.
    Dropped,
}

impl OutboxState {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Failed => "failed",
            Self::Sent => "sent",
            Self::Dropped => "dropped",
        }
    }

    pub(crate) fn parse(value: &str) -> Result<Self, String> {
        match value {
            "queued" => Ok(Self::Queued),
            "failed" => Ok(Self::Failed),
            "sent" => Ok(Self::Sent),
            "dropped" => Ok(Self::Dropped),
            other => Err(format!("unknown work item outbox state {other:?}")),
        }
    }
}

/// Why an outbox row was dropped (§8): the only ways a reply is given up.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OutboxDropReason {
    /// The connector no longer receives the conversation (its route policy
    /// stopped receiving it).
    ConnectorGone,
    /// The platform says the conversation no longer exists.
    ConversationGone,
    /// The platform says the agent may never post there.
    Forbidden,
    /// A person withdrew the reply before it was sent.
    Withdrawn,
}

impl OutboxDropReason {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ConnectorGone => "connector_gone",
            Self::ConversationGone => "conversation_gone",
            Self::Forbidden => "forbidden",
            Self::Withdrawn => "withdrawn",
        }
    }

    pub(crate) fn parse(value: &str) -> Result<Self, String> {
        match value {
            "connector_gone" => Ok(Self::ConnectorGone),
            "conversation_gone" => Ok(Self::ConversationGone),
            "forbidden" => Ok(Self::Forbidden),
            "withdrawn" => Ok(Self::Withdrawn),
            other => Err(format!("unknown work item outbox drop reason {other:?}")),
        }
    }
}

/// A reply the item sends to a conversation, in order (§8). The body is
/// shown here, as an input's is: the item read is tenant-scoped. Events
/// and logs never carry it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OutboxRow {
    pub seq: i64,
    /// With the item, the order key: rows of one conversation go out in
    /// `seq` order. For IM, the conversation's `thr_src_` id.
    pub conversation_key: String,
    pub connector_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub conversation_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub round_seq: Option<i64>,
    pub body: String,
    pub state: OutboxState,
    /// Failed attempts in a row.
    pub attempts: i64,
    /// When an unsent row is tried next (once every earlier row of its
    /// conversation is sent or dropped).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_attempt_at: Option<DateTime<Utc>>,
    /// A stable code for the last failure; never the transport's own text.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub drop_reason: Option<OutboxDropReason>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub external_message_id: String,
    pub created_at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sent_at: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dropped_at: Option<DateTime<Utc>>,
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
    /// At boot, rerun the rounds a previous process left running (D7, §6),
    /// with the same request id so they resume from their checkpoint. With
    /// `false` they are ended as interrupted and wait for a person.
    pub recover_on_boot: bool,
}

impl Default for WorkItemsConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            max_open_items: 50,
            subscription_idle_days: 14,
            max_rounds_per_item_per_hour: 20,
            recover_on_boot: true,
        }
    }
}
