//! Mail operation manager (port of `manager.go`): the single operation ledger, account
//! selection, backend dispatch, attachment resolution, and diagnostic failure projection.

use std::collections::HashMap;
use std::sync::Arc;

use chrono::Utc;
use kura_integrations::{BackendKind, ReadinessStatus, Resource};

use crate::{
    AccountProjection, Artifact, AttachmentReference, AttachmentResolutionStatus, Backend,
    ComposeMode, CreateDraftInput, DownloadAttachmentInput, DraftSnapshot, ForwardMessageInput,
    GetDraftInput, GetMessageInput, GetThreadInput, ListDraftsInput, ListThreadsInput, MailError,
    MessageSnapshot, Operation, OperationClass, OperationFilter, OperationStatus,
    ReplyForwardResultMode, ReplyMessageInput, ResultMode, Selection, SendDraftInput,
    SendMessageInput, SendPath, SourceLinkage, ThreadSnapshot, UpdateDraftInput,
    attachment_artifact, attachment_refs_from_ids, collect_artifact_ids, draft_artifact,
    first_non_empty, is_background_send, message_artifact, new_operation_id, summarize_draft_input,
    summarize_list_threads, thread_artifact, validate_explicit_recipients,
};

#[derive(Default)]
struct ManagerInner {
    backends: HashMap<BackendKind, Arc<dyn Backend>>,
    accounts: HashMap<String, AccountProjection>,
    operations: HashMap<String, Operation>,
    op_order: Vec<String>,
    artifacts: HashMap<String, Artifact>,
}

pub struct Manager {
    env: String,
    inner: parking_lot::RwLock<ManagerInner>,
}

impl Manager {
    pub fn new(environment_scope: &str) -> Self {
        let mut inner = ManagerInner::default();
        inner
            .backends
            .insert(BackendKind::FakeLocal, Arc::new(super::FakeBackend::new()));
        Manager {
            env: environment_scope.trim().to_string(),
            inner: parking_lot::RwLock::new(inner),
        }
    }

    pub fn register_backend(&self, kind: BackendKind, backend: Arc<dyn Backend>) {
        self.inner.write().backends.insert(kind, backend);
    }

    pub fn restore(
        &self,
        accounts: Vec<AccountProjection>,
        operations: Vec<Operation>,
        artifacts: Vec<Artifact>,
    ) {
        let mut inner = self.inner.write();
        inner.accounts = accounts
            .into_iter()
            .map(|a| (a.integration_id.clone(), a))
            .collect();
        inner.operations = operations
            .iter()
            .map(|o| (o.operation_id.clone(), o.clone()))
            .collect();
        inner.op_order = operations.iter().map(|o| o.operation_id.clone()).collect();
        inner.artifacts = artifacts
            .iter()
            .map(|a| (a.artifact_id.clone(), a.clone()))
            .collect();

        let mut threads_by_integration: HashMap<String, Vec<ThreadSnapshot>> = HashMap::new();
        let mut messages_by_integration: HashMap<String, Vec<MessageSnapshot>> = HashMap::new();
        let mut drafts_by_integration: HashMap<String, Vec<DraftSnapshot>> = HashMap::new();
        let mut attachments_by_integration: HashMap<String, Vec<AttachmentReference>> =
            HashMap::new();
        for item in &artifacts {
            match item.kind {
                crate::ArtifactKind::ThreadSnapshot => {
                    if let Some(thread) = &item.thread {
                        threads_by_integration
                            .entry(item.integration_id.clone())
                            .or_default()
                            .push(thread.clone());
                    }
                }
                crate::ArtifactKind::MessageSnapshot => {
                    if let Some(message) = &item.message {
                        messages_by_integration
                            .entry(item.integration_id.clone())
                            .or_default()
                            .push(message.clone());
                    }
                }
                crate::ArtifactKind::DraftSnapshot => {
                    if let Some(draft) = &item.draft {
                        drafts_by_integration
                            .entry(item.integration_id.clone())
                            .or_default()
                            .push(draft.clone());
                    }
                }
                crate::ArtifactKind::AttachmentRef => {
                    if let Some(attachment) = &item.attachment {
                        attachments_by_integration
                            .entry(item.integration_id.clone())
                            .or_default()
                            .push(attachment.clone());
                    }
                }
            }
        }
        if let Some(backend) = inner.backends.get(&BackendKind::FakeLocal).cloned() {
            let mut restored: std::collections::HashSet<String> = std::collections::HashSet::new();
            for (integration_id, threads) in &threads_by_integration {
                backend.restore_integration_state(
                    integration_id,
                    threads.clone(),
                    messages_by_integration
                        .get(integration_id)
                        .cloned()
                        .unwrap_or_default(),
                    drafts_by_integration
                        .get(integration_id)
                        .cloned()
                        .unwrap_or_default(),
                    attachments_by_integration
                        .get(integration_id)
                        .cloned()
                        .unwrap_or_default(),
                );
                restored.insert(integration_id.clone());
            }
            for (integration_id, messages) in &messages_by_integration {
                if restored.contains(integration_id) {
                    continue;
                }
                backend.restore_integration_state(
                    integration_id,
                    Vec::new(),
                    messages.clone(),
                    drafts_by_integration
                        .get(integration_id)
                        .cloned()
                        .unwrap_or_default(),
                    attachments_by_integration
                        .get(integration_id)
                        .cloned()
                        .unwrap_or_default(),
                );
                restored.insert(integration_id.clone());
            }
            for (integration_id, drafts) in &drafts_by_integration {
                if restored.contains(integration_id) {
                    continue;
                }
                backend.restore_integration_state(
                    integration_id,
                    Vec::new(),
                    Vec::new(),
                    drafts.clone(),
                    attachments_by_integration
                        .get(integration_id)
                        .cloned()
                        .unwrap_or_default(),
                );
                restored.insert(integration_id.clone());
            }
            for (integration_id, attachments) in &attachments_by_integration {
                if restored.contains(integration_id) {
                    continue;
                }
                backend.restore_integration_state(
                    integration_id,
                    Vec::new(),
                    Vec::new(),
                    Vec::new(),
                    attachments.clone(),
                );
            }
        }
    }

    pub fn list_accounts(
        &self,
        resources: &[Resource],
        selection: &Selection,
    ) -> Result<Vec<AccountProjection>, MailError> {
        if !selection.integration_id.trim().is_empty() {
            let (account, _, _, _) = self.select_account(resources, selection)?;
            return Ok(vec![account]);
        }
        let mut items = Vec::new();
        for resource in resources {
            if resource.domain_kind != "mail" || resource.environment_scope.trim() != self.env {
                continue;
            }
            let sel = Selection {
                integration_id: resource.integration_id.clone(),
                ..Selection::default()
            };
            let (account, _, _, _) = self.select_account(resources, &sel)?;
            items.push(account);
        }
        Ok(items)
    }

    pub fn list_threads(
        &self,
        resources: &[Resource],
        input: &ListThreadsInput,
    ) -> Result<
        (
            AccountProjection,
            Vec<ThreadSnapshot>,
            Operation,
            Vec<Artifact>,
        ),
        MailError,
    > {
        let (account, resource, backend, selection_mode) =
            self.select_account(resources, &input.selection)?;
        let operation = self.new_operation(
            &account,
            &resource,
            OperationClass::ListThreads,
            &selection_mode,
            &summarize_list_threads(input),
            &input.source,
        );
        let mut items = match backend.list_threads(&resource, &account, input) {
            Ok(items) => items,
            Err(err) => {
                let (class, provider_kind) = failure_class_and_provider("backend_error", &err);
                self.fail_operation(operation, &class, &provider_kind, &err.to_string());
                return Err(err);
            }
        };
        let mut artifacts = Vec::with_capacity(items.len());
        for item in &mut items {
            item.operation_id = operation.operation_id.clone();
            item.integration_id = account.integration_id.clone();
            item.mail_account_id = account.mail_account_id.clone();
            artifacts.push(thread_artifact(&operation, item));
        }
        let operation = self.complete_operation(
            operation,
            ResultMode::Inspection,
            "",
            "",
            "",
            artifacts.clone(),
        );
        Ok((account, items, operation, artifacts))
    }

    pub fn get_thread(
        &self,
        resources: &[Resource],
        input: &GetThreadInput,
    ) -> Result<(AccountProjection, ThreadSnapshot, Operation, Vec<Artifact>), MailError> {
        let (account, resource, backend, selection_mode) =
            self.select_account(resources, &input.selection)?;
        let operation = self.new_operation(
            &account,
            &resource,
            OperationClass::GetThread,
            &selection_mode,
            &input.thread_id,
            &input.source,
        );
        let mut item = match backend.get_thread(&resource, &account, &input.thread_id) {
            Ok(item) => item,
            Err(err) => {
                let (class, provider_kind) = failure_class_and_provider("not_found", &err);
                self.fail_operation(operation, &class, &provider_kind, &err.to_string());
                return Err(err);
            }
        };
        item.operation_id = operation.operation_id.clone();
        item.integration_id = account.integration_id.clone();
        item.mail_account_id = account.mail_account_id.clone();
        let artifact = thread_artifact(&operation, &item);
        let operation = self.complete_operation(
            operation,
            ResultMode::Inspection,
            &item.thread_id,
            "",
            "",
            vec![artifact.clone()],
        );
        Ok((account, item, operation, vec![artifact]))
    }

    pub fn get_message(
        &self,
        resources: &[Resource],
        input: &GetMessageInput,
    ) -> Result<(AccountProjection, MessageSnapshot, Operation, Vec<Artifact>), MailError> {
        let (account, resource, backend, selection_mode) =
            self.select_account(resources, &input.selection)?;
        let operation = self.new_operation(
            &account,
            &resource,
            OperationClass::GetMessage,
            &selection_mode,
            &input.message_id,
            &input.source,
        );
        let mut item = match backend.get_message(&resource, &account, &input.message_id) {
            Ok(item) => item,
            Err(err) => {
                let (class, provider_kind) = failure_class_and_provider("not_found", &err);
                self.fail_operation(operation, &class, &provider_kind, &err.to_string());
                return Err(err);
            }
        };
        item.operation_id = operation.operation_id.clone();
        item.integration_id = account.integration_id.clone();
        item.mail_account_id = account.mail_account_id.clone();
        let mut artifacts = vec![message_artifact(&operation, &item)];
        let refs = attachment_refs_from_ids(&item.attachment_ref_ids);
        for mut attachment in
            backend.resolve_attachments(&resource, &account, &refs, "message", &item.message_id)
        {
            attachment.operation_id = operation.operation_id.clone();
            artifacts.push(attachment_artifact(&operation, &attachment));
        }
        let operation = self.complete_operation(
            operation,
            ResultMode::Inspection,
            &item.thread_id,
            &item.message_id,
            "",
            artifacts.clone(),
        );
        Ok((account, item, operation, artifacts))
    }

    pub fn list_drafts(
        &self,
        resources: &[Resource],
        input: &ListDraftsInput,
    ) -> Result<
        (
            AccountProjection,
            Vec<DraftSnapshot>,
            Operation,
            Vec<Artifact>,
        ),
        MailError,
    > {
        let (account, resource, backend, selection_mode) =
            self.select_account(resources, &input.selection)?;
        let operation = self.new_operation(
            &account,
            &resource,
            OperationClass::ListDrafts,
            &selection_mode,
            "",
            &input.source,
        );
        let mut items = match backend.list_drafts(&resource, &account, input) {
            Ok(items) => items,
            Err(err) => {
                let (class, provider_kind) = failure_class_and_provider("backend_error", &err);
                self.fail_operation(operation, &class, &provider_kind, &err.to_string());
                return Err(err);
            }
        };
        let mut artifacts = Vec::with_capacity(items.len());
        for item in &mut items {
            item.operation_id = operation.operation_id.clone();
            item.integration_id = account.integration_id.clone();
            item.mail_account_id = account.mail_account_id.clone();
            artifacts.push(draft_artifact(&operation, item));
        }
        let operation = self.complete_operation(
            operation,
            ResultMode::Inspection,
            "",
            "",
            "",
            artifacts.clone(),
        );
        Ok((account, items, operation, artifacts))
    }

    pub fn get_draft(
        &self,
        resources: &[Resource],
        input: &GetDraftInput,
    ) -> Result<(AccountProjection, DraftSnapshot, Operation, Vec<Artifact>), MailError> {
        let (account, resource, backend, selection_mode) =
            self.select_account(resources, &input.selection)?;
        let operation = self.new_operation(
            &account,
            &resource,
            OperationClass::GetDraft,
            &selection_mode,
            &input.draft_id,
            &input.source,
        );
        let mut item = match backend.get_draft(&resource, &account, &input.draft_id) {
            Ok(item) => item,
            Err(err) => {
                let (class, provider_kind) = failure_class_and_provider("not_found", &err);
                self.fail_operation(operation, &class, &provider_kind, &err.to_string());
                return Err(err);
            }
        };
        item.operation_id = operation.operation_id.clone();
        item.integration_id = account.integration_id.clone();
        item.mail_account_id = account.mail_account_id.clone();
        let mut artifacts = vec![draft_artifact(&operation, &item)];
        let refs = attachment_refs_from_ids(&item.attachment_ref_ids);
        for mut attachment in
            backend.resolve_attachments(&resource, &account, &refs, "draft", &item.draft_id)
        {
            attachment.operation_id = operation.operation_id.clone();
            artifacts.push(attachment_artifact(&operation, &attachment));
        }
        let operation = self.complete_operation(
            operation,
            ResultMode::Inspection,
            &item.thread_id,
            "",
            &item.draft_id,
            artifacts.clone(),
        );
        Ok((account, item, operation, artifacts))
    }

    pub fn download_attachment(
        &self,
        resources: &[Resource],
        input: &DownloadAttachmentInput,
    ) -> Result<
        (
            AccountProjection,
            AttachmentReference,
            Operation,
            Vec<Artifact>,
        ),
        MailError,
    > {
        let (account, resource, backend, selection_mode) =
            self.select_account(resources, &input.selection)?;
        let operation = self.new_operation(
            &account,
            &resource,
            OperationClass::DownloadAttachment,
            &selection_mode,
            &input.attachment_ref_id,
            &input.source,
        );
        let reference = match backend.download_attachment(&resource, &account, input) {
            Ok(reference) => reference,
            Err(err) => {
                let (class, provider_kind) = failure_class_and_provider("not_found", &err);
                self.fail_operation(operation, &class, &provider_kind, &err.to_string());
                return Err(err);
            }
        };
        if reference.resolution_status != AttachmentResolutionStatus::Resolved {
            let reason = first_non_empty(&[
                &reference.failure_reason,
                "attachment rejected by transfer policy",
            ]);
            let failed = self.fail_operation(operation, "attachment_policy_rejected", "", &reason);
            let artifact = attachment_artifact(&failed, &reference);
            self.store_artifact(artifact);
            return Err(MailError::MailAttachmentUnresolved);
        }
        let mut reference = reference;
        reference.operation_id = operation.operation_id.clone();
        reference.integration_id = account.integration_id.clone();
        let artifact = attachment_artifact(&operation, &reference);
        let operation = self.complete_operation(
            operation,
            ResultMode::Inspection,
            "",
            &input.message_id,
            "",
            vec![artifact.clone()],
        );
        Ok((account, reference, operation, vec![artifact]))
    }

    pub fn create_draft(
        &self,
        resources: &[Resource],
        input: &CreateDraftInput,
    ) -> Result<(AccountProjection, DraftSnapshot, Operation, Vec<Artifact>), MailError> {
        let (account, resource, backend, selection_mode) =
            self.select_account(resources, &input.selection)?;
        let operation = self.new_operation(
            &account,
            &resource,
            OperationClass::CreateDraft,
            &selection_mode,
            &summarize_draft_input(&input.subject, &input.to, &input.cc, &input.bcc),
            &input.source,
        );
        let (mut item, attachments) = match backend.create_draft(&resource, &account, input) {
            Ok(result) => result,
            Err(err) => {
                let (class, provider_kind) = failure_class_and_provider("backend_error", &err);
                self.fail_operation(operation, &class, &provider_kind, &err.to_string());
                return Err(err);
            }
        };
        item.operation_id = operation.operation_id.clone();
        item.integration_id = account.integration_id.clone();
        item.mail_account_id = account.mail_account_id.clone();
        let mut artifacts = vec![draft_artifact(&operation, &item)];
        for mut attachment in attachments {
            attachment.operation_id = operation.operation_id.clone();
            artifacts.push(attachment_artifact(&operation, &attachment));
        }
        let operation = self.complete_operation(
            operation,
            ResultMode::DraftOnly,
            &item.thread_id,
            "",
            &item.draft_id,
            artifacts.clone(),
        );
        Ok((account, item, operation, artifacts))
    }

    pub fn update_draft(
        &self,
        resources: &[Resource],
        input: &UpdateDraftInput,
    ) -> Result<(AccountProjection, DraftSnapshot, Operation, Vec<Artifact>), MailError> {
        let (account, resource, backend, selection_mode) =
            self.select_account(resources, &input.selection)?;
        let operation = self.new_operation(
            &account,
            &resource,
            OperationClass::UpdateDraft,
            &selection_mode,
            &input.draft_id,
            &input.source,
        );
        let (mut item, attachments) = match backend.update_draft(&resource, &account, input) {
            Ok(result) => result,
            Err(err) => {
                let (class, provider_kind) = failure_class_and_provider("not_found", &err);
                self.fail_operation(operation, &class, &provider_kind, &err.to_string());
                return Err(err);
            }
        };
        item.operation_id = operation.operation_id.clone();
        item.integration_id = account.integration_id.clone();
        item.mail_account_id = account.mail_account_id.clone();
        let mut artifacts = vec![draft_artifact(&operation, &item)];
        for mut attachment in attachments {
            attachment.operation_id = operation.operation_id.clone();
            artifacts.push(attachment_artifact(&operation, &attachment));
        }
        let operation = self.complete_operation(
            operation,
            ResultMode::DraftOnly,
            &item.thread_id,
            "",
            &item.draft_id,
            artifacts.clone(),
        );
        Ok((account, item, operation, artifacts))
    }

    pub fn send_message(
        &self,
        resources: &[Resource],
        input: &SendMessageInput,
    ) -> Result<(AccountProjection, MessageSnapshot, Operation, Vec<Artifact>), MailError> {
        let (account, resource, backend, selection_mode) =
            self.select_account(resources, &input.selection)?;
        let operation = self.new_operation(
            &account,
            &resource,
            OperationClass::SendMessage,
            &selection_mode,
            &summarize_draft_input(&input.subject, &input.to, &input.cc, &input.bcc),
            &input.source,
        );
        if let Err(err) = validate_explicit_recipients(&crate::join_recipients(&[
            &input.to, &input.cc, &input.bcc,
        ])) {
            self.block_operation(
                operation,
                ResultMode::Blocked,
                "recipient_required",
                &err.to_string(),
            );
            return Err(err);
        }
        let (blocked, _artifacts) = self.resolve_and_block_on_attachments(
            &operation,
            &backend,
            &resource,
            &account,
            &input.attachment_refs,
            "message",
            "pending_direct_send",
        );
        if !blocked.operation_id.is_empty() {
            return Err(MailError::MailAttachmentUnresolved);
        }
        if is_background_send(&input.source) && !input.source.allow_send_side_effects {
            self.block_operation(
                operation,
                ResultMode::Blocked,
                "send_permission_required",
                &MailError::MailBackgroundSendBlocked.to_string(),
            );
            return Err(MailError::MailBackgroundSendBlocked);
        }
        let (mut item, attachments) = match backend.send_message(&resource, &account, input) {
            Ok(result) => result,
            Err(err) => {
                let (class, provider_kind) = failure_class_and_provider("backend_error", &err);
                self.fail_operation(operation, &class, &provider_kind, &err.to_string());
                return Err(err);
            }
        };
        item.operation_id = operation.operation_id.clone();
        item.integration_id = account.integration_id.clone();
        item.mail_account_id = account.mail_account_id.clone();
        let mut artifacts = vec![message_artifact(&operation, &item)];
        for mut attachment in attachments {
            attachment.operation_id = operation.operation_id.clone();
            artifacts.push(attachment_artifact(&operation, &attachment));
        }
        let mut operation = operation;
        operation.send_path = SendPath::Direct.as_str().to_string();
        let operation = self.complete_operation(
            operation,
            ResultMode::Sent,
            &item.thread_id,
            &item.message_id,
            "",
            artifacts.clone(),
        );
        Ok((account, item, operation, artifacts))
    }

    pub fn send_draft(
        &self,
        resources: &[Resource],
        input: &SendDraftInput,
    ) -> Result<
        (
            AccountProjection,
            DraftSnapshot,
            MessageSnapshot,
            Operation,
            Vec<Artifact>,
        ),
        MailError,
    > {
        let (account, resource, backend, selection_mode) =
            self.select_account(resources, &input.selection)?;
        let operation = self.new_operation(
            &account,
            &resource,
            OperationClass::SendDraft,
            &selection_mode,
            &input.draft_id,
            &input.source,
        );
        if is_background_send(&input.source) && !input.source.allow_send_side_effects {
            self.block_operation(
                operation,
                ResultMode::Blocked,
                "send_permission_required",
                &MailError::MailBackgroundSendBlocked.to_string(),
            );
            return Err(MailError::MailBackgroundSendBlocked);
        }
        let draft_before = match backend.get_draft(&resource, &account, &input.draft_id) {
            Ok(draft) => draft,
            Err(err) => {
                let (class, provider_kind) = failure_class_and_provider("not_found", &err);
                self.fail_operation(operation, &class, &provider_kind, &err.to_string());
                return Err(err);
            }
        };
        if validate_explicit_recipients(&draft_before.recipient_summary).is_err()
            && draft_before.compose_mode == ComposeMode::NewMessage
        {
            self.block_operation(
                operation,
                ResultMode::Blocked,
                "recipient_required",
                &MailError::MailRecipientRequired.to_string(),
            );
            return Err(MailError::MailRecipientRequired);
        }
        let refs = attachment_refs_from_ids(&draft_before.attachment_ref_ids);
        let (blocked, _artifacts) = self.resolve_and_block_on_attachments(
            &operation,
            &backend,
            &resource,
            &account,
            &refs,
            "draft",
            &draft_before.draft_id,
        );
        if !blocked.operation_id.is_empty() {
            return Err(MailError::MailAttachmentUnresolved);
        }
        let (mut draft, mut message, attachments) =
            match backend.send_draft(&resource, &account, input) {
                Ok(result) => result,
                Err(err) => {
                    let (class, provider_kind) = failure_class_and_provider("backend_error", &err);
                    self.fail_operation(operation, &class, &provider_kind, &err.to_string());
                    return Err(err);
                }
            };
        draft.operation_id = operation.operation_id.clone();
        draft.integration_id = account.integration_id.clone();
        draft.mail_account_id = account.mail_account_id.clone();
        message.operation_id = operation.operation_id.clone();
        message.integration_id = account.integration_id.clone();
        message.mail_account_id = account.mail_account_id.clone();
        let mut artifacts = vec![
            draft_artifact(&operation, &draft),
            message_artifact(&operation, &message),
        ];
        for mut attachment in attachments {
            attachment.operation_id = operation.operation_id.clone();
            artifacts.push(attachment_artifact(&operation, &attachment));
        }
        let mut operation = operation;
        operation.send_path = SendPath::Draft.as_str().to_string();
        let operation = self.complete_operation(
            operation,
            ResultMode::Sent,
            &message.thread_id,
            &message.message_id,
            &draft.draft_id,
            artifacts.clone(),
        );
        Ok((account, draft, message, operation, artifacts))
    }

    pub fn reply_message(
        &self,
        resources: &[Resource],
        input: &ReplyMessageInput,
    ) -> Result<
        (
            AccountProjection,
            Option<DraftSnapshot>,
            Option<MessageSnapshot>,
            Operation,
            Vec<Artifact>,
        ),
        MailError,
    > {
        let (account, resource, backend, selection_mode) =
            self.select_account(resources, &input.selection)?;
        let operation = self.new_operation(
            &account,
            &resource,
            OperationClass::ReplyMessage,
            &selection_mode,
            &input.message_id,
            &input.source,
        );
        if input.result_mode == ReplyForwardResultMode::Send {
            let (blocked, _) = self.resolve_and_block_on_attachments(
                &operation,
                &backend,
                &resource,
                &account,
                &input.attachment_refs,
                "message",
                "pending_reply_send",
            );
            if !blocked.operation_id.is_empty() {
                return Err(MailError::MailAttachmentUnresolved);
            }
            if is_background_send(&input.source) && !input.source.allow_send_side_effects {
                self.block_operation(
                    operation,
                    ResultMode::Blocked,
                    "send_permission_required",
                    &MailError::MailBackgroundSendBlocked.to_string(),
                );
                return Err(MailError::MailBackgroundSendBlocked);
            }
        }
        let (mut draft, mut message, attachments) =
            match backend.reply_message(&resource, &account, input) {
                Ok(result) => result,
                Err(err) => {
                    let (class, provider_kind) = failure_class_and_provider("backend_error", &err);
                    self.fail_operation(operation, &class, &provider_kind, &err.to_string());
                    return Err(err);
                }
            };
        let mut artifacts = Vec::new();
        let mut result_mode = ResultMode::DraftOnly;
        let mut thread_id = String::new();
        let mut message_id = String::new();
        let mut draft_id = String::new();
        if let Some(d) = draft.as_mut() {
            d.operation_id = operation.operation_id.clone();
            d.integration_id = account.integration_id.clone();
            d.mail_account_id = account.mail_account_id.clone();
            thread_id = d.thread_id.clone();
            draft_id = d.draft_id.clone();
            artifacts.push(draft_artifact(&operation, d));
        }
        if let Some(m) = message.as_mut() {
            result_mode = ResultMode::Sent;
            m.operation_id = operation.operation_id.clone();
            m.integration_id = account.integration_id.clone();
            m.mail_account_id = account.mail_account_id.clone();
            thread_id = m.thread_id.clone();
            message_id = m.message_id.clone();
            artifacts.push(message_artifact(&operation, m));
        }
        for mut attachment in attachments {
            attachment.operation_id = operation.operation_id.clone();
            artifacts.push(attachment_artifact(&operation, &attachment));
        }
        let mut operation = operation;
        if result_mode == ResultMode::Sent {
            operation.send_path = SendPath::Direct.as_str().to_string();
        }
        let operation = self.complete_operation(
            operation,
            result_mode,
            &thread_id,
            &message_id,
            &draft_id,
            artifacts.clone(),
        );
        Ok((account, draft, message, operation, artifacts))
    }

    pub fn forward_message(
        &self,
        resources: &[Resource],
        input: &ForwardMessageInput,
    ) -> Result<
        (
            AccountProjection,
            Option<DraftSnapshot>,
            Option<MessageSnapshot>,
            Operation,
            Vec<Artifact>,
        ),
        MailError,
    > {
        let (account, resource, backend, selection_mode) =
            self.select_account(resources, &input.selection)?;
        let operation = self.new_operation(
            &account,
            &resource,
            OperationClass::ForwardMessage,
            &selection_mode,
            &input.message_id,
            &input.source,
        );
        if input.result_mode == ReplyForwardResultMode::Send {
            if let Err(err) = validate_explicit_recipients(&crate::join_recipients(&[
                &input.to, &input.cc, &input.bcc,
            ])) {
                self.block_operation(
                    operation,
                    ResultMode::Blocked,
                    "recipient_required",
                    &err.to_string(),
                );
                return Err(err);
            }
            let (blocked, _) = self.resolve_and_block_on_attachments(
                &operation,
                &backend,
                &resource,
                &account,
                &input.attachment_refs,
                "message",
                "pending_forward_send",
            );
            if !blocked.operation_id.is_empty() {
                return Err(MailError::MailAttachmentUnresolved);
            }
            if is_background_send(&input.source) && !input.source.allow_send_side_effects {
                self.block_operation(
                    operation,
                    ResultMode::Blocked,
                    "send_permission_required",
                    &MailError::MailBackgroundSendBlocked.to_string(),
                );
                return Err(MailError::MailBackgroundSendBlocked);
            }
        }
        let (mut draft, mut message, attachments) =
            match backend.forward_message(&resource, &account, input) {
                Ok(result) => result,
                Err(err) => {
                    let (class, provider_kind) = failure_class_and_provider("backend_error", &err);
                    self.fail_operation(operation, &class, &provider_kind, &err.to_string());
                    return Err(err);
                }
            };
        let mut artifacts = Vec::new();
        let mut result_mode = ResultMode::DraftOnly;
        let mut thread_id = String::new();
        let mut message_id = String::new();
        let mut draft_id = String::new();
        if let Some(d) = draft.as_mut() {
            d.operation_id = operation.operation_id.clone();
            d.integration_id = account.integration_id.clone();
            d.mail_account_id = account.mail_account_id.clone();
            thread_id = d.thread_id.clone();
            draft_id = d.draft_id.clone();
            artifacts.push(draft_artifact(&operation, d));
        }
        if let Some(m) = message.as_mut() {
            result_mode = ResultMode::Sent;
            m.operation_id = operation.operation_id.clone();
            m.integration_id = account.integration_id.clone();
            m.mail_account_id = account.mail_account_id.clone();
            thread_id = m.thread_id.clone();
            message_id = m.message_id.clone();
            artifacts.push(message_artifact(&operation, m));
        }
        for mut attachment in attachments {
            attachment.operation_id = operation.operation_id.clone();
            artifacts.push(attachment_artifact(&operation, &attachment));
        }
        let mut operation = operation;
        if result_mode == ResultMode::Sent {
            operation.send_path = SendPath::Direct.as_str().to_string();
        }
        let operation = self.complete_operation(
            operation,
            result_mode,
            &thread_id,
            &message_id,
            &draft_id,
            artifacts.clone(),
        );
        Ok((account, draft, message, operation, artifacts))
    }

    pub fn list_operations(&self, filter: &OperationFilter) -> Vec<Operation> {
        let inner = self.inner.read();
        let mut items = Vec::new();
        for id in &inner.op_order {
            let item = &inner.operations[id];
            if !filter.integration_id.is_empty() && item.integration_id != filter.integration_id {
                continue;
            }
            if !filter.run_id.is_empty() && item.run_id != filter.run_id {
                continue;
            }
            if !filter.workflow_id.is_empty() && item.workflow_id != filter.workflow_id {
                continue;
            }
            if !filter.schedule_id.is_empty() && item.schedule_id != filter.schedule_id {
                continue;
            }
            if !filter.delivery_id.is_empty() && item.delivery_id != filter.delivery_id {
                continue;
            }
            if filter.operation_class != OperationClass::default()
                && item.operation_class != filter.operation_class
            {
                continue;
            }
            if filter.status != OperationStatus::default() && item.status != filter.status {
                continue;
            }
            if filter.result_mode != ResultMode::default() && item.result_mode != filter.result_mode
            {
                continue;
            }
            if !filter.thread_id.is_empty() && item.thread_id != filter.thread_id {
                continue;
            }
            if !filter.message_id.is_empty() && item.message_id != filter.message_id {
                continue;
            }
            if !filter.draft_id.is_empty() && item.draft_id != filter.draft_id {
                continue;
            }
            items.push(item.clone());
        }
        items
    }

    pub fn get_operation(&self, operation_id: &str) -> Option<Operation> {
        self.inner
            .read()
            .operations
            .get(operation_id.trim())
            .cloned()
    }

    pub fn get_account(&self, integration_id: &str) -> Option<AccountProjection> {
        self.inner
            .read()
            .accounts
            .get(integration_id.trim())
            .cloned()
    }

    pub fn store_operation(&self, item: Operation) {
        let mut inner = self.inner.write();
        if !inner.operations.contains_key(&item.operation_id) {
            inner.op_order.push(item.operation_id.clone());
        }
        inner.operations.insert(item.operation_id.clone(), item);
    }

    pub fn list_artifacts(&self, operation_id: &str) -> Vec<Artifact> {
        let inner = self.inner.read();
        inner
            .artifacts
            .values()
            .filter(|item| operation_id.is_empty() || item.operation_id == operation_id)
            .cloned()
            .collect()
    }

    fn store_artifact(&self, artifact: Artifact) {
        self.inner
            .write()
            .artifacts
            .insert(artifact.artifact_id.clone(), artifact);
    }

    fn select_account(
        &self,
        resources: &[Resource],
        selection: &Selection,
    ) -> Result<(AccountProjection, Resource, Arc<dyn Backend>, String), MailError> {
        let explicit = selection.integration_id.trim();
        if !explicit.is_empty() {
            for resource in resources {
                if resource.integration_id != explicit {
                    continue;
                }
                return self.project_resource(resource, "explicit");
            }
            return Err(MailError::MailIntegrationNotFound);
        }
        let selected = resources.iter().find(|r| {
            r.domain_kind == "mail" && r.environment_scope.trim() == self.env && r.canonical_default
        });
        match selected {
            Some(resource) => self.project_resource(resource, "canonical_default"),
            None => Err(MailError::MailSelectionInvalid),
        }
    }

    fn project_resource(
        &self,
        resource: &Resource,
        selection_mode: &str,
    ) -> Result<(AccountProjection, Resource, Arc<dyn Backend>, String), MailError> {
        if resource.domain_kind != "mail" || resource.environment_scope.trim() != self.env {
            return Err(MailError::MailSelectionInvalid);
        }
        if resource.readiness_status != ReadinessStatus::Healthy
            && resource.readiness_status != ReadinessStatus::Degraded
        {
            return Err(MailError::MailUnavailable);
        }
        let backend = self
            .inner
            .read()
            .backends
            .get(&resource.backend_binding.backend_kind)
            .cloned();
        let Some(backend) = backend else {
            return Err(MailError::MailBackendNotConfigured);
        };
        if !backend.supports_resource(resource) {
            return Err(MailError::MailUnavailable);
        }
        let mut account = backend.project_account(resource)?;
        account.selection_mode = selection_mode.to_string();
        self.inner
            .write()
            .accounts
            .insert(account.integration_id.clone(), account.clone());
        Ok((
            account,
            resource.clone(),
            backend,
            selection_mode.to_string(),
        ))
    }

    fn new_operation(
        &self,
        account: &AccountProjection,
        resource: &Resource,
        class: OperationClass,
        selection_mode: &str,
        summary: &str,
        source: &SourceLinkage,
    ) -> Operation {
        let now = Utc::now();
        let mut operation_id = source.operation_id.trim().to_string();
        if operation_id.is_empty() {
            operation_id = new_operation_id();
        }
        let operation = Operation {
            operation_id: operation_id.clone(),
            operation_class: class,
            status: OperationStatus::Requested,
            result_mode: ResultMode::Inspection,
            integration_id: resource.integration_id.clone(),
            mail_account_id: account.mail_account_id.clone(),
            environment_scope: account.environment_scope.clone(),
            selection_mode: selection_mode.to_string(),
            request_summary: summary.trim().to_string(),
            background_send_permitted: source.allow_send_side_effects,
            run_id: source.run_id.clone(),
            step_id: source.step_id.clone(),
            tool_call_id: source.tool_call_id.clone(),
            workflow_id: source.workflow_id.clone(),
            workflow_step_id: source.workflow_step_id.clone(),
            schedule_id: source.schedule_id.clone(),
            schedule_attempt_id: source.schedule_attempt_id.clone(),
            delivery_id: source.delivery_id.clone(),
            created_at: now,
            updated_at: now,
            ..Operation::default()
        };
        self.store_operation(operation.clone());
        operation
    }

    fn complete_operation(
        &self,
        mut operation: Operation,
        result_mode: ResultMode,
        thread_id: &str,
        message_id: &str,
        draft_id: &str,
        artifacts: Vec<Artifact>,
    ) -> Operation {
        let now = Utc::now();
        operation.status = OperationStatus::Completed;
        operation.result_mode = result_mode;
        operation.thread_id = thread_id.trim().to_string();
        operation.message_id = message_id.trim().to_string();
        operation.draft_id = draft_id.trim().to_string();
        operation.completed_at = Some(now);
        operation.updated_at = now;
        operation.artifact_ids = collect_artifact_ids(&artifacts);
        let mut inner = self.inner.write();
        for artifact in &artifacts {
            inner
                .artifacts
                .insert(artifact.artifact_id.clone(), artifact.clone());
        }
        if !inner.operations.contains_key(&operation.operation_id) {
            inner.op_order.push(operation.operation_id.clone());
        }
        inner
            .operations
            .insert(operation.operation_id.clone(), operation.clone());
        operation
    }

    fn fail_operation(
        &self,
        mut operation: Operation,
        failure_class: &str,
        provider_kind: &str,
        reason: &str,
    ) -> Operation {
        let now = Utc::now();
        operation.status = OperationStatus::Failed;
        operation.result_mode = ResultMode::Failed;
        operation.failure_class = failure_class.trim().to_string();
        operation.failure_reason = reason.trim().to_string();
        let diagnostic = kura_integrations::diagnostic_failure_for_operation_failure(
            "mail",
            provider_kind,
            &operation.integration_id,
            operation.operation_class.as_str(),
            &operation.failure_class,
            &operation.failure_reason,
            mail_operation_side_effecting(operation.operation_class),
            now,
        );
        operation.diagnostic_failure = Some(diagnostic);
        operation.completed_at = Some(now);
        operation.updated_at = now;
        self.store_operation(operation.clone());
        operation
    }

    fn block_operation(
        &self,
        mut operation: Operation,
        result_mode: ResultMode,
        failure_class: &str,
        reason: &str,
    ) -> Operation {
        let now = Utc::now();
        operation.status = OperationStatus::Blocked;
        operation.result_mode = result_mode;
        operation.failure_class = failure_class.trim().to_string();
        operation.failure_reason = reason.trim().to_string();
        let diagnostic = kura_integrations::diagnostic_failure_for_operation_failure(
            "mail",
            "",
            &operation.integration_id,
            operation.operation_class.as_str(),
            &operation.failure_class,
            &operation.failure_reason,
            mail_operation_side_effecting(operation.operation_class),
            now,
        );
        operation.diagnostic_failure = Some(diagnostic);
        operation.completed_at = Some(now);
        operation.updated_at = now;
        self.store_operation(operation.clone());
        operation
    }

    fn resolve_and_block_on_attachments(
        &self,
        operation: &Operation,
        backend: &Arc<dyn Backend>,
        resource: &Resource,
        account: &AccountProjection,
        refs: &[crate::AttachmentRefInput],
        parent_kind: &str,
        parent_id: &str,
    ) -> (Operation, Vec<Artifact>) {
        let attachments =
            backend.resolve_attachments(resource, account, refs, parent_kind, parent_id);
        for mut attachment in attachments {
            if attachment.resolution_status == AttachmentResolutionStatus::Resolved {
                continue;
            }
            attachment.operation_id = operation.operation_id.clone();
            let artifacts = vec![attachment_artifact(operation, &attachment)];
            let reason = first_non_empty(&[
                &attachment.failure_reason,
                &MailError::MailAttachmentUnresolved.to_string(),
            ]);
            let mut blocked = self.block_operation(
                operation.clone(),
                ResultMode::Blocked,
                "attachment_unresolved",
                &reason,
            );
            blocked.artifact_ids = collect_artifact_ids(&artifacts);
            let mut inner = self.inner.write();
            for artifact in &artifacts {
                inner
                    .artifacts
                    .insert(artifact.artifact_id.clone(), artifact.clone());
            }
            inner
                .operations
                .insert(blocked.operation_id.clone(), blocked.clone());
            return (blocked, artifacts);
        }
        (Operation::default(), Vec::new())
    }
}

fn mail_operation_side_effecting(class: OperationClass) -> bool {
    matches!(
        class,
        OperationClass::SendMessage
            | OperationClass::SendDraft
            | OperationClass::ReplyMessage
            | OperationClass::ForwardMessage
    )
}

#[must_use]
fn failure_class_and_provider(default_class: &str, err: &MailError) -> (String, String) {
    if let MailError::Adapter(af) = err {
        return (af.class.clone(), af.provider_kind.clone());
    }
    (default_class.to_string(), String::new())
}
