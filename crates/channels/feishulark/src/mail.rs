//! Feishu/Lark mail provider (port of mail.go): maps the Feishu Open Platform Mail API onto
//! the mail domain resources. Stateless; the daemon owns the operation ledger.

use std::time::Duration;

use chrono::{DateTime, Utc};
use kura_adapterprovider::{Handler, HandlerError, Operation};
use kura_integrations::{ReadinessStatus, Resource};
use kura_mail::{
    AccountProjection, AttachmentRefInput, AttachmentReference, AttachmentResolutionStatus,
    ComposeMode, CreateDraftInput, DeliveryState, Direction, DownloadAttachmentInput,
    DraftSnapshot, DraftStatus, ForwardMessageInput, ListDraftsInput, ListThreadsInput,
    MessageSnapshot, ReplyForwardResultMode, ReplyMessageInput, SendDraftInput, SendMessageInput,
    ThreadSnapshot, UpdateDraftInput, apply_attachment_policy,
};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use serde_json::value::RawValue;

use crate::{Client, FaultKind, ProviderFault, ScopedToken, first_non_empty, parse_token};

pub struct MailProvider {
    client: Client,
}

pub fn new_mail_provider(client: Client) -> MailProvider {
    MailProvider { client }
}

impl Handler for MailProvider {
    fn handle(
        &self,
        op: Operation,
        deadline: Option<Duration>,
    ) -> Result<Option<Box<RawValue>>, HandlerError> {
        if op.domain != "mail" {
            return Err(HandlerError::Fault(
                ProviderFault {
                    kind: FaultKind::Internal,
                    code: "unsupported_domain".to_string(),
                    message: "adapter serves the mail domain only".to_string(),
                }
                .to_adapter_fault(),
            ));
        }
        let raw_cred = op
            .credential
            .as_deref()
            .map(|r| r.get().as_bytes())
            .unwrap_or(&[]);
        let token = parse_token(raw_cred).map_err(|f| HandlerError::Fault(f.to_adapter_fault()))?;
        let resource: Resource = op
            .resource
            .as_deref()
            .map(|r| serde_json::from_str(r.get()).unwrap_or_default())
            .unwrap_or_default();
        match self.route(&token, &resource, &op, deadline) {
            Ok(raw) => Ok(Some(raw)),
            Err(pf) if pf.is_ambiguous() => Err(HandlerError::Ambiguous),
            Err(pf) => Err(HandlerError::Fault(pf.to_adapter_fault())),
        }
    }
}

impl MailProvider {
    fn route(
        &self,
        token: &ScopedToken,
        resource: &Resource,
        op: &Operation,
        deadline: Option<Duration>,
    ) -> Result<Box<RawValue>, ProviderFault> {
        let payload = op.payload.as_deref();
        match op.operation.as_str() {
            "ProjectAccount" => marshal_result(self.project_account(token, resource, deadline)?),
            "ListThreads" => {
                let input = decode_payload::<ListThreadsPayload>(payload)?;
                marshal_result(self.list_threads(token, &input.account, &input.input, deadline)?)
            }
            "GetThread" => {
                let input = decode_payload::<IdPayload>(payload)?;
                marshal_result(self.get_thread(token, &input.account, &input.id, deadline)?)
            }
            "GetMessage" => {
                let input = decode_payload::<IdPayload>(payload)?;
                marshal_result(self.get_message(token, &input.account, &input.id, deadline)?)
            }
            "ListDrafts" => {
                let input = decode_payload::<ListDraftsPayload>(payload)?;
                marshal_result(self.list_drafts(token, &input.account, deadline)?)
            }
            "GetDraft" => {
                let input = decode_payload::<IdPayload>(payload)?;
                marshal_result(self.get_draft(token, &input.account, &input.id, deadline)?)
            }
            "CreateDraft" => {
                let input = decode_payload::<CreateDraftPayload>(payload)?;
                marshal_result(self.create_draft(token, &input.account, &input.input, deadline)?)
            }
            "UpdateDraft" => {
                let input = decode_payload::<UpdateDraftPayload>(payload)?;
                marshal_result(self.update_draft(token, &input.account, &input.input, deadline)?)
            }
            "SendMessage" => {
                let input = decode_payload::<SendMessagePayload>(payload)?;
                marshal_result(self.send_message(token, &input.account, &input.input, deadline)?)
            }
            "SendDraft" => {
                let input = decode_payload::<SendDraftPayload>(payload)?;
                marshal_result(self.send_draft(token, &input.account, &input.input, deadline)?)
            }
            "ReplyMessage" => {
                let input = decode_payload::<ReplyMessagePayload>(payload)?;
                marshal_result(self.reply_message(token, &input.account, &input.input, deadline)?)
            }
            "ForwardMessage" => {
                let input = decode_payload::<ForwardMessagePayload>(payload)?;
                marshal_result(self.forward_message(
                    token,
                    &input.account,
                    &input.input,
                    deadline,
                )?)
            }
            "ResolveAttachments" => {
                let input = decode_payload::<ResolveAttachmentsPayload>(payload)?;
                marshal_result(resolve_with_policy(
                    &input.refs,
                    &input.parent_kind,
                    &input.parent_id,
                ))
            }
            "DownloadAttachment" => {
                let input = decode_payload::<DownloadAttachmentPayload>(payload)?;
                marshal_result(self.download_attachment(
                    token,
                    &input.account,
                    &input.input,
                    deadline,
                )?)
            }
            _ => Err(ProviderFault {
                kind: FaultKind::Internal,
                code: "unsupported_operation".to_string(),
                message: "unsupported mail operation".to_string(),
            }),
        }
    }

    fn project_account(
        &self,
        token: &ScopedToken,
        resource: &Resource,
        deadline: Option<Duration>,
    ) -> Result<AccountProjection, ProviderFault> {
        let mut out = MailboxResp::default();
        self.client.call(
            deadline,
            "GET",
            "/open-apis/mail/v1/user_mailboxes/primary",
            &token.access_token,
            None::<&Value>,
            Some(&mut out),
            false,
        )?;
        let account_key = resource
            .account_binding
            .as_ref()
            .map(|b| b.account_key.clone())
            .unwrap_or_default();
        let external_id = resource
            .account_binding
            .as_ref()
            .map(|b| b.external_account_id.clone())
            .unwrap_or_default();
        let account_label = resource
            .account_binding
            .as_ref()
            .map(|b| b.account_label.clone())
            .unwrap_or_default();
        let address = first_non_empty(&[&out.mailbox_address, &account_key, &external_id]);
        let now = Utc::now();
        Ok(AccountProjection {
            mail_account_id: format!("fl_{address}"),
            integration_id: resource.integration_id.clone(),
            domain_kind: "mail".to_string(),
            environment_scope: resource.environment_scope.clone(),
            account_key: address.clone(),
            account_label: first_non_empty(&[&out.name, &account_label]),
            readiness_status: ReadinessStatus::Healthy.as_str().to_string(),
            canonical_default: resource.canonical_default,
            mailbox_address: address.clone(),
            mailbox_label: first_non_empty(&[&out.name, &address]),
            supports_thread_inspection: true,
            supports_drafts: true,
            supports_direct_send: true,
            supports_reply: true,
            supports_forward: true,
            last_synced_at: now,
            updated_at: now,
            ..AccountProjection::default()
        })
    }

    fn list_threads(
        &self,
        token: &ScopedToken,
        account: &AccountProjection,
        input: &ListThreadsInput,
        deadline: Option<Duration>,
    ) -> Result<Vec<ThreadSnapshot>, ProviderFault> {
        let mut query: Vec<String> = Vec::new();
        if input.limit > 0 {
            query.push(format!("page_size={}", input.limit));
        }
        if !input.cursor.trim().is_empty() {
            query.push(format!("page_token={}", input.cursor));
        }
        let mut path = format!(
            "/open-apis/mail/v1/user_mailboxes/{}/threads",
            account.mailbox_address
        );
        if !query.is_empty() {
            path.push('?');
            path.push_str(&query.join("&"));
        }
        let mut out = MailThreadItems::default();
        self.client.call(
            deadline,
            "GET",
            &path,
            &token.access_token,
            None::<&Value>,
            Some(&mut out),
            false,
        )?;
        Ok(out
            .items
            .iter()
            .map(|item| map_thread(account, item))
            .collect())
    }

    fn get_thread(
        &self,
        token: &ScopedToken,
        account: &AccountProjection,
        thread_id: &str,
        deadline: Option<Duration>,
    ) -> Result<ThreadSnapshot, ProviderFault> {
        let path = format!(
            "/open-apis/mail/v1/user_mailboxes/{}/threads/{}",
            account.mailbox_address, thread_id
        );
        let mut out = MailThreadResp::default();
        self.client.call(
            deadline,
            "GET",
            &path,
            &token.access_token,
            None::<&Value>,
            Some(&mut out),
            false,
        )?;
        Ok(map_thread(account, &out.thread))
    }

    fn get_message(
        &self,
        token: &ScopedToken,
        account: &AccountProjection,
        message_id: &str,
        deadline: Option<Duration>,
    ) -> Result<MessageSnapshot, ProviderFault> {
        let path = format!(
            "/open-apis/mail/v1/user_mailboxes/{}/messages/{}",
            account.mailbox_address, message_id
        );
        let mut out = MailMessageResp::default();
        self.client.call(
            deadline,
            "GET",
            &path,
            &token.access_token,
            None::<&Value>,
            Some(&mut out),
            false,
        )?;
        Ok(map_message(account, &out.message, Direction::Inbound))
    }

    fn list_drafts(
        &self,
        token: &ScopedToken,
        account: &AccountProjection,
        deadline: Option<Duration>,
    ) -> Result<Vec<DraftSnapshot>, ProviderFault> {
        let path = format!(
            "/open-apis/mail/v1/user_mailboxes/{}/drafts",
            account.mailbox_address
        );
        let mut out = MailDraftItems::default();
        self.client.call(
            deadline,
            "GET",
            &path,
            &token.access_token,
            None::<&Value>,
            Some(&mut out),
            false,
        )?;
        Ok(out
            .items
            .iter()
            .map(|item| map_draft(account, item, ComposeMode::NewMessage))
            .collect())
    }

    fn get_draft(
        &self,
        token: &ScopedToken,
        account: &AccountProjection,
        draft_id: &str,
        deadline: Option<Duration>,
    ) -> Result<DraftSnapshot, ProviderFault> {
        let path = format!(
            "/open-apis/mail/v1/user_mailboxes/{}/drafts/{}",
            account.mailbox_address, draft_id
        );
        let mut out = MailDraftResp::default();
        self.client.call(
            deadline,
            "GET",
            &path,
            &token.access_token,
            None::<&Value>,
            Some(&mut out),
            false,
        )?;
        Ok(map_draft(account, &out.draft, ComposeMode::NewMessage))
    }

    fn create_draft(
        &self,
        token: &ScopedToken,
        account: &AccountProjection,
        input: &CreateDraftInput,
        deadline: Option<Duration>,
    ) -> Result<DraftWithAttachments, ProviderFault> {
        let mut body = serde_json::json!({ "subject": input.subject, "body": input.body, "to": input.to, "cc": input.cc, "bcc": input.bcc });
        if !input.thread_id.trim().is_empty() {
            body["thread_id"] = Value::String(input.thread_id.clone());
        }
        let path = format!(
            "/open-apis/mail/v1/user_mailboxes/{}/drafts",
            account.mailbox_address
        );
        let mut out = MailDraftResp::default();
        self.client.call(
            deadline,
            "POST",
            &path,
            &token.access_token,
            Some(&body),
            Some(&mut out),
            true,
        )?;
        let mut draft = map_draft(account, &out.draft, input.compose_mode);
        draft.thread_id = first_non_empty(&[&draft.thread_id, &input.thread_id]);
        let attachments = resolve_with_policy(&input.attachment_refs, "draft", &draft.draft_id);
        Ok(DraftWithAttachments { draft, attachments })
    }

    fn update_draft(
        &self,
        token: &ScopedToken,
        account: &AccountProjection,
        input: &UpdateDraftInput,
        deadline: Option<Duration>,
    ) -> Result<DraftWithAttachments, ProviderFault> {
        let body = serde_json::json!({ "subject": input.subject, "body": input.body, "to": input.to, "cc": input.cc, "bcc": input.bcc });
        let path = format!(
            "/open-apis/mail/v1/user_mailboxes/{}/drafts/{}",
            account.mailbox_address, input.draft_id
        );
        let mut out = MailDraftResp::default();
        self.client.call(
            deadline,
            "PATCH",
            &path,
            &token.access_token,
            Some(&body),
            Some(&mut out),
            true,
        )?;
        let mut draft = map_draft(account, &out.draft, ComposeMode::NewMessage);
        if draft.draft_id.is_empty() {
            draft.draft_id = input.draft_id.clone();
        }
        draft.draft_status = DraftStatus::Updated;
        let attachments = resolve_with_policy(&input.attachment_refs, "draft", &draft.draft_id);
        Ok(DraftWithAttachments { draft, attachments })
    }

    fn send_message(
        &self,
        token: &ScopedToken,
        account: &AccountProjection,
        input: &SendMessageInput,
        deadline: Option<Duration>,
    ) -> Result<MessageWithAttachments, ProviderFault> {
        let body = serde_json::json!({ "subject": input.subject, "body": input.body, "to": input.to, "cc": input.cc, "bcc": input.bcc });
        let path = format!(
            "/open-apis/mail/v1/user_mailboxes/{}/messages/send",
            account.mailbox_address
        );
        let mut out = MailMessageResp::default();
        self.client.call(
            deadline,
            "POST",
            &path,
            &token.access_token,
            Some(&body),
            Some(&mut out),
            true,
        )?;
        let message_id = out.message.message_id.clone();
        let attachments = resolve_with_policy(&input.attachment_refs, "message", &message_id);
        Ok(MessageWithAttachments {
            message: map_message(account, &out.message, Direction::Outbound),
            attachments,
        })
    }

    fn send_draft(
        &self,
        token: &ScopedToken,
        account: &AccountProjection,
        input: &SendDraftInput,
        deadline: Option<Duration>,
    ) -> Result<SendDraftResult, ProviderFault> {
        let path = format!(
            "/open-apis/mail/v1/user_mailboxes/{}/drafts/{}/send",
            account.mailbox_address, input.draft_id
        );
        let mut out = SendDraftResp::default();
        self.client.call(
            deadline,
            "POST",
            &path,
            &token.access_token,
            Some(&Value::Object(Default::default())),
            Some(&mut out),
            true,
        )?;
        let mut draft = map_draft(account, &out.draft, ComposeMode::NewMessage);
        if draft.draft_id.is_empty() {
            draft.draft_id = input.draft_id.clone();
        }
        draft.draft_status = DraftStatus::SentFromDraft;
        Ok(SendDraftResult {
            draft,
            message: map_message(account, &out.message, Direction::Outbound),
            attachments: Vec::new(),
        })
    }

    fn reply_message(
        &self,
        token: &ScopedToken,
        account: &AccountProjection,
        input: &ReplyMessageInput,
        deadline: Option<Duration>,
    ) -> Result<OptionalDraftMessage, ProviderFault> {
        let body = serde_json::json!({ "subject": input.subject, "body": input.body });
        let path = format!(
            "/open-apis/mail/v1/user_mailboxes/{}/messages/{}/reply",
            account.mailbox_address, input.message_id
        );
        if input.result_mode == ReplyForwardResultMode::Draft {
            let mut out = MailDraftResp::default();
            self.client.call(
                deadline,
                "POST",
                &format!("{path}?as_draft=true"),
                &token.access_token,
                Some(&body),
                Some(&mut out),
                true,
            )?;
            let mut draft = map_draft(account, &out.draft, ComposeMode::Reply);
            draft.source_message_id = input.message_id.clone();
            return Ok(OptionalDraftMessage {
                draft: Some(draft),
                message: None,
                attachments: Vec::new(),
            });
        }
        let mut out = MailMessageResp::default();
        self.client.call(
            deadline,
            "POST",
            &path,
            &token.access_token,
            Some(&body),
            Some(&mut out),
            true,
        )?;
        let mut message = map_message(account, &out.message, Direction::Outbound);
        if message.reply_to_message_id.is_empty() {
            message.reply_to_message_id = input.message_id.clone();
        }
        Ok(OptionalDraftMessage {
            draft: None,
            message: Some(message),
            attachments: Vec::new(),
        })
    }

    fn forward_message(
        &self,
        token: &ScopedToken,
        account: &AccountProjection,
        input: &ForwardMessageInput,
        deadline: Option<Duration>,
    ) -> Result<OptionalDraftMessage, ProviderFault> {
        let body = serde_json::json!({ "subject": input.subject, "body": input.body, "to": input.to, "cc": input.cc, "bcc": input.bcc });
        let path = format!(
            "/open-apis/mail/v1/user_mailboxes/{}/messages/{}/forward",
            account.mailbox_address, input.message_id
        );
        if input.result_mode == ReplyForwardResultMode::Draft {
            let mut out = MailDraftResp::default();
            self.client.call(
                deadline,
                "POST",
                &format!("{path}?as_draft=true"),
                &token.access_token,
                Some(&body),
                Some(&mut out),
                true,
            )?;
            let mut draft = map_draft(account, &out.draft, ComposeMode::Forward);
            draft.source_message_id = input.message_id.clone();
            return Ok(OptionalDraftMessage {
                draft: Some(draft),
                message: None,
                attachments: Vec::new(),
            });
        }
        let mut out = MailMessageResp::default();
        self.client.call(
            deadline,
            "POST",
            &path,
            &token.access_token,
            Some(&body),
            Some(&mut out),
            true,
        )?;
        let mut message = map_message(account, &out.message, Direction::Outbound);
        if message.forwarded_from_message_id.is_empty() {
            message.forwarded_from_message_id = input.message_id.clone();
        }
        Ok(OptionalDraftMessage {
            draft: None,
            message: Some(message),
            attachments: Vec::new(),
        })
    }

    fn download_attachment(
        &self,
        token: &ScopedToken,
        account: &AccountProjection,
        input: &DownloadAttachmentInput,
        deadline: Option<Duration>,
    ) -> Result<AttachmentReference, ProviderFault> {
        let path = format!(
            "/open-apis/mail/v1/user_mailboxes/{}/messages/{}/attachments/{}",
            account.mailbox_address, input.message_id, input.attachment_ref_id
        );
        let mut out = MailAttachmentResp::default();
        self.client.call(
            deadline,
            "GET",
            &path,
            &token.access_token,
            None::<&Value>,
            Some(&mut out),
            false,
        )?;
        let mut reference = AttachmentReference {
            attachment_ref_id: input.attachment_ref_id.clone(),
            integration_id: account.integration_id.clone(),
            parent_kind: "message".to_string(),
            parent_id: input.message_id.clone(),
            display_name: first_non_empty(&[
                &out.display_name,
                &input.display_name,
                "attachment.bin",
            ]),
            media_type: first_non_empty(&[&out.media_type, &input.media_type]),
            size_bytes: Some(non_zero(out.size_bytes, input.size_bytes)),
            resolution_status: AttachmentResolutionStatus::Resolved,
            created_at: Utc::now(),
            ..AttachmentReference::default()
        };
        apply_attachment_policy(&mut reference);
        if reference.resolution_status == AttachmentResolutionStatus::Resolved {
            reference.downloaded = true;
        }
        Ok(reference)
    }
}

// ---- payload shapes ----

#[derive(Debug, Default, Deserialize)]
struct ListThreadsPayload {
    account: AccountProjection,
    input: ListThreadsInput,
}

#[derive(Debug, Default, Deserialize)]
struct IdPayload {
    account: AccountProjection,
    #[serde(rename = "threadId", default)]
    id: String,
}

#[derive(Debug, Default, Deserialize)]
#[allow(dead_code)]
struct ListDraftsPayload {
    account: AccountProjection,
    input: ListDraftsInput,
}

#[derive(Debug, Default, Deserialize)]
struct CreateDraftPayload {
    account: AccountProjection,
    input: CreateDraftInput,
}

#[derive(Debug, Default, Deserialize)]
struct UpdateDraftPayload {
    account: AccountProjection,
    input: UpdateDraftInput,
}

#[derive(Debug, Default, Deserialize)]
struct SendMessagePayload {
    account: AccountProjection,
    input: SendMessageInput,
}

#[derive(Debug, Default, Deserialize)]
struct SendDraftPayload {
    account: AccountProjection,
    input: SendDraftInput,
}

#[derive(Debug, Default, Deserialize)]
struct ReplyMessagePayload {
    account: AccountProjection,
    input: ReplyMessageInput,
}

#[derive(Debug, Default, Deserialize)]
struct ForwardMessagePayload {
    account: AccountProjection,
    input: ForwardMessageInput,
}

#[derive(Debug, Default, Deserialize)]
#[allow(dead_code)]
struct ResolveAttachmentsPayload {
    account: AccountProjection,
    #[serde(default)]
    refs: Vec<AttachmentRefInput>,
    #[serde(rename = "parentKind", default)]
    parent_kind: String,
    #[serde(rename = "parentId", default)]
    parent_id: String,
}

#[derive(Debug, Default, Deserialize)]
struct DownloadAttachmentPayload {
    account: AccountProjection,
    input: DownloadAttachmentInput,
}

// ---- Feishu response shapes ----

#[derive(Debug, Default, Deserialize)]
struct MailboxResp {
    #[serde(rename = "mailbox_address", default)]
    mailbox_address: String,
    #[serde(default)]
    name: String,
}

#[derive(Debug, Default, Deserialize)]
struct FeishuMailThread {
    #[serde(rename = "thread_id", default)]
    thread_id: String,
    #[serde(default)]
    subject: String,
    #[serde(default)]
    participants: Vec<String>,
    #[serde(rename = "message_ids", default)]
    message_ids: Vec<String>,
    #[serde(rename = "latest_message_time", default)]
    latest_message_at: i64,
    #[serde(rename = "message_count", default)]
    message_count: i64,
}

#[derive(Debug, Default, Deserialize)]
struct FeishuMailMessage {
    #[serde(rename = "message_id", default)]
    message_id: String,
    #[serde(rename = "thread_id", default)]
    thread_id: String,
    #[serde(default)]
    subject: String,
    #[serde(default)]
    from: String,
    #[serde(default)]
    to: Vec<String>,
    #[serde(rename = "body_preview", default)]
    body_preview: String,
    #[serde(rename = "in_reply_to", default)]
    reply_to: String,
    #[serde(rename = "forwarded_from", default)]
    forwarded_from: String,
    #[serde(rename = "sent_time", default)]
    sent_at: i64,
}

#[derive(Debug, Default, Deserialize)]
struct FeishuMailDraft {
    #[serde(rename = "draft_id", default)]
    draft_id: String,
    #[serde(rename = "thread_id", default)]
    thread_id: String,
    #[serde(default)]
    subject: String,
    #[serde(default)]
    to: Vec<String>,
    #[serde(rename = "body_preview", default)]
    body_preview: String,
}

#[derive(Debug, Default, Deserialize)]
struct MailThreadItems {
    #[serde(default)]
    items: Vec<FeishuMailThread>,
}

#[derive(Debug, Default, Deserialize)]
struct MailThreadResp {
    #[serde(default)]
    thread: FeishuMailThread,
}

#[derive(Debug, Default, Deserialize)]
struct MailMessageResp {
    #[serde(default)]
    message: FeishuMailMessage,
}

#[derive(Debug, Default, Deserialize)]
struct MailDraftItems {
    #[serde(default)]
    items: Vec<FeishuMailDraft>,
}

#[derive(Debug, Default, Deserialize)]
struct MailDraftResp {
    #[serde(default)]
    draft: FeishuMailDraft,
}

#[derive(Debug, Default, Deserialize)]
struct SendDraftResp {
    #[serde(default)]
    draft: FeishuMailDraft,
    #[serde(default)]
    message: FeishuMailMessage,
}

#[derive(Debug, Default, Deserialize)]
struct MailAttachmentResp {
    #[serde(rename = "display_name", default)]
    display_name: String,
    #[serde(rename = "media_type", default)]
    media_type: String,
    #[serde(rename = "size_bytes", default)]
    size_bytes: i64,
}

// ---- result shapes ----

#[derive(Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
struct DraftWithAttachments {
    draft: DraftSnapshot,
    attachments: Vec<AttachmentReference>,
}

#[derive(Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
struct MessageWithAttachments {
    message: MessageSnapshot,
    attachments: Vec<AttachmentReference>,
}

#[derive(Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
struct SendDraftResult {
    draft: DraftSnapshot,
    message: MessageSnapshot,
    #[serde(default)]
    attachments: Vec<AttachmentReference>,
}

#[derive(Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
struct OptionalDraftMessage {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    draft: Option<DraftSnapshot>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    message: Option<MessageSnapshot>,
    #[serde(default)]
    attachments: Vec<AttachmentReference>,
}

// ---- mapping helpers ----

fn map_thread(account: &AccountProjection, item: &FeishuMailThread) -> ThreadSnapshot {
    ThreadSnapshot {
        thread_id: item.thread_id.clone(),
        integration_id: account.integration_id.clone(),
        mail_account_id: account.mail_account_id.clone(),
        subject: item.subject.clone(),
        participant_summary: item.participants.clone(),
        message_ids: item.message_ids.clone(),
        latest_message_at: DateTime::from_timestamp(item.latest_message_at, 0).unwrap_or_default(),
        message_count: item.message_count,
        created_at: Utc::now(),
        ..ThreadSnapshot::default()
    }
}

fn map_message(
    account: &AccountProjection,
    item: &FeishuMailMessage,
    direction: Direction,
) -> MessageSnapshot {
    let now = Utc::now();
    let mut message = MessageSnapshot {
        message_id: item.message_id.clone(),
        thread_id: item.thread_id.clone(),
        integration_id: account.integration_id.clone(),
        mail_account_id: account.mail_account_id.clone(),
        direction,
        sender_summary: item.from.clone(),
        recipient_summary: item.to.clone(),
        subject: item.subject.clone(),
        body_preview: item.body_preview.clone(),
        reply_to_message_id: item.reply_to.clone(),
        forwarded_from_message_id: item.forwarded_from.clone(),
        delivery_state: DeliveryState::Sent,
        created_at: now,
        ..MessageSnapshot::default()
    };
    if direction == Direction::Inbound {
        message.delivery_state = DeliveryState::Received;
    }
    if item.sent_at > 0 {
        message.sent_at = DateTime::from_timestamp(item.sent_at, 0);
    }
    message
}

fn map_draft(
    account: &AccountProjection,
    item: &FeishuMailDraft,
    mode: ComposeMode,
) -> DraftSnapshot {
    let now = Utc::now();
    DraftSnapshot {
        draft_id: item.draft_id.clone(),
        thread_id: item.thread_id.clone(),
        integration_id: account.integration_id.clone(),
        mail_account_id: account.mail_account_id.clone(),
        compose_mode: mode,
        recipient_summary: item.to.clone(),
        subject: item.subject.clone(),
        body_preview: item.body_preview.clone(),
        draft_status: DraftStatus::Draft,
        created_at: now,
        updated_at: now,
        ..DraftSnapshot::default()
    }
}

fn resolve_with_policy(
    refs: &[AttachmentRefInput],
    parent_kind: &str,
    parent_id: &str,
) -> Vec<AttachmentReference> {
    if refs.is_empty() {
        return Vec::new();
    }
    let now = Utc::now();
    let mut out = Vec::with_capacity(refs.len());
    for r in refs {
        let mut reference = AttachmentReference {
            attachment_ref_id: r.attachment_ref_id.clone(),
            parent_kind: parent_kind.to_string(),
            parent_id: parent_id.to_string(),
            display_name: r.display_name.clone(),
            media_type: r.media_type.clone(),
            size_bytes: r.size_bytes,
            resolution_status: AttachmentResolutionStatus::Resolved,
            created_at: now,
            ..AttachmentReference::default()
        };
        apply_attachment_policy(&mut reference);
        out.push(reference);
    }
    out
}

fn non_zero(a: i64, b: i64) -> i64 {
    if a != 0 { a } else { b }
}

fn decode_payload<T: DeserializeOwned>(payload: Option<&RawValue>) -> Result<T, ProviderFault> {
    let raw = payload.ok_or_else(|| ProviderFault {
        kind: FaultKind::Internal,
        code: "empty_payload".to_string(),
        message: "operation payload missing".to_string(),
    })?;
    serde_json::from_str(raw.get()).map_err(|_| ProviderFault {
        kind: FaultKind::Internal,
        code: "payload_decode_failed".to_string(),
        message: "operation payload unreadable".to_string(),
    })
}

fn marshal_result<T: Serialize>(value: T) -> Result<Box<RawValue>, ProviderFault> {
    serde_json::value::to_raw_value(&value).map_err(|_| ProviderFault {
        kind: FaultKind::Internal,
        code: "result_encode_failed".to_string(),
        message: "result encode failed".to_string(),
    })
}
