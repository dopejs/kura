//! Mail fake backend (port of `fake_backend.go`): an in-memory, deterministic backend used
//! as the default for `fake_local` bindings and for daemon tests.

use std::collections::{HashMap, HashSet};

use chrono::{DateTime, TimeZone, Utc};
use kura_integrations::Resource;
use parking_lot::Mutex;

use crate::{
    AccountProjection, AttachmentRefInput, AttachmentReference, AttachmentResolutionStatus,
    Backend, ComposeMode, CreateDraftInput, DeliveryState, Direction, DownloadAttachmentInput,
    DraftSnapshot, DraftStatus, ForwardMessageInput, ListDraftsInput, ListThreadsInput, MailError,
    MessageSnapshot, ReplyForwardResultMode, ReplyMessageInput, SendDraftInput, SendMessageInput,
    ThreadSnapshot, UpdateDraftInput, apply_attachment_policy, clone_drafts, clone_threads,
    first_non_empty, join_recipients,
};

#[derive(Debug)]
struct FakeState {
    account: AccountProjection,
    threads: HashMap<String, ThreadSnapshot>,
    messages: HashMap<String, MessageSnapshot>,
    drafts: HashMap<String, DraftSnapshot>,
    attachments: HashMap<String, AttachmentReference>,
}

pub struct FakeBackend {
    inner: Mutex<HashMap<String, FakeState>>,
}

impl FakeBackend {
    pub fn new() -> Self {
        FakeBackend {
            inner: Mutex::new(HashMap::new()),
        }
    }
}

impl Default for FakeBackend {
    fn default() -> Self {
        Self::new()
    }
}

fn ensure_state_locked<'a>(
    inner: &'a mut HashMap<String, FakeState>,
    resource: &Resource,
) -> &'a mut FakeState {
    if !inner.contains_key(&resource.integration_id) {
        let now = Utc::now();
        let thread_id = "thread_seed";
        let message_id = "msg_seed";
        let draft_id = "draft_seed";
        let account_key = resource
            .account_binding
            .as_ref()
            .map(|b| b.account_key.clone())
            .unwrap_or_default();
        let account_label = resource
            .account_binding
            .as_ref()
            .map(|b| b.account_label.clone())
            .unwrap_or_default();
        let state = FakeState {
            account: AccountProjection {
                mail_account_id: format!("mail_acct_{}", resource.integration_id),
                integration_id: resource.integration_id.clone(),
                domain_kind: resource.domain_kind.clone(),
                environment_scope: resource.environment_scope.clone(),
                account_key: account_key.clone(),
                account_label: account_label.clone(),
                readiness_status: resource.readiness_status.as_str().to_string(),
                canonical_default: resource.canonical_default,
                selection_mode: "explicit".to_string(),
                mailbox_address: first_non_empty(&[&account_key, "alice@example.com"]),
                mailbox_label: first_non_empty(&[&account_label, "Alice Mail"]),
                supports_thread_inspection: true,
                supports_drafts: true,
                supports_direct_send: true,
                supports_reply: true,
                supports_forward: true,
                last_synced_at: now,
                updated_at: now,
                ..AccountProjection::default()
            },
            threads: HashMap::from([(
                thread_id.to_string(),
                ThreadSnapshot {
                    thread_id: thread_id.to_string(),
                    integration_id: resource.integration_id.clone(),
                    mail_account_id: format!("mail_acct_{}", resource.integration_id),
                    subject: "Seed message thread".to_string(),
                    participant_summary: vec![
                        "alice@example.com".to_string(),
                        "bob@example.com".to_string(),
                    ],
                    message_ids: vec![message_id.to_string()],
                    draft_ids: vec![draft_id.to_string()],
                    latest_message_at: Utc
                        .with_ymd_and_hms(2026, 4, 23, 16, 0, 0)
                        .single()
                        .unwrap(),
                    message_count: 1,
                    draft_count: 1,
                    created_at: now,
                    ..ThreadSnapshot::default()
                },
            )]),
            messages: HashMap::from([(
                message_id.to_string(),
                MessageSnapshot {
                    message_id: message_id.to_string(),
                    thread_id: thread_id.to_string(),
                    integration_id: resource.integration_id.clone(),
                    mail_account_id: format!("mail_acct_{}", resource.integration_id),
                    direction: Direction::Inbound,
                    sender_summary: "bob@example.com".to_string(),
                    recipient_summary: vec!["alice@example.com".to_string()],
                    subject: "Seed message thread".to_string(),
                    body_preview: "Can you review phase 30 today?".to_string(),
                    delivery_state: DeliveryState::Received,
                    received_at: Some(
                        Utc.with_ymd_and_hms(2026, 4, 23, 16, 0, 0)
                            .single()
                            .unwrap(),
                    ),
                    created_at: now,
                    ..MessageSnapshot::default()
                },
            )]),
            drafts: HashMap::from([(
                draft_id.to_string(),
                DraftSnapshot {
                    draft_id: draft_id.to_string(),
                    thread_id: thread_id.to_string(),
                    integration_id: resource.integration_id.clone(),
                    mail_account_id: format!("mail_acct_{}", resource.integration_id),
                    compose_mode: ComposeMode::Reply,
                    source_message_id: message_id.to_string(),
                    recipient_summary: vec!["bob@example.com".to_string()],
                    subject: "Re: Seed message thread".to_string(),
                    body_preview: "Draft response body.".to_string(),
                    draft_status: DraftStatus::Draft,
                    created_at: now,
                    updated_at: now,
                    ..DraftSnapshot::default()
                },
            )]),
            attachments: HashMap::new(),
        };
        inner.insert(resource.integration_id.clone(), state);
    }
    inner.get_mut(&resource.integration_id).unwrap()
}

fn resolve_attachments_locked(
    inner: &mut HashMap<String, FakeState>,
    resource: &Resource,
    account: &AccountProjection,
    refs: &[AttachmentRefInput],
    parent_kind: &str,
    parent_id: &str,
    now: DateTime<Utc>,
) -> Vec<AttachmentReference> {
    if refs.is_empty() {
        return Vec::new();
    }
    ensure_state_locked(inner, resource);
    let mut items = Vec::with_capacity(refs.len());
    for (idx, reference) in refs.iter().enumerate() {
        let mut attachment_id = reference.attachment_ref_id.trim().to_string();
        if attachment_id.is_empty() {
            attachment_id = format!(
                "attachment_{}_{}_{}",
                resource.integration_id.replace('-', "_"),
                now.timestamp_nanos_opt().unwrap_or(0),
                idx
            );
        }
        let (status, failure_reason) =
            if looks_unresolved_attachment(&attachment_id, &reference.display_name) {
                (
                    AttachmentResolutionStatus::Unresolved,
                    "attachment reference not found".to_string(),
                )
            } else {
                (AttachmentResolutionStatus::Resolved, String::new())
            };
        let mut item = AttachmentReference {
            attachment_ref_id: attachment_id.clone(),
            integration_id: account.integration_id.clone(),
            parent_kind: parent_kind.to_string(),
            parent_id: parent_id.to_string(),
            display_name: reference.display_name.trim().to_string(),
            media_type: reference.media_type.trim().to_string(),
            size_bytes: reference.size_bytes,
            resolution_status: status,
            failure_reason,
            created_at: now,
            ..AttachmentReference::default()
        };
        if item.display_name.is_empty() {
            item.display_name = "attachment.bin".to_string();
        }
        if item.resolution_status == AttachmentResolutionStatus::Resolved {
            apply_attachment_policy(&mut item);
        }
        if let Some(state) = inner.get_mut(&resource.integration_id) {
            state
                .attachments
                .insert(item.attachment_ref_id.clone(), item.clone());
        }
        items.push(item);
    }
    items
}

fn attach_draft_to_thread_locked(state: &mut FakeState, draft: &DraftSnapshot) {
    if draft.thread_id.trim().is_empty() {
        return;
    }
    let mut thread = match state.threads.get(&draft.thread_id) {
        Some(t) => t.clone(),
        None => ThreadSnapshot {
            thread_id: draft.thread_id.clone(),
            integration_id: draft.integration_id.clone(),
            mail_account_id: draft.mail_account_id.clone(),
            subject: draft.subject.clone(),
            latest_message_at: draft.updated_at,
            created_at: draft.created_at,
            ..ThreadSnapshot::default()
        },
    };
    if !contains(&thread.draft_ids, &draft.draft_id) {
        thread.draft_ids.push(draft.draft_id.clone());
    }
    thread.draft_count = thread.draft_ids.len() as i64;
    thread.subject = first_non_empty(&[&draft.subject, &thread.subject]);
    state.threads.insert(draft.thread_id.clone(), thread);
}

fn append_message_to_thread_locked(
    state: &mut FakeState,
    thread_id: &str,
    message: &mut MessageSnapshot,
) {
    let mut id = thread_id.trim().to_string();
    if id.is_empty() {
        id = format!("thread_{}", message.message_id);
        message.thread_id = id.clone();
    }
    let mut thread = match state.threads.get(&id) {
        Some(t) => t.clone(),
        None => ThreadSnapshot {
            thread_id: id.clone(),
            integration_id: message.integration_id.clone(),
            mail_account_id: message.mail_account_id.clone(),
            subject: message.subject.clone(),
            created_at: message.created_at,
            ..ThreadSnapshot::default()
        },
    };
    if !contains(&thread.message_ids, &message.message_id) {
        thread.message_ids.push(message.message_id.clone());
    }
    thread.message_count = thread.message_ids.len() as i64;
    if let Some(sent_at) = message.sent_at {
        thread.latest_message_at = sent_at;
    } else if let Some(received_at) = message.received_at {
        thread.latest_message_at = received_at;
    }
    thread.subject = first_non_empty(&[&message.subject, &thread.subject]);
    let mut participants: Vec<String> = thread.participant_summary.clone();
    participants.push(message.sender_summary.clone());
    participants.extend(message.recipient_summary.iter().cloned());
    thread.participant_summary = unique_strings(&participants);
    state.threads.insert(id, thread);
}

fn attachment_ids(items: &[AttachmentReference]) -> Vec<String> {
    if items.is_empty() {
        return Vec::new();
    }
    items.iter().map(|i| i.attachment_ref_id.clone()).collect()
}

fn attachments_by_id(
    all: &HashMap<String, AttachmentReference>,
    ids: &[String],
) -> Vec<AttachmentReference> {
    if ids.is_empty() {
        return Vec::new();
    }
    ids.iter().filter_map(|id| all.get(id).cloned()).collect()
}

fn looks_unresolved_attachment(ref_id: &str, name: &str) -> bool {
    let text = format!("{} {}", ref_id.trim(), name.trim()).to_lowercase();
    text.contains("missing") || text.contains("broken") || text.contains("unresolved")
}

fn preview_body(body: &str) -> String {
    let trimmed = body.trim();
    if trimmed.len() <= 280 {
        trimmed.to_string()
    } else {
        trimmed.get(..280).unwrap_or(trimmed).to_string()
    }
}

fn reply_subject(subject: &str) -> String {
    let trimmed = subject.trim();
    if trimmed.to_lowercase().starts_with("re:") {
        trimmed.to_string()
    } else {
        format!("Re: {trimmed}")
    }
}

fn forward_subject(subject: &str) -> String {
    let trimmed = subject.trim();
    if trimmed.to_lowercase().starts_with("fwd:") {
        trimmed.to_string()
    } else {
        format!("Fwd: {trimmed}")
    }
}

fn contains(items: &[String], needle: &str) -> bool {
    items.iter().any(|i| i == needle)
}

fn unique_strings(items: &[String]) -> Vec<String> {
    let mut seen: HashSet<String> = HashSet::new();
    let mut out = Vec::new();
    for item in items {
        let trimmed = item.trim();
        if trimmed.is_empty() {
            continue;
        }
        if !seen.insert(trimmed.to_string()) {
            continue;
        }
        out.push(trimmed.to_string());
    }
    out
}

impl Backend for FakeBackend {
    fn supports_resource(&self, resource: &Resource) -> bool {
        resource.domain_kind.trim() == "mail"
            && kura_integrations::backend_kind_supports_domain(
                resource.backend_binding.backend_kind,
                &resource.domain_kind,
            )
    }

    fn project_account(&self, resource: &Resource) -> Result<AccountProjection, MailError> {
        let mut inner = self.inner.lock();
        let state = ensure_state_locked(&mut inner, resource);
        let now = Utc::now();
        state.account.readiness_status = resource.readiness_status.as_str().to_string();
        state.account.canonical_default = resource.canonical_default;
        state.account.account_key = resource
            .account_binding
            .as_ref()
            .map(|b| b.account_key.clone())
            .unwrap_or_default();
        state.account.account_label = resource
            .account_binding
            .as_ref()
            .map(|b| b.account_label.clone())
            .unwrap_or_default();
        let account_key = resource
            .account_binding
            .as_ref()
            .map(|b| b.account_key.clone())
            .unwrap_or_default();
        let account_label = resource
            .account_binding
            .as_ref()
            .map(|b| b.account_label.clone())
            .unwrap_or_default();
        if state.account.mailbox_address.is_empty() {
            state.account.mailbox_address = first_non_empty(&[&account_key, "alice@example.com"]);
        }
        state.account.mailbox_label = first_non_empty(&[&account_label, "Primary Mailbox"]);
        state.account.updated_at = now;
        state.account.last_synced_at = now;
        Ok(state.account.clone())
    }

    fn list_threads(
        &self,
        resource: &Resource,
        _account: &AccountProjection,
        input: &ListThreadsInput,
    ) -> Result<Vec<ThreadSnapshot>, MailError> {
        let mut inner = self.inner.lock();
        let state = ensure_state_locked(&mut inner, resource);
        let mut items: Vec<ThreadSnapshot> = state.threads.values().cloned().collect();
        items.sort_by(|a, b| {
            b.latest_message_at
                .cmp(&a.latest_message_at)
                .then_with(|| a.thread_id.cmp(&b.thread_id))
        });
        if input.limit > 0 && items.len() > input.limit as usize {
            items.truncate(input.limit as usize);
        }
        Ok(clone_threads(&items))
    }

    fn get_thread(
        &self,
        resource: &Resource,
        _account: &AccountProjection,
        thread_id: &str,
    ) -> Result<ThreadSnapshot, MailError> {
        let mut inner = self.inner.lock();
        let state = ensure_state_locked(&mut inner, resource);
        match state.threads.get(thread_id.trim()) {
            Some(item) => Ok(item.clone()),
            None => Err(MailError::MailThreadNotFound),
        }
    }

    fn get_message(
        &self,
        resource: &Resource,
        _account: &AccountProjection,
        message_id: &str,
    ) -> Result<MessageSnapshot, MailError> {
        let mut inner = self.inner.lock();
        let state = ensure_state_locked(&mut inner, resource);
        match state.messages.get(message_id.trim()) {
            Some(item) => Ok(item.clone()),
            None => Err(MailError::MailMessageNotFound),
        }
    }

    fn list_drafts(
        &self,
        resource: &Resource,
        _account: &AccountProjection,
        _input: &ListDraftsInput,
    ) -> Result<Vec<DraftSnapshot>, MailError> {
        let mut inner = self.inner.lock();
        let state = ensure_state_locked(&mut inner, resource);
        let mut items: Vec<DraftSnapshot> = state.drafts.values().cloned().collect();
        items.sort_by(|a, b| {
            b.updated_at
                .cmp(&a.updated_at)
                .then_with(|| a.draft_id.cmp(&b.draft_id))
        });
        Ok(clone_drafts(&items))
    }

    fn get_draft(
        &self,
        resource: &Resource,
        _account: &AccountProjection,
        draft_id: &str,
    ) -> Result<DraftSnapshot, MailError> {
        let mut inner = self.inner.lock();
        let state = ensure_state_locked(&mut inner, resource);
        match state.drafts.get(draft_id.trim()) {
            Some(item) => Ok(item.clone()),
            None => Err(MailError::MailDraftNotFound),
        }
    }

    fn create_draft(
        &self,
        resource: &Resource,
        account: &AccountProjection,
        input: &CreateDraftInput,
    ) -> Result<(DraftSnapshot, Vec<AttachmentReference>), MailError> {
        let mut inner = self.inner.lock();
        ensure_state_locked(&mut inner, resource);
        let now = Utc::now();
        let draft_id = format!(
            "draft_{}_{}",
            resource.integration_id.replace('-', "_"),
            now.timestamp_nanos_opt().unwrap_or(0)
        );
        let recipients = join_recipients(&[&input.to, &input.cc, &input.bcc]);
        let attachments = resolve_attachments_locked(
            &mut inner,
            resource,
            account,
            &input.attachment_refs,
            "draft",
            &draft_id,
            now,
        );
        let draft = DraftSnapshot {
            draft_id: draft_id.clone(),
            thread_id: input.thread_id.trim().to_string(),
            integration_id: account.integration_id.clone(),
            mail_account_id: account.mail_account_id.clone(),
            compose_mode: input.compose_mode,
            source_message_id: input.source_message_id.trim().to_string(),
            recipient_summary: recipients,
            subject: input.subject.trim().to_string(),
            body_preview: preview_body(&input.body),
            attachment_ref_ids: attachment_ids(&attachments),
            draft_status: DraftStatus::Draft,
            created_at: now,
            updated_at: now,
            ..DraftSnapshot::default()
        };
        let state = inner.get_mut(&resource.integration_id).unwrap();
        state.drafts.insert(draft_id.clone(), draft.clone());
        attach_draft_to_thread_locked(state, &draft);
        Ok((draft, attachments))
    }

    fn update_draft(
        &self,
        resource: &Resource,
        account: &AccountProjection,
        input: &UpdateDraftInput,
    ) -> Result<(DraftSnapshot, Vec<AttachmentReference>), MailError> {
        let mut inner = self.inner.lock();
        ensure_state_locked(&mut inner, resource);
        let now = Utc::now();
        let mut draft = {
            let state = inner.get(&resource.integration_id).unwrap();
            match state.drafts.get(input.draft_id.trim()) {
                Some(d) => d.clone(),
                None => return Err(MailError::MailDraftNotFound),
            }
        };
        let recipients = join_recipients(&[&input.to, &input.cc, &input.bcc]);
        if !recipients.is_empty() {
            draft.recipient_summary = recipients;
        }
        if !input.subject.trim().is_empty() {
            draft.subject = input.subject.trim().to_string();
        }
        if !input.body.trim().is_empty() {
            draft.body_preview = preview_body(&input.body);
        }
        if !input.attachment_refs.is_empty() {
            let attachments = resolve_attachments_locked(
                &mut inner,
                resource,
                account,
                &input.attachment_refs,
                "draft",
                &draft.draft_id,
                now,
            );
            draft.attachment_ref_ids = attachment_ids(&attachments);
            draft.draft_status = DraftStatus::Updated;
            draft.updated_at = now;
            let state = inner.get_mut(&resource.integration_id).unwrap();
            state.drafts.insert(draft.draft_id.clone(), draft.clone());
            attach_draft_to_thread_locked(state, &draft);
            return Ok((draft, attachments));
        }
        draft.draft_status = DraftStatus::Updated;
        draft.updated_at = now;
        let state = inner.get_mut(&resource.integration_id).unwrap();
        state.drafts.insert(draft.draft_id.clone(), draft.clone());
        attach_draft_to_thread_locked(state, &draft);
        let attachments = attachments_by_id(&state.attachments, &draft.attachment_ref_ids);
        Ok((draft, attachments))
    }

    fn send_message(
        &self,
        resource: &Resource,
        account: &AccountProjection,
        input: &SendMessageInput,
    ) -> Result<(MessageSnapshot, Vec<AttachmentReference>), MailError> {
        let mut inner = self.inner.lock();
        ensure_state_locked(&mut inner, resource);
        let now = Utc::now();
        let thread_id = format!(
            "thread_{}_{}",
            resource.integration_id.replace('-', "_"),
            now.timestamp_nanos_opt().unwrap_or(0)
        );
        let message_id = format!(
            "msg_{}_{}",
            resource.integration_id.replace('-', "_"),
            now.timestamp_nanos_opt().unwrap_or(0)
        );
        let attachments = resolve_attachments_locked(
            &mut inner,
            resource,
            account,
            &input.attachment_refs,
            "message",
            &message_id,
            now,
        );
        let message = MessageSnapshot {
            message_id: message_id.clone(),
            thread_id: thread_id.clone(),
            integration_id: account.integration_id.clone(),
            mail_account_id: account.mail_account_id.clone(),
            direction: Direction::Outbound,
            sender_summary: account.mailbox_address.clone(),
            recipient_summary: join_recipients(&[&input.to, &input.cc, &input.bcc]),
            subject: input.subject.trim().to_string(),
            body_preview: preview_body(&input.body),
            delivery_state: DeliveryState::Sent,
            attachment_ref_ids: attachment_ids(&attachments),
            sent_at: Some(now),
            created_at: now,
            ..MessageSnapshot::default()
        };
        let mut participants = vec![account.mailbox_address.clone()];
        participants.extend(message.recipient_summary.iter().cloned());
        let thread = ThreadSnapshot {
            thread_id: thread_id.clone(),
            integration_id: account.integration_id.clone(),
            mail_account_id: account.mail_account_id.clone(),
            subject: message.subject.clone(),
            participant_summary: participants,
            message_ids: vec![message_id],
            latest_message_at: now,
            message_count: 1,
            created_at: now,
            ..ThreadSnapshot::default()
        };
        let state = inner.get_mut(&resource.integration_id).unwrap();
        state
            .messages
            .insert(message.message_id.clone(), message.clone());
        state.threads.insert(thread_id, thread);
        Ok((message, attachments))
    }

    fn send_draft(
        &self,
        resource: &Resource,
        account: &AccountProjection,
        input: &SendDraftInput,
    ) -> Result<(DraftSnapshot, MessageSnapshot, Vec<AttachmentReference>), MailError> {
        let mut inner = self.inner.lock();
        ensure_state_locked(&mut inner, resource);
        let now = Utc::now();
        let mut draft = {
            let state = inner.get(&resource.integration_id).unwrap();
            match state.drafts.get(input.draft_id.trim()) {
                Some(d) => d.clone(),
                None => return Err(MailError::MailDraftNotFound),
            }
        };
        let message_id = format!(
            "msg_{}_{}",
            resource.integration_id.replace('-', "_"),
            now.timestamp_nanos_opt().unwrap_or(0)
        );
        let mut message = MessageSnapshot {
            message_id: message_id.clone(),
            thread_id: draft.thread_id.clone(),
            integration_id: account.integration_id.clone(),
            mail_account_id: account.mail_account_id.clone(),
            direction: Direction::Outbound,
            sender_summary: account.mailbox_address.clone(),
            recipient_summary: draft.recipient_summary.clone(),
            subject: draft.subject.clone(),
            body_preview: draft.body_preview.clone(),
            delivery_state: DeliveryState::Sent,
            attachment_ref_ids: draft.attachment_ref_ids.clone(),
            sent_at: Some(now),
            created_at: now,
            ..MessageSnapshot::default()
        };
        draft.draft_status = DraftStatus::SentFromDraft;
        draft.updated_at = now;
        let state = inner.get_mut(&resource.integration_id).unwrap();
        state.drafts.insert(draft.draft_id.clone(), draft.clone());
        append_message_to_thread_locked(state, &draft.thread_id, &mut message);
        state
            .messages
            .insert(message.message_id.clone(), message.clone());
        let attachments = attachments_by_id(&state.attachments, &draft.attachment_ref_ids);
        Ok((draft, message, attachments))
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
        let original = self.get_message(resource, account, &input.message_id)?;
        if input.result_mode == ReplyForwardResultMode::Draft {
            let (draft, attachments) = self.create_draft(
                resource,
                account,
                &CreateDraftInput {
                    compose_mode: ComposeMode::Reply,
                    thread_id: original.thread_id.clone(),
                    source_message_id: original.message_id.clone(),
                    to: vec![original.sender_summary.clone()],
                    subject: reply_subject(&original.subject),
                    body: input.body.clone(),
                    attachment_refs: input.attachment_refs.clone(),
                    ..CreateDraftInput::default()
                },
            )?;
            return Ok((Some(draft), None, attachments));
        }
        let (mut message, attachments) = self.send_message(
            resource,
            account,
            &SendMessageInput {
                to: vec![original.sender_summary.clone()],
                subject: first_non_empty(&[
                    input.subject.trim(),
                    &reply_subject(&original.subject),
                ]),
                body: input.body.clone(),
                attachment_refs: input.attachment_refs.clone(),
                ..SendMessageInput::default()
            },
        )?;
        message.thread_id = original.thread_id.clone();
        message.reply_to_message_id = original.message_id.clone();
        let mut inner = self.inner.lock();
        ensure_state_locked(&mut inner, resource);
        let state = inner.get_mut(&resource.integration_id).unwrap();
        append_message_to_thread_locked(state, &original.thread_id, &mut message);
        state
            .messages
            .insert(message.message_id.clone(), message.clone());
        Ok((None, Some(message), attachments))
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
        let original = self.get_message(resource, account, &input.message_id)?;
        if input.result_mode == ReplyForwardResultMode::Draft {
            let (draft, attachments) = self.create_draft(
                resource,
                account,
                &CreateDraftInput {
                    compose_mode: ComposeMode::Forward,
                    source_message_id: original.message_id.clone(),
                    to: input.to.clone(),
                    cc: input.cc.clone(),
                    bcc: input.bcc.clone(),
                    subject: first_non_empty(&[
                        input.subject.trim(),
                        &forward_subject(&original.subject),
                    ]),
                    body: input.body.clone(),
                    attachment_refs: input.attachment_refs.clone(),
                    ..CreateDraftInput::default()
                },
            )?;
            return Ok((Some(draft), None, attachments));
        }
        let (mut message, attachments) = self.send_message(
            resource,
            account,
            &SendMessageInput {
                to: input.to.clone(),
                cc: input.cc.clone(),
                bcc: input.bcc.clone(),
                subject: first_non_empty(&[
                    input.subject.trim(),
                    &forward_subject(&original.subject),
                ]),
                body: input.body.clone(),
                attachment_refs: input.attachment_refs.clone(),
                ..SendMessageInput::default()
            },
        )?;
        message.forwarded_from_message_id = original.message_id.clone();
        let mut inner = self.inner.lock();
        ensure_state_locked(&mut inner, resource);
        let state = inner.get_mut(&resource.integration_id).unwrap();
        state
            .messages
            .insert(message.message_id.clone(), message.clone());
        Ok((None, Some(message), attachments))
    }

    fn resolve_attachments(
        &self,
        resource: &Resource,
        account: &AccountProjection,
        refs: &[AttachmentRefInput],
        parent_kind: &str,
        parent_id: &str,
    ) -> Vec<AttachmentReference> {
        let mut inner = self.inner.lock();
        resolve_attachments_locked(
            &mut inner,
            resource,
            account,
            refs,
            parent_kind,
            parent_id,
            Utc::now(),
        )
    }

    fn download_attachment(
        &self,
        resource: &Resource,
        account: &AccountProjection,
        input: &DownloadAttachmentInput,
    ) -> Result<AttachmentReference, MailError> {
        let mut inner = self.inner.lock();
        ensure_state_locked(&mut inner, resource);
        let now = Utc::now();
        let ref_id = input.attachment_ref_id.trim();
        if ref_id.is_empty() {
            return Err(MailError::MailAttachmentUnresolved);
        }
        let mut reference = AttachmentReference {
            attachment_ref_id: ref_id.to_string(),
            integration_id: account.integration_id.clone(),
            parent_kind: "message".to_string(),
            parent_id: input.message_id.trim().to_string(),
            display_name: first_non_empty(&[input.display_name.trim(), "attachment.bin"]),
            media_type: input.media_type.trim().to_string(),
            size_bytes: Some(input.size_bytes),
            resolution_status: AttachmentResolutionStatus::Resolved,
            created_at: now,
            ..AttachmentReference::default()
        };
        apply_attachment_policy(&mut reference);
        if reference.resolution_status == AttachmentResolutionStatus::Resolved {
            reference.downloaded = true;
        }
        if let Some(state) = inner.get_mut(&resource.integration_id) {
            state
                .attachments
                .insert(reference.attachment_ref_id.clone(), reference.clone());
        }
        Ok(reference)
    }

    fn restore_integration_state(
        &self,
        integration_id: &str,
        threads: Vec<ThreadSnapshot>,
        messages: Vec<MessageSnapshot>,
        drafts: Vec<DraftSnapshot>,
        attachments: Vec<AttachmentReference>,
    ) {
        let mut inner = self.inner.lock();
        let trimmed = integration_id.trim();
        let state = inner
            .entry(trimmed.to_string())
            .or_insert_with(|| FakeState {
                account: AccountProjection {
                    mail_account_id: format!("mail_acct_{trimmed}"),
                    integration_id: trimmed.to_string(),
                    domain_kind: "mail".to_string(),
                    ..AccountProjection::default()
                },
                threads: HashMap::new(),
                messages: HashMap::new(),
                drafts: HashMap::new(),
                attachments: HashMap::new(),
            });
        state.threads = threads
            .into_iter()
            .map(|t| (t.thread_id.clone(), t))
            .collect();
        state.messages = messages
            .into_iter()
            .map(|m| (m.message_id.clone(), m))
            .collect();
        state.drafts = drafts
            .into_iter()
            .map(|d| (d.draft_id.clone(), d))
            .collect();
        state.attachments = attachments
            .into_iter()
            .map(|a| (a.attachment_ref_id.clone(), a))
            .collect();
    }
}
