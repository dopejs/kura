//! Port of `daemon/internal/mail`: mail domain types, the backend seam, attachment
//! transfer policy, artifacts, and the live-validation matrix. The operation manager, adapter
//! backend, and fake backend are the next increment.

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

string_enum!(OperationClass {
    ListThreads => "list_threads",
    GetThread => "get_thread",
    GetMessage => "get_message",
    ListDrafts => "list_drafts",
    GetDraft => "get_draft",
    CreateDraft => "create_draft",
    UpdateDraft => "update_draft",
    SendMessage => "send_message",
    SendDraft => "send_draft",
    ReplyMessage => "reply_message",
    ForwardMessage => "forward_message",
    DownloadAttachment => "download_attachment",
});

string_enum!(OperationStatus {
    Requested => "requested",
    Completed => "completed",
    Failed => "failed",
    Blocked => "blocked",
    Cancelled => "cancelled",
});

string_enum!(ResultMode {
    Inspection => "inspection",
    DraftOnly => "draft_only",
    Sent => "sent",
    Blocked => "blocked",
    Failed => "failed",
});

string_enum!(SendPath {
    Direct => "direct",
    Draft => "draft",
});

string_enum!(ArtifactKind {
    ThreadSnapshot => "thread_snapshot",
    MessageSnapshot => "message_snapshot",
    DraftSnapshot => "draft_snapshot",
    AttachmentRef => "attachment_reference",
});

string_enum!(Direction {
    Inbound => "inbound",
    Outbound => "outbound",
});

string_enum!(DeliveryState {
    Received => "received",
    Sent => "sent",
    Blocked => "blocked",
    Historical => "historical",
});

string_enum!(DraftStatus {
    Draft => "draft",
    Updated => "updated",
    SendBlocked => "send_blocked",
    SentFromDraft => "sent_from_draft",
    StaleSnapshot => "stale_snapshot",
    Unavailable => "unavailable",
});

string_enum!(ComposeMode {
    NewMessage => "new_message",
    Reply => "reply",
    Forward => "forward",
});

string_enum!(AttachmentResolutionStatus {
    Resolved => "resolved",
    Unresolved => "unresolved",
    Failed => "failed",
});

string_enum!(ReplyForwardResultMode {
    Draft => "draft",
    Send => "send",
});

#[derive(Debug, thiserror::Error, Clone, PartialEq, Eq)]
pub enum MailError {
    #[error("mail integration is unavailable")]
    MailUnavailable,
    #[error("mail integration not found")]
    MailIntegrationNotFound,
    #[error("mail integration selection is invalid")]
    MailSelectionInvalid,
    #[error("mail operation not found")]
    MailOperationNotFound,
    #[error("mail account projection not found")]
    MailAccountNotFound,
    #[error("mail thread not found")]
    MailThreadNotFound,
    #[error("mail message not found")]
    MailMessageNotFound,
    #[error("mail draft not found")]
    MailDraftNotFound,
    #[error("explicit recipients are required for new outbound mail")]
    MailRecipientRequired,
    #[error("attachment reference could not be resolved")]
    MailAttachmentUnresolved,
    #[error("background final send requires explicit allowSendSideEffects permission")]
    MailBackgroundSendBlocked,
    #[error("mail backend is not configured")]
    MailBackendNotConfigured,
    #[error("{0}")]
    Adapter(AdapterFailure),
    #[error("mail adapter transport error: {0}")]
    AdapterTransport(String),
}

/// A wrapped out-of-process adapter failure carrying the stable, redacted failure class and
/// diagnostics provider kind the Manager records on the operation ledger (FR-006/FR-008).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdapterFailure {
    pub class: String,
    pub provider_kind: String,
    pub detail: String,
    pub ambiguous: bool,
    pub unavailable: bool,
}

impl AdapterFailure {
    #[must_use]
    pub fn failure_class(&self) -> &str {
        &self.class
    }
}

impl std::fmt::Display for AdapterFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if !self.detail.is_empty() {
            f.write_str(&self.detail)
        } else {
            f.write_str(&self.class)
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountProjection {
    pub mail_account_id: String,
    pub integration_id: String,
    pub domain_kind: String,
    pub environment_scope: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub account_key: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub account_label: String,
    pub readiness_status: String,
    pub canonical_default: bool,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub selection_mode: String,
    pub mailbox_address: String,
    pub mailbox_label: String,
    pub supports_thread_inspection: bool,
    pub supports_drafts: bool,
    pub supports_direct_send: bool,
    pub supports_reply: bool,
    pub supports_forward: bool,
    pub last_synced_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadSnapshot {
    pub thread_id: String,
    pub operation_id: String,
    pub integration_id: String,
    pub mail_account_id: String,
    pub subject: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub participant_summary: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub message_ids: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub draft_ids: Vec<String>,
    pub latest_message_at: DateTime<Utc>,
    pub message_count: i64,
    pub draft_count: i64,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MessageSnapshot {
    pub message_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub thread_id: String,
    pub operation_id: String,
    pub integration_id: String,
    pub mail_account_id: String,
    pub direction: Direction,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub sender_summary: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub recipient_summary: Vec<String>,
    pub subject: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub body_preview: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub reply_to_message_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub forwarded_from_message_id: String,
    pub delivery_state: DeliveryState,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attachment_ref_ids: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sent_at: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub received_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DraftSnapshot {
    pub draft_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub thread_id: String,
    pub operation_id: String,
    pub integration_id: String,
    pub mail_account_id: String,
    pub compose_mode: ComposeMode,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub source_message_id: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub recipient_summary: Vec<String>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub subject: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub body_preview: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attachment_ref_ids: Vec<String>,
    pub draft_status: DraftStatus,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AttachmentReference {
    pub attachment_ref_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub operation_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub integration_id: String,
    pub parent_kind: String,
    pub parent_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub display_name: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub media_type: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size_bytes: Option<i64>,
    pub resolution_status: AttachmentResolutionStatus,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub failure_reason: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub retention_class: String,
    #[serde(default, skip_serializing_if = "is_false")]
    pub redacted: bool,
    #[serde(default, skip_serializing_if = "is_false")]
    pub downloaded: bool,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Artifact {
    pub artifact_id: String,
    pub operation_id: String,
    pub kind: ArtifactKind,
    pub integration_id: String,
    pub environment_scope: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub thread_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub message_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub draft_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub attachment_ref_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread: Option<ThreadSnapshot>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<MessageSnapshot>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub draft: Option<DraftSnapshot>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attachment: Option<AttachmentReference>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Operation {
    pub operation_id: String,
    pub operation_class: OperationClass,
    pub status: OperationStatus,
    pub result_mode: ResultMode,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub send_path: String,
    pub integration_id: String,
    pub mail_account_id: String,
    pub environment_scope: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub selection_mode: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub thread_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub message_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub draft_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub request_summary: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub failure_class: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub failure_reason: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub diagnostic_failure: Option<kura_integrations::DiagnosticFailureProjection>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub background_send_permitted: bool,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub run_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub step_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub tool_call_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub workflow_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub workflow_step_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub schedule_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub schedule_attempt_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub delivery_id: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub artifact_ids: Vec<String>,
    pub created_at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completed_at: Option<DateTime<Utc>>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OperationSummary {
    pub operation_id: String,
    pub operation_class: OperationClass,
    pub integration_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub thread_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub message_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub draft_id: String,
    pub result_mode: ResultMode,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub send_path: String,
    pub status: OperationStatus,
    pub captured_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Selection {
    pub integration_id: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceLinkage {
    pub operation_id: String,
    pub run_id: String,
    pub step_id: String,
    pub tool_call_id: String,
    pub workflow_id: String,
    pub workflow_step_id: String,
    pub schedule_id: String,
    pub schedule_attempt_id: String,
    pub delivery_id: String,
    pub allow_send_side_effects: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AttachmentRefInput {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub attachment_ref_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub display_name: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub media_type: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size_bytes: Option<i64>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Action {
    pub operation_class: OperationClass,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub integration_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub thread_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub message_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub draft_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub compose_mode: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub result_mode: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub to: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub cc: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub bcc: Vec<String>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub subject: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub body: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attachment_refs: Vec<AttachmentRefInput>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub allow_send_side_effects: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DownloadAttachmentInput {
    pub selection: Selection,
    pub message_id: String,
    pub attachment_ref_id: String,
    pub display_name: String,
    pub media_type: String,
    pub size_bytes: i64,
    pub source: SourceLinkage,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ListThreadsInput {
    pub selection: Selection,
    pub limit: i64,
    pub cursor: String,
    pub source: SourceLinkage,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GetThreadInput {
    pub selection: Selection,
    pub thread_id: String,
    pub source: SourceLinkage,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GetMessageInput {
    pub selection: Selection,
    pub message_id: String,
    pub source: SourceLinkage,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ListDraftsInput {
    pub selection: Selection,
    pub source: SourceLinkage,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GetDraftInput {
    pub selection: Selection,
    pub draft_id: String,
    pub source: SourceLinkage,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateDraftInput {
    pub selection: Selection,
    pub compose_mode: ComposeMode,
    pub thread_id: String,
    pub source_message_id: String,
    pub to: Vec<String>,
    pub cc: Vec<String>,
    pub bcc: Vec<String>,
    pub subject: String,
    pub body: String,
    pub attachment_refs: Vec<AttachmentRefInput>,
    pub source: SourceLinkage,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateDraftInput {
    pub selection: Selection,
    pub draft_id: String,
    pub to: Vec<String>,
    pub cc: Vec<String>,
    pub bcc: Vec<String>,
    pub subject: String,
    pub body: String,
    pub attachment_refs: Vec<AttachmentRefInput>,
    pub source: SourceLinkage,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SendMessageInput {
    pub selection: Selection,
    pub to: Vec<String>,
    pub cc: Vec<String>,
    pub bcc: Vec<String>,
    pub subject: String,
    pub body: String,
    pub attachment_refs: Vec<AttachmentRefInput>,
    pub source: SourceLinkage,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SendDraftInput {
    pub selection: Selection,
    pub draft_id: String,
    pub source: SourceLinkage,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReplyMessageInput {
    pub selection: Selection,
    pub message_id: String,
    pub result_mode: ReplyForwardResultMode,
    pub subject: String,
    pub body: String,
    pub attachment_refs: Vec<AttachmentRefInput>,
    pub source: SourceLinkage,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ForwardMessageInput {
    pub selection: Selection,
    pub message_id: String,
    pub result_mode: ReplyForwardResultMode,
    pub to: Vec<String>,
    pub cc: Vec<String>,
    pub bcc: Vec<String>,
    pub subject: String,
    pub body: String,
    pub attachment_refs: Vec<AttachmentRefInput>,
    pub source: SourceLinkage,
}

#[derive(Debug, Clone, Default)]
pub struct OperationFilter {
    pub integration_id: String,
    pub run_id: String,
    pub workflow_id: String,
    pub schedule_id: String,
    pub delivery_id: String,
    pub operation_class: OperationClass,
    pub status: OperationStatus,
    pub result_mode: ResultMode,
    pub thread_id: String,
    pub message_id: String,
    pub draft_id: String,
}

pub trait Backend: Send + Sync {
    fn supports_resource(&self, resource: &kura_integrations::Resource) -> bool;
    fn project_account(
        &self,
        resource: &kura_integrations::Resource,
    ) -> Result<AccountProjection, MailError>;
    fn list_threads(
        &self,
        resource: &kura_integrations::Resource,
        account: &AccountProjection,
        input: &ListThreadsInput,
    ) -> Result<Vec<ThreadSnapshot>, MailError>;
    fn get_thread(
        &self,
        resource: &kura_integrations::Resource,
        account: &AccountProjection,
        thread_id: &str,
    ) -> Result<ThreadSnapshot, MailError>;
    fn get_message(
        &self,
        resource: &kura_integrations::Resource,
        account: &AccountProjection,
        message_id: &str,
    ) -> Result<MessageSnapshot, MailError>;
    fn list_drafts(
        &self,
        resource: &kura_integrations::Resource,
        account: &AccountProjection,
        input: &ListDraftsInput,
    ) -> Result<Vec<DraftSnapshot>, MailError>;
    fn get_draft(
        &self,
        resource: &kura_integrations::Resource,
        account: &AccountProjection,
        draft_id: &str,
    ) -> Result<DraftSnapshot, MailError>;
    fn create_draft(
        &self,
        resource: &kura_integrations::Resource,
        account: &AccountProjection,
        input: &CreateDraftInput,
    ) -> Result<(DraftSnapshot, Vec<AttachmentReference>), MailError>;
    fn update_draft(
        &self,
        resource: &kura_integrations::Resource,
        account: &AccountProjection,
        input: &UpdateDraftInput,
    ) -> Result<(DraftSnapshot, Vec<AttachmentReference>), MailError>;
    fn send_message(
        &self,
        resource: &kura_integrations::Resource,
        account: &AccountProjection,
        input: &SendMessageInput,
    ) -> Result<(MessageSnapshot, Vec<AttachmentReference>), MailError>;
    fn send_draft(
        &self,
        resource: &kura_integrations::Resource,
        account: &AccountProjection,
        input: &SendDraftInput,
    ) -> Result<(DraftSnapshot, MessageSnapshot, Vec<AttachmentReference>), MailError>;
    fn reply_message(
        &self,
        resource: &kura_integrations::Resource,
        account: &AccountProjection,
        input: &ReplyMessageInput,
    ) -> Result<
        (
            Option<DraftSnapshot>,
            Option<MessageSnapshot>,
            Vec<AttachmentReference>,
        ),
        MailError,
    >;
    fn forward_message(
        &self,
        resource: &kura_integrations::Resource,
        account: &AccountProjection,
        input: &ForwardMessageInput,
    ) -> Result<
        (
            Option<DraftSnapshot>,
            Option<MessageSnapshot>,
            Vec<AttachmentReference>,
        ),
        MailError,
    >;
    fn resolve_attachments(
        &self,
        resource: &kura_integrations::Resource,
        account: &AccountProjection,
        refs: &[AttachmentRefInput],
        parent_kind: &str,
        parent_id: &str,
    ) -> Vec<AttachmentReference>;
    fn download_attachment(
        &self,
        resource: &kura_integrations::Resource,
        account: &AccountProjection,
        input: &DownloadAttachmentInput,
    ) -> Result<AttachmentReference, MailError>;
    fn restore_integration_state(
        &self,
        integration_id: &str,
        threads: Vec<ThreadSnapshot>,
        messages: Vec<MessageSnapshot>,
        drafts: Vec<DraftSnapshot>,
        attachments: Vec<AttachmentReference>,
    );
}

#[must_use]
fn is_false(v: &bool) -> bool {
    !*v
}

#[must_use]
pub fn first_non_empty(values: &[&str]) -> String {
    for value in values {
        let trimmed = value.trim();
        if !trimmed.is_empty() {
            return trimmed.to_string();
        }
    }
    String::new()
}

#[must_use]
pub fn join_recipients(parts: &[&[String]]) -> Vec<String> {
    let mut out = Vec::new();
    for group in parts {
        for item in *group {
            let trimmed = item.trim();
            if !trimmed.is_empty() {
                out.push(trimmed.to_string());
            }
        }
    }
    out
}

#[must_use]
pub fn clone_threads(items: &[ThreadSnapshot]) -> Vec<ThreadSnapshot> {
    items.to_vec()
}

#[must_use]
pub fn clone_drafts(items: &[DraftSnapshot]) -> Vec<DraftSnapshot> {
    items.to_vec()
}

#[must_use]
pub fn attachment_refs_from_ids(ids: &[String]) -> Vec<AttachmentRefInput> {
    if ids.is_empty() {
        return Vec::new();
    }
    ids.iter()
        .map(|id| AttachmentRefInput {
            attachment_ref_id: id.clone(),
            ..AttachmentRefInput::default()
        })
        .collect()
}

#[must_use]
pub fn validate_explicit_recipients(recipients: &[String]) -> Result<(), MailError> {
    if join_recipients(&[recipients]).is_empty() {
        return Err(MailError::MailRecipientRequired);
    }
    Ok(())
}

#[must_use]
pub fn is_background_send(source: &SourceLinkage) -> bool {
    !source.workflow_id.trim().is_empty()
        || !source.schedule_id.trim().is_empty()
        || !source.schedule_attempt_id.trim().is_empty()
}

#[must_use]
pub fn collect_artifact_ids(artifacts: &[Artifact]) -> Vec<String> {
    if artifacts.is_empty() {
        return Vec::new();
    }
    artifacts.iter().map(|a| a.artifact_id.clone()).collect()
}

#[must_use]
pub fn summarize_list_threads(input: &ListThreadsInput) -> String {
    if input.limit > 0 {
        format!("limit={}", input.limit)
    } else {
        String::new()
    }
}

#[must_use]
pub fn summarize_draft_input(
    subject: &str,
    to: &[String],
    cc: &[String],
    bcc: &[String],
) -> String {
    let recipients = join_recipients(&[to, cc, bcc]);
    if recipients.is_empty() {
        return subject.trim().to_string();
    }
    format!("{} -> {}", subject.trim(), recipients.join(", "))
}

#[must_use]
pub fn new_operation_id() -> String {
    new_id("mail_op")
}

#[must_use]
fn new_id(prefix: &str) -> String {
    let hex = Uuid::new_v4().simple().to_string();
    format!("{prefix}_{}", &hex[..12])
}

#[must_use]
fn choose_attachment_message_id(item: &AttachmentReference) -> String {
    if item.parent_kind == "message" {
        item.parent_id.clone()
    } else {
        String::new()
    }
}

#[must_use]
fn choose_attachment_draft_id(item: &AttachmentReference) -> String {
    if item.parent_kind == "draft" {
        item.parent_id.clone()
    } else {
        String::new()
    }
}

#[must_use]
pub fn thread_artifact(operation: &Operation, item: &ThreadSnapshot) -> Artifact {
    Artifact {
        artifact_id: new_id("mail_artifact"),
        operation_id: operation.operation_id.clone(),
        kind: ArtifactKind::ThreadSnapshot,
        integration_id: operation.integration_id.clone(),
        environment_scope: operation.environment_scope.clone(),
        thread_id: item.thread_id.clone(),
        thread: Some(item.clone()),
        created_at: Utc::now(),
        ..Artifact::default()
    }
}

#[must_use]
pub fn message_artifact(operation: &Operation, item: &MessageSnapshot) -> Artifact {
    Artifact {
        artifact_id: new_id("mail_artifact"),
        operation_id: operation.operation_id.clone(),
        kind: ArtifactKind::MessageSnapshot,
        integration_id: operation.integration_id.clone(),
        environment_scope: operation.environment_scope.clone(),
        thread_id: item.thread_id.clone(),
        message_id: item.message_id.clone(),
        message: Some(item.clone()),
        created_at: Utc::now(),
        ..Artifact::default()
    }
}

#[must_use]
pub fn draft_artifact(operation: &Operation, item: &DraftSnapshot) -> Artifact {
    Artifact {
        artifact_id: new_id("mail_artifact"),
        operation_id: operation.operation_id.clone(),
        kind: ArtifactKind::DraftSnapshot,
        integration_id: operation.integration_id.clone(),
        environment_scope: operation.environment_scope.clone(),
        thread_id: item.thread_id.clone(),
        draft_id: item.draft_id.clone(),
        draft: Some(item.clone()),
        created_at: Utc::now(),
        ..Artifact::default()
    }
}

#[must_use]
pub fn attachment_artifact(operation: &Operation, item: &AttachmentReference) -> Artifact {
    Artifact {
        artifact_id: new_id("mail_artifact"),
        operation_id: operation.operation_id.clone(),
        kind: ArtifactKind::AttachmentRef,
        integration_id: operation.integration_id.clone(),
        environment_scope: operation.environment_scope.clone(),
        message_id: choose_attachment_message_id(item),
        draft_id: choose_attachment_draft_id(item),
        attachment_ref_id: item.attachment_ref_id.clone(),
        attachment: Some(item.clone()),
        created_at: Utc::now(),
        ..Artifact::default()
    }
}

#[must_use]
pub fn live_validation_matrix_rows() -> Vec<kura_livevalidation::MatrixRow> {
    let classes = [
        kura_livevalidation::ToolClass::from(kura_livevalidation::ToolClass::MAIL_DRAFT_CREATE),
        kura_livevalidation::ToolClass::from(kura_livevalidation::ToolClass::MAIL_DRAFT_UPDATE),
        kura_livevalidation::ToolClass::from(kura_livevalidation::ToolClass::MAIL_SEND),
        kura_livevalidation::ToolClass::from(kura_livevalidation::ToolClass::MAIL_REPLY),
        kura_livevalidation::ToolClass::from(kura_livevalidation::ToolClass::MAIL_FORWARD),
    ];
    let mut rows = Vec::new();
    for tool_class in classes {
        if let Some(row) = kura_livevalidation::default_matrix_row(&tool_class) {
            rows.push(row);
        }
    }
    rows
}

mod adapter_backend;
mod fake_backend;
mod manager;
pub use adapter_backend::*;
pub use fake_backend::*;
pub use manager::*;

// Attachment transfer policy (port of attachments_policy.go, Roadmap 64).

pub const MAX_ATTACHMENT_BYTES: i64 = 25 * 1024 * 1024;
pub const RETENTION_CLASS_STANDARD: &str = "standard";

const BLOCKED_MEDIA_TYPES: &[&str] = &[
    "application/x-msdownload",
    "application/x-dosexec",
    "application/x-executable",
    "application/vnd.microsoft.portable-executable",
    "application/x-sh",
    "application/x-bat",
    "application/x-msdos-program",
];

const BLOCKED_EXTENSIONS: &[&str] = &[
    ".exe", ".bat", ".cmd", ".com", ".scr", ".sh", ".dll", ".msi", ".ps1", ".js",
];

#[derive(Debug, Clone, Default, PartialEq)]
pub struct AttachmentPolicyResult {
    pub status: AttachmentResolutionStatus,
    pub retention_class: String,
    pub redacted: bool,
    pub failure_reason: String,
}

#[must_use]
pub fn evaluate_attachment(
    display_name: &str,
    media_type: &str,
    size_bytes: i64,
) -> AttachmentPolicyResult {
    if size_bytes > MAX_ATTACHMENT_BYTES {
        return AttachmentPolicyResult {
            status: AttachmentResolutionStatus::Failed,
            failure_reason: format!(
                "too_large: attachment is {size_bytes} bytes (limit {MAX_ATTACHMENT_BYTES})"
            ),
            ..AttachmentPolicyResult::default()
        };
    }
    if is_blocked_attachment(display_name, media_type) {
        return AttachmentPolicyResult {
            status: AttachmentResolutionStatus::Failed,
            failure_reason: "unsupported_type: attachment media type is not permitted for transfer"
                .to_string(),
            ..AttachmentPolicyResult::default()
        };
    }
    AttachmentPolicyResult {
        status: AttachmentResolutionStatus::Resolved,
        retention_class: RETENTION_CLASS_STANDARD.to_string(),
        redacted: false,
        ..AttachmentPolicyResult::default()
    }
}

#[must_use]
fn is_blocked_attachment(display_name: &str, media_type: &str) -> bool {
    let mt = media_type.trim().to_lowercase();
    if BLOCKED_MEDIA_TYPES.iter().any(|&x| x == mt) {
        return true;
    }
    let ext = extension(display_name).to_lowercase();
    BLOCKED_EXTENSIONS.iter().any(|&x| x == ext)
}

#[must_use]
fn extension(name: &str) -> String {
    match name.rfind('.') {
        Some(i) => name[i..].to_string(),
        None => String::new(),
    }
}

pub fn apply_attachment_policy(reference: &mut AttachmentReference) {
    let result = evaluate_attachment(
        &reference.display_name,
        &reference.media_type,
        reference.size_bytes.unwrap_or(0),
    );
    reference.resolution_status = result.status;
    reference.retention_class = result.retention_class;
    reference.redacted = result.redacted;
    if !result.failure_reason.is_empty() {
        reference.failure_reason = result.failure_reason;
    }
}
