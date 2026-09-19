use kura_integrations::{BackendBinding, BackendKind, ReadinessStatus, Resource};
use kura_mail::{
    AttachmentReference, AttachmentResolutionStatus, ComposeMode, CreateDraftInput, DeliveryState,
    DraftStatus, GetThreadInput, ListThreadsInput, MAX_ATTACHMENT_BYTES, MailError, Manager,
    OperationFilter, OperationStatus, ReplyForwardResultMode, ReplyMessageInput, ResultMode,
    Selection, SendDraftInput, SendMessageInput, SourceLinkage, apply_attachment_policy,
    evaluate_attachment, join_recipients, live_validation_matrix_rows, summarize_draft_input,
    validate_explicit_recipients,
};

fn mail_resource(integration_id: &str, env: &str, canonical: bool) -> Resource {
    Resource {
        integration_id: integration_id.to_string(),
        domain_kind: "mail".to_string(),
        environment_scope: env.to_string(),
        readiness_status: ReadinessStatus::Healthy,
        canonical_default: canonical,
        backend_binding: BackendBinding {
            backend_kind: BackendKind::FakeLocal,
            ..BackendBinding::default()
        },
        ..Resource::default()
    }
}

#[test]
fn join_recipients_trims_and_drops_empty() {
    let out = join_recipients(&[
        &["a@x.com".to_string(), "  b@x.com ".to_string()],
        &["".to_string(), "c@x.com".to_string()],
    ]);
    assert_eq!(out, vec!["a@x.com", "b@x.com", "c@x.com"]);
}

#[test]
fn summarize_draft_input_shows_recipients() {
    let to = vec!["a@x.com".to_string()];
    assert_eq!(summarize_draft_input("Hi", &to, &[], &[]), "Hi -> a@x.com");
    assert_eq!(
        summarize_draft_input("Subject only", &[], &[], &[]),
        "Subject only"
    );
}

#[test]
fn validate_explicit_recipients_requires_one() {
    assert!(validate_explicit_recipients(&[]).is_err());
    assert!(matches!(
        validate_explicit_recipients(&["".to_string()]),
        Err(MailError::MailRecipientRequired)
    ));
    assert!(validate_explicit_recipients(&["a@x.com".to_string()]).is_ok());
}

#[test]
fn evaluate_attachment_resolves_standard() {
    let result = evaluate_attachment("report.pdf", "application/pdf", 100);
    assert_eq!(result.status, AttachmentResolutionStatus::Resolved);
    assert_eq!(result.retention_class, "standard");
    assert!(!result.redacted);
}

#[test]
fn evaluate_attachment_blocks_executable_extension() {
    let result = evaluate_attachment("evil.exe", "application/octet-stream", 100);
    assert_eq!(result.status, AttachmentResolutionStatus::Failed);
    assert!(result.failure_reason.contains("unsupported_type"));
}

#[test]
fn evaluate_attachment_blocks_too_large() {
    let result = evaluate_attachment(
        "big.bin",
        "application/octet-stream",
        MAX_ATTACHMENT_BYTES + 1,
    );
    assert_eq!(result.status, AttachmentResolutionStatus::Failed);
    assert!(result.failure_reason.contains("too_large"));
}

#[test]
fn apply_attachment_policy_stamps_reference() {
    let mut reference = AttachmentReference {
        display_name: "script.sh".to_string(),
        media_type: "application/x-sh".to_string(),
        size_bytes: Some(10),
        ..AttachmentReference::default()
    };
    apply_attachment_policy(&mut reference);
    assert_eq!(
        reference.resolution_status,
        AttachmentResolutionStatus::Failed
    );
    assert!(reference.failure_reason.contains("unsupported_type"));
}

#[test]
fn live_validation_rows_cover_mail_classes() {
    assert_eq!(live_validation_matrix_rows().len(), 5);
}

#[test]
fn list_threads_returns_seed() {
    let manager = Manager::new("test");
    let resources = vec![mail_resource("mail_1", "test", true)];
    let input = ListThreadsInput {
        selection: Selection {
            integration_id: "mail_1".to_string(),
        },
        ..ListThreadsInput::default()
    };
    let (account, threads, operation, artifacts) =
        manager.list_threads(&resources, &input).unwrap();
    assert_eq!(account.integration_id, "mail_1");
    assert_eq!(threads.len(), 1);
    assert_eq!(threads[0].thread_id, "thread_seed");
    assert_eq!(operation.status, OperationStatus::Completed);
    assert_eq!(artifacts.len(), 1);
}

#[test]
fn get_thread_missing_records_failed_operation() {
    let manager = Manager::new("test");
    let resources = vec![mail_resource("mail_1", "test", true)];
    let input = GetThreadInput {
        selection: Selection {
            integration_id: "mail_1".to_string(),
        },
        thread_id: "nope".to_string(),
        ..GetThreadInput::default()
    };
    let err = manager.get_thread(&resources, &input).unwrap_err();
    assert!(matches!(err, MailError::MailThreadNotFound));
    let ops = manager.list_operations(&OperationFilter::default());
    assert_eq!(ops.len(), 1);
    assert_eq!(ops[0].status, OperationStatus::Failed);
    assert_eq!(ops[0].failure_class, "not_found");
    assert!(ops[0].diagnostic_failure.is_some());
}

#[test]
fn create_draft_records_draft_only_operation() {
    let manager = Manager::new("test");
    let resources = vec![mail_resource("mail_1", "test", true)];
    let input = CreateDraftInput {
        selection: Selection {
            integration_id: "mail_1".to_string(),
        },
        compose_mode: ComposeMode::NewMessage,
        to: vec!["a@x.com".to_string()],
        subject: "Hi".to_string(),
        body: "Hello".to_string(),
        ..CreateDraftInput::default()
    };
    let (_, draft, operation, _) = manager.create_draft(&resources, &input).unwrap();
    assert_eq!(draft.subject, "Hi");
    assert_eq!(operation.result_mode, ResultMode::DraftOnly);
    assert_eq!(operation.status, OperationStatus::Completed);
}

#[test]
fn send_message_records_sent_operation() {
    let manager = Manager::new("test");
    let resources = vec![mail_resource("mail_1", "test", true)];
    let input = SendMessageInput {
        selection: Selection {
            integration_id: "mail_1".to_string(),
        },
        to: vec!["a@x.com".to_string()],
        subject: "Hi".to_string(),
        body: "Hello".to_string(),
        ..SendMessageInput::default()
    };
    let (_, message, operation, _) = manager.send_message(&resources, &input).unwrap();
    assert_eq!(message.delivery_state, DeliveryState::Sent);
    assert_eq!(operation.result_mode, ResultMode::Sent);
    assert_eq!(operation.send_path, "direct");
}

#[test]
fn send_message_requires_recipients() {
    let manager = Manager::new("test");
    let resources = vec![mail_resource("mail_1", "test", true)];
    let input = SendMessageInput {
        selection: Selection {
            integration_id: "mail_1".to_string(),
        },
        ..SendMessageInput::default()
    };
    let err = manager.send_message(&resources, &input).unwrap_err();
    assert!(matches!(err, MailError::MailRecipientRequired));
    let ops = manager.list_operations(&OperationFilter::default());
    assert_eq!(ops[0].status, OperationStatus::Blocked);
    assert_eq!(ops[0].failure_class, "recipient_required");
}

#[test]
fn send_message_background_blocked_without_permission() {
    let manager = Manager::new("test");
    let resources = vec![mail_resource("mail_1", "test", true)];
    let input = SendMessageInput {
        selection: Selection {
            integration_id: "mail_1".to_string(),
        },
        to: vec!["a@x.com".to_string()],
        source: SourceLinkage {
            workflow_id: "wf_1".to_string(),
            ..SourceLinkage::default()
        },
        ..SendMessageInput::default()
    };
    let err = manager.send_message(&resources, &input).unwrap_err();
    assert!(matches!(err, MailError::MailBackgroundSendBlocked));
    let ops = manager.list_operations(&OperationFilter::default());
    assert_eq!(ops[0].status, OperationStatus::Blocked);
    assert_eq!(ops[0].failure_class, "send_permission_required");
}

#[test]
fn send_draft_sends_seed_draft() {
    let manager = Manager::new("test");
    let resources = vec![mail_resource("mail_1", "test", true)];
    let input = SendDraftInput {
        selection: Selection {
            integration_id: "mail_1".to_string(),
        },
        draft_id: "draft_seed".to_string(),
        ..SendDraftInput::default()
    };
    let (_, draft, message, operation, _) = manager.send_draft(&resources, &input).unwrap();
    assert_eq!(message.delivery_state, DeliveryState::Sent);
    assert_eq!(draft.draft_status, DraftStatus::SentFromDraft);
    assert_eq!(operation.send_path, "draft");
}

#[test]
fn reply_message_draft_mode_returns_draft_only() {
    let manager = Manager::new("test");
    let resources = vec![mail_resource("mail_1", "test", true)];
    let input = ReplyMessageInput {
        selection: Selection {
            integration_id: "mail_1".to_string(),
        },
        message_id: "msg_seed".to_string(),
        result_mode: ReplyForwardResultMode::Draft,
        body: "Reply body".to_string(),
        ..ReplyMessageInput::default()
    };
    let (_, draft, message, operation, _) = manager.reply_message(&resources, &input).unwrap();
    assert!(draft.is_some());
    assert!(message.is_none());
    assert_eq!(operation.result_mode, ResultMode::DraftOnly);
}
