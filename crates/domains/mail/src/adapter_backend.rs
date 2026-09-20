//! Mail adapter backend (port of `adapter_backend.go`): dispatches each operation to an
//! out-of-process integration adapter over the capability RPC contract (Roadmap 59). It does
//! provider request/response mapping only; the Manager retains the operation ledger.

use std::time::Duration;

use kura_integrations::Resource;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use crate::{
    AccountProjection, AdapterFailure, AttachmentRefInput, AttachmentReference, Backend,
    CreateDraftInput, DownloadAttachmentInput, DraftSnapshot, ForwardMessageInput, ListDraftsInput,
    ListThreadsInput, MailError, MessageSnapshot, ReplyMessageInput, SendDraftInput,
    SendMessageInput, ThreadSnapshot, UpdateDraftInput,
};

const DOMAIN_MAIL: &str = "mail";

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DraftWithAttachments {
    #[serde(default)]
    draft: DraftSnapshot,
    #[serde(default)]
    attachments: Vec<AttachmentReference>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct MessageWithAttachments {
    #[serde(default)]
    message: MessageSnapshot,
    #[serde(default)]
    attachments: Vec<AttachmentReference>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SendDraftResult {
    #[serde(default)]
    draft: DraftSnapshot,
    #[serde(default)]
    message: MessageSnapshot,
    #[serde(default)]
    attachments: Vec<AttachmentReference>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct OptionalDraftMessage {
    #[serde(default)]
    draft: Option<DraftSnapshot>,
    #[serde(default)]
    message: Option<MessageSnapshot>,
    #[serde(default)]
    attachments: Vec<AttachmentReference>,
}

pub struct AdapterBackend {
    client: kura_adapterrpc::Client,
    deadline: Duration,
    provider_kind: String,
}

impl AdapterBackend {
    pub fn new(client: kura_adapterrpc::Client, deadline: Duration) -> Self {
        AdapterBackend {
            client,
            deadline,
            provider_kind: String::new(),
        }
    }

    pub fn with_provider_kind(mut self, kind: &str) -> Self {
        self.provider_kind = kind.to_string();
        self
    }

    #[must_use]
    pub fn provider_kind(&self) -> &str {
        &self.provider_kind
    }

    fn dispatch<R, P, O>(
        &self,
        operation: &str,
        resource: Option<&R>,
        payload: Option<&P>,
        out: Option<&mut O>,
    ) -> Result<(), MailError>
    where
        R: Serialize + ?Sized,
        P: Serialize + ?Sized,
        O: DeserializeOwned,
    {
        let result = if self.deadline.is_zero() {
            self.client
                .dispatch(DOMAIN_MAIL, operation, resource, payload, out)
        } else {
            self.client.dispatch_with_timeout(
                self.deadline,
                DOMAIN_MAIL,
                operation,
                resource,
                payload,
                out,
            )
        };
        self.map_err(result)
    }

    fn map_err(&self, err: Result<(), kura_adapterrpc::Error>) -> Result<(), MailError> {
        match err {
            Ok(()) => Ok(()),
            Err(e) => {
                if kura_adapterrpc::is_ambiguous(&e) {
                    return Err(MailError::Adapter(AdapterFailure {
                        class: "ambiguous_commit".to_string(),
                        provider_kind: self.provider_kind.clone(),
                        detail: e.to_string(),
                        ambiguous: true,
                        unavailable: false,
                    }));
                }
                if let kura_adapterrpc::Error::Adapter(ae) = &e {
                    return Err(MailError::Adapter(AdapterFailure {
                        class: stable_failure_class(ae),
                        provider_kind: self.provider_kind.clone(),
                        detail: ae.detail.clone(),
                        ambiguous: false,
                        unavailable: ae.kind == kura_adapterrpc::FailureKind::Unavailable,
                    }));
                }
                Err(MailError::AdapterTransport(e.to_string()))
            }
        }
    }
}

fn stable_failure_class(ae: &kura_adapterrpc::AdapterError) -> String {
    if !ae.detail.is_empty() {
        return ae.detail.clone();
    }
    match ae.kind {
        kura_adapterrpc::FailureKind::Auth => "user_access_token_invalid".to_string(),
        kura_adapterrpc::FailureKind::Scope => "scope_not_granted".to_string(),
        kura_adapterrpc::FailureKind::RateLimited => "rate_limited".to_string(),
        kura_adapterrpc::FailureKind::Unavailable => "service_unavailable".to_string(),
        kura_adapterrpc::FailureKind::Malformed => "malformed_provider_response".to_string(),
        _ => "provider_internal_error".to_string(),
    }
}

impl Backend for AdapterBackend {
    fn supports_resource(&self, resource: &Resource) -> bool {
        resource.domain_kind == "mail"
    }

    fn project_account(&self, resource: &Resource) -> Result<AccountProjection, MailError> {
        let mut out = AccountProjection::default();
        self.dispatch::<Resource, serde_json::Value, AccountProjection>(
            "ProjectAccount",
            Some(resource),
            None,
            Some(&mut out),
        )?;
        Ok(out)
    }

    fn list_threads(
        &self,
        resource: &Resource,
        account: &AccountProjection,
        input: &ListThreadsInput,
    ) -> Result<Vec<ThreadSnapshot>, MailError> {
        let mut out: Vec<ThreadSnapshot> = Vec::new();
        let payload = serde_json::json!({ "account": account, "input": input });
        self.dispatch::<Resource, serde_json::Value, Vec<ThreadSnapshot>>(
            "ListThreads",
            Some(resource),
            Some(&payload),
            Some(&mut out),
        )?;
        Ok(out)
    }

    fn get_thread(
        &self,
        resource: &Resource,
        account: &AccountProjection,
        thread_id: &str,
    ) -> Result<ThreadSnapshot, MailError> {
        let mut out = ThreadSnapshot::default();
        let payload = serde_json::json!({ "account": account, "threadId": thread_id });
        self.dispatch::<Resource, serde_json::Value, ThreadSnapshot>(
            "GetThread",
            Some(resource),
            Some(&payload),
            Some(&mut out),
        )?;
        Ok(out)
    }

    fn get_message(
        &self,
        resource: &Resource,
        account: &AccountProjection,
        message_id: &str,
    ) -> Result<MessageSnapshot, MailError> {
        let mut out = MessageSnapshot::default();
        let payload = serde_json::json!({ "account": account, "messageId": message_id });
        self.dispatch::<Resource, serde_json::Value, MessageSnapshot>(
            "GetMessage",
            Some(resource),
            Some(&payload),
            Some(&mut out),
        )?;
        Ok(out)
    }

    fn list_drafts(
        &self,
        resource: &Resource,
        account: &AccountProjection,
        input: &ListDraftsInput,
    ) -> Result<Vec<DraftSnapshot>, MailError> {
        let mut out: Vec<DraftSnapshot> = Vec::new();
        let payload = serde_json::json!({ "account": account, "input": input });
        self.dispatch::<Resource, serde_json::Value, Vec<DraftSnapshot>>(
            "ListDrafts",
            Some(resource),
            Some(&payload),
            Some(&mut out),
        )?;
        Ok(out)
    }

    fn get_draft(
        &self,
        resource: &Resource,
        account: &AccountProjection,
        draft_id: &str,
    ) -> Result<DraftSnapshot, MailError> {
        let mut out = DraftSnapshot::default();
        let payload = serde_json::json!({ "account": account, "draftId": draft_id });
        self.dispatch::<Resource, serde_json::Value, DraftSnapshot>(
            "GetDraft",
            Some(resource),
            Some(&payload),
            Some(&mut out),
        )?;
        Ok(out)
    }

    fn create_draft(
        &self,
        resource: &Resource,
        account: &AccountProjection,
        input: &CreateDraftInput,
    ) -> Result<(DraftSnapshot, Vec<AttachmentReference>), MailError> {
        let mut out = DraftWithAttachments::default();
        let payload = serde_json::json!({ "account": account, "input": input });
        self.dispatch::<Resource, serde_json::Value, DraftWithAttachments>(
            "CreateDraft",
            Some(resource),
            Some(&payload),
            Some(&mut out),
        )?;
        Ok((out.draft, out.attachments))
    }

    fn update_draft(
        &self,
        resource: &Resource,
        account: &AccountProjection,
        input: &UpdateDraftInput,
    ) -> Result<(DraftSnapshot, Vec<AttachmentReference>), MailError> {
        let mut out = DraftWithAttachments::default();
        let payload = serde_json::json!({ "account": account, "input": input });
        self.dispatch::<Resource, serde_json::Value, DraftWithAttachments>(
            "UpdateDraft",
            Some(resource),
            Some(&payload),
            Some(&mut out),
        )?;
        Ok((out.draft, out.attachments))
    }

    fn send_message(
        &self,
        resource: &Resource,
        account: &AccountProjection,
        input: &SendMessageInput,
    ) -> Result<(MessageSnapshot, Vec<AttachmentReference>), MailError> {
        let mut out = MessageWithAttachments::default();
        let payload = serde_json::json!({ "account": account, "input": input });
        self.dispatch::<Resource, serde_json::Value, MessageWithAttachments>(
            "SendMessage",
            Some(resource),
            Some(&payload),
            Some(&mut out),
        )?;
        Ok((out.message, out.attachments))
    }

    fn send_draft(
        &self,
        resource: &Resource,
        account: &AccountProjection,
        input: &SendDraftInput,
    ) -> Result<(DraftSnapshot, MessageSnapshot, Vec<AttachmentReference>), MailError> {
        let mut out = SendDraftResult::default();
        let payload = serde_json::json!({ "account": account, "input": input });
        self.dispatch::<Resource, serde_json::Value, SendDraftResult>(
            "SendDraft",
            Some(resource),
            Some(&payload),
            Some(&mut out),
        )?;
        Ok((out.draft, out.message, out.attachments))
    }

    fn reply_message(
        &self,
        resource: &Resource,
        account: &AccountProjection,
        input: &ReplyMessageInput,
    ) -> Result<
        (
            Option<DraftSnapshot>,
            Option<MessageSnapshot>,
            Vec<AttachmentReference>,
        ),
        MailError,
    > {
        let mut out = OptionalDraftMessage::default();
        let payload = serde_json::json!({ "account": account, "input": input });
        self.dispatch::<Resource, serde_json::Value, OptionalDraftMessage>(
            "ReplyMessage",
            Some(resource),
            Some(&payload),
            Some(&mut out),
        )?;
        Ok((out.draft, out.message, out.attachments))
    }

    fn forward_message(
        &self,
        resource: &Resource,
        account: &AccountProjection,
        input: &ForwardMessageInput,
    ) -> Result<
        (
            Option<DraftSnapshot>,
            Option<MessageSnapshot>,
            Vec<AttachmentReference>,
        ),
        MailError,
    > {
        let mut out = OptionalDraftMessage::default();
        let payload = serde_json::json!({ "account": account, "input": input });
        self.dispatch::<Resource, serde_json::Value, OptionalDraftMessage>(
            "ForwardMessage",
            Some(resource),
            Some(&payload),
            Some(&mut out),
        )?;
        Ok((out.draft, out.message, out.attachments))
    }

    fn resolve_attachments(
        &self,
        resource: &Resource,
        account: &AccountProjection,
        refs: &[AttachmentRefInput],
        parent_kind: &str,
        parent_id: &str,
    ) -> Vec<AttachmentReference> {
        let mut out: Vec<AttachmentReference> = Vec::new();
        let payload = serde_json::json!({ "account": account, "refs": refs, "parentKind": parent_kind, "parentId": parent_id });
        let _ = self.dispatch::<Resource, serde_json::Value, Vec<AttachmentReference>>(
            "ResolveAttachments",
            Some(resource),
            Some(&payload),
            Some(&mut out),
        );
        out
    }

    fn download_attachment(
        &self,
        resource: &Resource,
        account: &AccountProjection,
        input: &DownloadAttachmentInput,
    ) -> Result<AttachmentReference, MailError> {
        let mut out = AttachmentReference::default();
        let payload = serde_json::json!({ "account": account, "input": input });
        self.dispatch::<Resource, serde_json::Value, AttachmentReference>(
            "DownloadAttachment",
            Some(resource),
            Some(&payload),
            Some(&mut out),
        )?;
        Ok(out)
    }

    fn restore_integration_state(
        &self,
        _integration_id: &str,
        _threads: Vec<ThreadSnapshot>,
        _messages: Vec<MessageSnapshot>,
        _drafts: Vec<DraftSnapshot>,
        _attachments: Vec<AttachmentReference>,
    ) {
        // The adapter is stateless; restore is daemon-owned.
    }
}
