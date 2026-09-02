//! Behavioral tests for the Slack connector runtime (port of runtime_test.go):
//! hosted-setup validation projection + events, inbound normalization and
//! evidence, route-policy failures, final-only DM replies, unready-setup
//! blocking, thread-rooted channel replies, reply-failure diagnostics, and the
//! conformance profile.

use std::collections::HashMap;
use std::sync::Arc;

use chrono::{Duration, TimeZone, Utc};
use kura_checkpoints::Manager as CheckpointManager;
use kura_connectors::{
    DiagnosticReasonCode, RedactionStatus as ConnectorRedactionStatus, Supervisor, SurfaceSupport,
};
use kura_events::{Bus, Filter};
use kura_im::MessageLoop;
use kura_llm::{
    Dispatcher, Provider, ProviderError, ProviderRequest, ProviderResponse, StreamEmitter, Usage,
};
use kura_router::SessionRouter;
use kura_runtime::Manager as RuntimeManager;
use kura_store::SQLiteStore;
use kura_store::slack_setup::{
    SlackConversationRouteRecord, SlackHostedSetupRecord, SlackRoutePolicyRecord,
    SlackWorkspaceBinding,
};
use futures::future::BoxFuture;
use tempfile::TempDir;

use kura_slack::destinations::{
    ConversationRoute, ConversationType, RoutePolicy, RouteValidationState, SelectedChannelState,
};
use kura_slack::readiness::{HostedSetupInput, OAuthState, TerminalState, WorkspaceBinding};
use kura_slack::route::InboundEvent;
use kura_slack::runtime::{Config, conformance_profile, new_runtime};
use kura_slack::transport::{FakeTransport, Transport};

fn ts(y: i32, mo: u32, d: u32, h: u32, mi: u32) -> chrono::DateTime<Utc> {
    Utc.with_ymd_and_hms(y, mo, d, h, mi, 0)
        .single()
        .expect("valid timestamp")
}

// ---------------------------------------------------------------------------
// Test provider (Go slackRuntimeTestProvider): echoes the first message
// content with a reply: prefix.
// ---------------------------------------------------------------------------

struct EchoTestProvider;

impl Provider for EchoTestProvider {
    fn name(&self) -> &str {
        "echo"
    }

    fn complete<'a>(
        &'a self,
        request: ProviderRequest,
    ) -> BoxFuture<'a, Result<ProviderResponse, ProviderError>> {
        Box::pin(async move {
            let content = request
                .messages
                .first()
                .map(|m| m.content.clone())
                .unwrap_or_default();
            Ok(ProviderResponse {
            tool_calls: Vec::new(),
                output: format!("reply:{content}"),
                finish_reason: "stop".to_string(),
                usage: Usage {
                    input_tokens: 1,
                    output_tokens: 1,
                    total_tokens: 2,
                },
            })
        })
    }

    fn stream<'a>(
        &'a self,
        request: ProviderRequest,
        emit: StreamEmitter<'a>,
    ) -> BoxFuture<'a, Result<ProviderResponse, ProviderError>> {
        Box::pin(async move {
            let content = request
                .messages
                .first()
                .map(|m| m.content.clone())
                .unwrap_or_default();
            emit(kura_llm::StreamChunk {
                delta: "reply:".to_string(),
                output: "reply:".to_string(),
                ..kura_llm::StreamChunk::default()
            })?;
            emit(kura_llm::StreamChunk {
                delta: content.clone(),
                output: format!("reply:{content}"),
                finish_reason: "stop".to_string(),
                usage: Some(Usage {
                    input_tokens: 1,
                    output_tokens: 1,
                    total_tokens: 2,
                }),
            })?;
            Ok(ProviderResponse {
            tool_calls: Vec::new(),
                output: format!("reply:{content}"),
                finish_reason: "stop".to_string(),
                usage: Usage {
                    input_tokens: 1,
                    output_tokens: 1,
                    total_tokens: 2,
                },
            })
        })
    }
}

// ---------------------------------------------------------------------------
// Harness + fixtures
// ---------------------------------------------------------------------------

struct Harness {
    _dir: TempDir,
    store: Arc<SQLiteStore>,
    bus: Bus,
    loop_: MessageLoop,
}

/// Builds a store-backed message loop with the echo provider registered
/// (Go newSlackRuntimeTestLoop). The store is intentionally shared through
/// an Arc like the im crate's harness (the connector loop runs on one thread).
#[allow(clippy::arc_with_non_send_sync)]
fn harness() -> Harness {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = Arc::new(SQLiteStore::new(dir.path().to_str().expect("path")).expect("store"));
    let bus = Bus::new();
    let dispatcher = Arc::new(Dispatcher::new());
    dispatcher.register_provider(Arc::new(EchoTestProvider));
    dispatcher
        .set_default_provider("echo")
        .expect("default provider");
    dispatcher.set_default_model("echo-v1");
    let chat = kura_chat::Service::new_service(dispatcher, None, None, Some(bus.clone()), None);
    let runtime = Arc::new(RuntimeManager::new());
    let checkpoints = CheckpointManager::new(
        Arc::new(parking_lot::Mutex::new(
            SQLiteStore::new(dir.path().to_str().expect("path")).expect("store"),
        )),
        runtime.clone(),
    );
    let loop_ = MessageLoop::new(
        SessionRouter::new(),
        runtime.clone(),
        Some(checkpoints),
        Some(bus.clone()),
        store.clone(),
        chat,
    );
    Harness {
        _dir: dir,
        store,
        bus,
        loop_,
    }
}

/// Go seedReadySlackSetup.
fn seed_ready_slack_setup(store: &SQLiteStore, tenant_id: &str, cfg: &Config) {
    let now = ts(2026, 5, 8, 11, 0);
    let workspace_binding_id = if cfg.workspace_binding_id.trim().is_empty() {
        "workspace_binding_redacted".to_string()
    } else {
        cfg.workspace_binding_id.clone()
    };
    let mut policy = SlackRoutePolicyRecord {
        tenant_id: tenant_id.to_string(),
        connector_id: cfg.connector_id.clone(),
        workspace_binding_id: workspace_binding_id.clone(),
        selected_channels: Vec::new(),
        allowed_dm_users: cfg.allowed_dm_user_ids.clone(),
        allowed_dm_user_groups: cfg.allowed_dm_user_groups.clone(),
        mention_gate: "agent_mention_required".to_string(),
        thread_reply_mode: "channel_mentions_thread_rooted".to_string(),
        validation_state: RouteValidationState::Valid.as_str().to_string(),
        reason_code: String::new(),
        validated_at: now,
        redaction_status: ConnectorRedactionStatus::Redacted.as_str().to_string(),
        safe_evidence: HashMap::new(),
    };
    for channel_id in &cfg.allowed_channel_ids {
        policy.selected_channels.push(SlackConversationRouteRecord {
            conversation_id: channel_id.clone(),
            conversation_type: ConversationType::Channel.as_str().to_string(),
            selected_channel_state: SelectedChannelState::Selected.as_str().to_string(),
            validation_state: RouteValidationState::Valid.as_str().to_string(),
            reason_code: String::new(),
            redaction_status: ConnectorRedactionStatus::Redacted.as_str().to_string(),
            safe_evidence: HashMap::new(),
        });
    }
    store
        .save_slack_hosted_setup(&SlackHostedSetupRecord {
            tenant_id: tenant_id.to_string(),
            connector_id: cfg.connector_id.clone(),
            connector_kind: "slack".to_string(),
            display_name: cfg.display_name.clone(),
            status: "healthy".to_string(),
            terminal_state: TerminalState::Ready.as_str().to_string(),
            oauth_state: OAuthState::GrantValid.as_str().to_string(),
            route_policy_state: "valid".to_string(),
            delivery_eligible: true,
            workspace_binding_id: workspace_binding_id.clone(),
            reason_code: "healthy".to_string(),
            redaction_status: ConnectorRedactionStatus::Redacted.as_str().to_string(),
            created_at: now,
            updated_at: now,
            validated_at: Some(now),
            retention_expires_at: now + Duration::days(90),
            workspace_binding: Some(SlackWorkspaceBinding {
                tenant_id: tenant_id.to_string(),
                connector_id: cfg.connector_id.clone(),
                workspace_binding_id: workspace_binding_id.clone(),
                workspace_id: cfg.workspace_id.clone(),
                workspace_label: String::new(),
                installation_id: "installation_redacted".to_string(),
                oauth_grant_state: "valid".to_string(),
                required_scope_state: "valid".to_string(),
                validated_at: now,
                redaction_status: ConnectorRedactionStatus::Redacted.as_str().to_string(),
                safe_evidence: HashMap::new(),
            }),
            route_policy: Some(policy.clone()),
        })
        .expect("save slack hosted setup");
    store
        .save_slack_route_policy(&policy)
        .expect("save slack route policy");
}

fn base_cfg(connector_id: &str, display_name: &str) -> Config {
    Config {
        enabled: true,
        connector_id: connector_id.to_string(),
        display_name: display_name.to_string(),
        tenant_id: "ten_slack".to_string(),
        ..Config::default()
    }
}

fn dm_event(message_id: &str, text: &str, sender_id: &str) -> InboundEvent {
    InboundEvent {
        tenant_id: "ten_slack".to_string(),
        workspace_id: "workspace_redacted".to_string(),
        conversation_id: "dm_redacted".to_string(),
        conversation_type: ConversationType::DirectMessage,
        message_id: message_id.to_string(),
        event_id: format!("event_{message_id}"),
        sender_id: sender_id.to_string(),
        text: text.to_string(),
        received_at: ts(2026, 5, 8, 12, 2),
        ..InboundEvent::default()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

/// Go TestRuntimeRecordsSlackSetupValidationEventAndStoreProjection.
#[test]
fn runtime_records_slack_setup_validation_event_and_store_projection() {
    let harness = harness();
    let cfg = base_cfg("slack-main", "Slack Main");
    let runtime = new_runtime(
        cfg,
        Arc::new(Supervisor::new()),
        harness.loop_,
        Some(harness.store.clone()),
        Some(harness.bus.clone()),
        Some(Arc::new(FakeTransport::new(Vec::new()))),
    )
    .expect("new runtime")
    .expect("enabled runtime");

    let now = ts(2026, 5, 8, 10, 1);
    let setup = runtime
        .record_hosted_setup_validation(HostedSetupInput {
            tenant_id: "ten_slack".to_string(),
            oauth_state: OAuthState::GrantValid,
            provider_available: true,
            network_available: true,
            started_at: now,
            validated_at: now,
            workspace_binding: WorkspaceBinding {
                workspace_id: "workspace_redacted".to_string(),
                installation_id: "installation_redacted".to_string(),
                oauth_grant_state: "valid".to_string(),
                required_scope_state: "valid".to_string(),
                ..WorkspaceBinding::default()
            },
            route_policy: RoutePolicy {
                validation_state: RouteValidationState::Valid,
                selected_channels: vec![ConversationRoute {
                    conversation_id: "channel_redacted".to_string(),
                    conversation_type: ConversationType::Channel,
                    selected_channel_state: SelectedChannelState::Selected,
                    validation_state: RouteValidationState::Valid,
                    ..ConversationRoute::default()
                }],
                ..RoutePolicy::default()
            },
            ..HostedSetupInput::default()
        })
        .expect("record hosted setup validation");
    assert_eq!(
        setup.terminal_state,
        TerminalState::Ready,
        "expected ready setup"
    );

    let stored = harness
        .store
        .get_slack_hosted_setup("ten_slack", "slack-main")
        .expect("get stored setup")
        .expect("stored setup exists");
    assert_eq!(stored.terminal_state, "ready");
    assert!(
        stored.route_policy.is_some(),
        "expected route policy projection"
    );
    assert_eq!(
        stored
            .workspace_binding
            .as_ref()
            .map(|b| b.workspace_id.as_str()),
        Some("workspace_redacted"),
        "expected workspace binding to be retained"
    );

    let published = harness.bus.list(&Filter {
        category: "connector".to_string(),
        ..Filter::default()
    });
    assert_eq!(published.len(), 1, "expected Slack setup validation event");
    assert_eq!(published[0].name, "connector.slack_setup_validated");
    assert_eq!(
        published[0]
            .payload
            .get("redactionStatus")
            .and_then(|v| v.as_str()),
        Some("redacted")
    );
    assert_eq!(
        published[0]
            .payload
            .get("routePolicyState")
            .and_then(|v| v.as_str()),
        Some("valid")
    );
}

/// Go TestRuntimeNormalizesSlackInboundAndRecordsEvidence.
#[test]
fn runtime_normalizes_slack_inbound_and_records_evidence() {
    let harness = harness();
    let mut cfg = base_cfg("slack-main", "Slack Main");
    cfg.workspace_id = "workspace_redacted".to_string();
    cfg.bot_user_id = "bot_redacted".to_string();
    cfg.allowed_channel_ids = vec!["channel_selected".to_string()];
    cfg.allowed_dm_user_ids = vec!["user_allowed".to_string()];
    cfg.allowed_dm_user_groups = vec!["group_allowed".to_string()];
    seed_ready_slack_setup(&harness.store, "ten_slack", &cfg);
    let runtime = new_runtime(
        cfg.clone(),
        Arc::new(Supervisor::new()),
        harness.loop_,
        Some(harness.store.clone()),
        Some(harness.bus.clone()),
        Some(Arc::new(FakeTransport::new(Vec::new()))),
    )
    .expect("new runtime")
    .expect("enabled runtime");

    let inbound = runtime
        .normalize_inbound_event(InboundEvent {
            tenant_id: "ten_slack".to_string(),
            workspace_id: "workspace_redacted".to_string(),
            conversation_id: "channel_selected".to_string(),
            conversation_type: ConversationType::Channel,
            message_id: "message_1".to_string(),
            event_id: "event_1".to_string(),
            sender_id: "user_allowed".to_string(),
            text: "<@bot_redacted> hello".to_string(),
            received_at: ts(2026, 5, 8, 12, 0),
            ..InboundEvent::default()
        })
        .expect("accepted inbound");
    assert_eq!(inbound.content, "hello");
    assert_eq!(
        inbound.equivalent_rule_id,
        "slack_workspace_conversation_message_id"
    );

    let duplicate = runtime.normalize_inbound_event(InboundEvent {
        tenant_id: "ten_slack".to_string(),
        workspace_id: "workspace_redacted".to_string(),
        conversation_id: "channel_selected".to_string(),
        conversation_type: ConversationType::Channel,
        message_id: "message_1".to_string(),
        event_id: "event_2".to_string(),
        sender_id: "user_allowed".to_string(),
        text: "<@bot_redacted> hello again".to_string(),
        mentioned: true,
        received_at: ts(2026, 5, 8, 12, 1),
        ..InboundEvent::default()
    });
    assert!(duplicate.is_none(), "duplicate should be suppressed");

    let evidence = harness
        .store
        .list_slack_event_evidence("ten_slack", "slack-main", ts(2026, 5, 8, 12, 0), 10)
        .expect("list event evidence");
    assert_eq!(evidence.len(), 2, "expected two event evidence rows");
    assert_eq!(evidence[0].route_outcome, "duplicate");
    assert_eq!(evidence[1].route_outcome, "accepted");

    let published = harness.bus.list(&Filter {
        category: "connector".to_string(),
        ..Filter::default()
    });
    assert_eq!(
        published.len(),
        2,
        "expected accepted and duplicate route events"
    );
    assert_eq!(
        published[1].payload.get("outcome").and_then(|v| v.as_str()),
        Some("duplicate")
    );
}

/// Go TestRuntimeAppliesSlackRoutePolicyFailures.
#[test]
fn runtime_applies_slack_route_policy_failures() {
    let harness = harness();
    let mut cfg = base_cfg("slack-main", "Slack Main");
    cfg.workspace_id = "workspace_redacted".to_string();
    cfg.bot_user_id = "bot_redacted".to_string();
    cfg.allowed_channel_ids = vec!["channel_selected".to_string()];
    cfg.allowed_dm_user_ids = vec!["user_allowed".to_string()];
    cfg.allowed_dm_user_groups = vec!["group_allowed".to_string()];
    let runtime = new_runtime(
        cfg,
        Arc::new(Supervisor::new()),
        harness.loop_,
        None,
        None,
        Some(Arc::new(FakeTransport::new(Vec::new()))),
    )
    .expect("new runtime")
    .expect("enabled runtime");

    let cases: Vec<(&str, InboundEvent, bool)> = vec![
        (
            "allowed dm",
            InboundEvent {
                tenant_id: "ten_slack".to_string(),
                workspace_id: "workspace_redacted".to_string(),
                conversation_id: "dm_1".to_string(),
                conversation_type: ConversationType::DirectMessage,
                message_id: "dm_1".to_string(),
                sender_id: "user_allowed".to_string(),
                text: "hello".to_string(),
                ..InboundEvent::default()
            },
            true,
        ),
        (
            "allowed dm user group",
            InboundEvent {
                tenant_id: "ten_slack".to_string(),
                workspace_id: "workspace_redacted".to_string(),
                conversation_id: "dm_2".to_string(),
                conversation_type: ConversationType::DirectMessage,
                message_id: "dm_2".to_string(),
                sender_id: "user_other".to_string(),
                sender_user_group_ids: vec!["group_allowed".to_string()],
                text: "hello".to_string(),
                ..InboundEvent::default()
            },
            true,
        ),
        (
            "blocked dm",
            InboundEvent {
                tenant_id: "ten_slack".to_string(),
                workspace_id: "workspace_redacted".to_string(),
                conversation_id: "dm_3".to_string(),
                conversation_type: ConversationType::DirectMessage,
                message_id: "dm_3".to_string(),
                sender_id: "user_other".to_string(),
                text: "hello".to_string(),
                ..InboundEvent::default()
            },
            false,
        ),
        (
            "selected channel mention",
            InboundEvent {
                tenant_id: "ten_slack".to_string(),
                workspace_id: "workspace_redacted".to_string(),
                conversation_id: "channel_selected".to_string(),
                conversation_type: ConversationType::Channel,
                message_id: "chan_1".to_string(),
                text: "<@bot_redacted> hello".to_string(),
                ..InboundEvent::default()
            },
            true,
        ),
        (
            "selected channel no mention",
            InboundEvent {
                tenant_id: "ten_slack".to_string(),
                workspace_id: "workspace_redacted".to_string(),
                conversation_id: "channel_selected".to_string(),
                conversation_type: ConversationType::Channel,
                message_id: "chan_2".to_string(),
                text: "hello".to_string(),
                ..InboundEvent::default()
            },
            false,
        ),
        (
            "wrong workspace",
            InboundEvent {
                tenant_id: "ten_slack".to_string(),
                workspace_id: "workspace_other".to_string(),
                conversation_id: "channel_selected".to_string(),
                conversation_type: ConversationType::Channel,
                message_id: "chan_3".to_string(),
                mentioned: true,
                text: "hello".to_string(),
                ..InboundEvent::default()
            },
            false,
        ),
        (
            "unsupported file",
            InboundEvent {
                tenant_id: "ten_slack".to_string(),
                workspace_id: "workspace_redacted".to_string(),
                conversation_id: "channel_selected".to_string(),
                conversation_type: ConversationType::Channel,
                message_id: "chan_4".to_string(),
                surface: "file".to_string(),
                mentioned: true,
                ..InboundEvent::default()
            },
            false,
        ),
    ];
    for (name, event, want) in cases {
        let ok = runtime.normalize_inbound_event(event).is_some();
        assert_eq!(ok, want, "{name}");
    }
}

/// Go TestRuntimeSendsFinalOnlySlackDirectMessageReply.
#[test]
fn runtime_sends_final_only_slack_direct_message_reply() {
    let harness = harness();
    let transport = Arc::new(FakeTransport::new(vec![dm_event(
        "message_dm_1",
        "hello",
        "user_allowed",
    )]));
    let mut cfg = base_cfg("slack-main", "Slack Main");
    cfg.workspace_id = "workspace_redacted".to_string();
    cfg.workspace_binding_id = "workspace_binding_redacted".to_string();
    cfg.allowed_dm_user_ids = vec!["user_allowed".to_string()];
    seed_ready_slack_setup(&harness.store, "ten_slack", &cfg);
    let runtime = new_runtime(
        cfg,
        Arc::new(Supervisor::new()),
        harness.loop_,
        Some(harness.store.clone()),
        Some(harness.bus.clone()),
        Some(transport.clone()),
    )
    .expect("new runtime")
    .expect("enabled runtime");

    runtime.start().expect("start");
    let replies = transport.sent_replies();
    assert_eq!(replies.len(), 1, "expected one final-only reply");
    assert_eq!(replies[0].channel_id, "dm_redacted");
    assert_eq!(replies[0].content, "reply:hello");
    assert_eq!(replies[0].reply_to_external_message_id, "message_dm_1");
    let caps = transport.reply_capabilities();
    assert!(
        !caps.supports_streaming,
        "expected Slack fake transport to be final-only"
    );
    assert!(!caps.supports_thinking);
}

/// Go TestRuntimeBlocksInboundWhenHostedSetupIsNotReady.
#[test]
fn runtime_blocks_inbound_when_hosted_setup_is_not_ready() {
    let harness = harness();
    let mut cfg = base_cfg("slack-main", "Slack Main");
    cfg.workspace_id = "workspace_redacted".to_string();
    cfg.workspace_binding_id = "workspace_binding_redacted".to_string();
    cfg.allowed_dm_user_ids = vec!["user_allowed".to_string()];
    let transport = Arc::new(FakeTransport::new(Vec::new()));
    let runtime = new_runtime(
        cfg,
        Arc::new(Supervisor::new()),
        harness.loop_,
        Some(harness.store.clone()),
        Some(harness.bus.clone()),
        Some(transport.clone()),
    )
    .expect("new runtime")
    .expect("enabled runtime");

    let inbound =
        runtime.normalize_inbound_event(dm_event("message_dm_unready", "hello", "user_allowed"));
    assert!(
        inbound.is_none(),
        "expected unready hosted setup to block inbound"
    );

    let evidence = harness
        .store
        .list_slack_event_evidence("ten_slack", "slack-main", ts(2026, 5, 8, 12, 3), 10)
        .expect("list event evidence");
    assert_eq!(evidence.len(), 1, "expected one blocked route evidence row");
    assert_eq!(evidence[0].route_outcome, "blocked");
    assert_eq!(evidence[0].reason_code, "auth_missing");
}

/// Go TestRuntimeSendsSlackChannelMentionReplyRootedAtTriggerThread.
#[test]
fn runtime_sends_slack_channel_mention_reply_rooted_at_trigger_thread() {
    let harness = harness();
    let transport = Arc::new(FakeTransport::new(vec![InboundEvent {
        tenant_id: "ten_slack".to_string(),
        workspace_id: "workspace_redacted".to_string(),
        conversation_id: "channel_selected".to_string(),
        conversation_type: ConversationType::Channel,
        message_id: "message_thread_reply".to_string(),
        thread_root_message_id: "message_thread_root".to_string(),
        event_id: "event_channel_1".to_string(),
        sender_id: "user_allowed".to_string(),
        text: "<@bot_redacted> summarize".to_string(),
        received_at: ts(2026, 5, 8, 12, 3),
        ..InboundEvent::default()
    }]));
    let mut cfg = base_cfg("slack-main", "Slack Main");
    cfg.workspace_id = "workspace_redacted".to_string();
    cfg.workspace_binding_id = "workspace_binding_redacted".to_string();
    cfg.bot_user_id = "bot_redacted".to_string();
    cfg.allowed_channel_ids = vec!["channel_selected".to_string()];
    seed_ready_slack_setup(&harness.store, "ten_slack", &cfg);
    let runtime = new_runtime(
        cfg,
        Arc::new(Supervisor::new()),
        harness.loop_,
        Some(harness.store.clone()),
        Some(harness.bus.clone()),
        Some(transport.clone()),
    )
    .expect("new runtime")
    .expect("enabled runtime");

    runtime.start().expect("start");
    let replies = transport.sent_replies();
    assert_eq!(replies.len(), 1, "expected one channel reply");
    assert_eq!(replies[0].channel_id, "channel_selected");
    assert_eq!(replies[0].content, "reply:summarize");
    assert_eq!(
        replies[0].reply_to_external_message_id, "message_thread_root",
        "expected thread-rooted Slack reply"
    );
}

/// Go TestRuntimeRecordsSlackReplyFailureSeparatelyFromAssistantExecution.
#[test]
fn runtime_records_slack_reply_failure_separately_from_assistant_execution() {
    let harness = harness();
    let transport = Arc::new(FakeTransport::new(vec![dm_event(
        "message_dm_fail",
        "hello",
        "user_allowed",
    )]));
    transport.set_reply_error("slack 5xx transport failure".to_string());
    let mut cfg = base_cfg("slack-main", "Slack Main");
    cfg.workspace_id = "workspace_redacted".to_string();
    cfg.workspace_binding_id = "workspace_binding_redacted".to_string();
    cfg.allowed_dm_user_ids = vec!["user_allowed".to_string()];
    seed_ready_slack_setup(&harness.store, "ten_slack", &cfg);
    let runtime = new_runtime(
        cfg,
        Arc::new(Supervisor::new()),
        harness.loop_,
        Some(harness.store.clone()),
        Some(harness.bus.clone()),
        Some(transport.clone()),
    )
    .expect("new runtime")
    .expect("enabled runtime");

    runtime.start().expect("start");
    let connector_events = harness.bus.list(&Filter {
        category: "connector".to_string(),
        ..Filter::default()
    });
    let failed = connector_events
        .iter()
        .find(|event| event.name == "connector.reply_failed")
        .expect("expected connector.reply_failed event");
    assert_eq!(
        failed
            .payload
            .get("assistantExecutionOutcome")
            .and_then(|v| v.as_str()),
        Some("succeeded")
    );
    assert_eq!(
        failed
            .payload
            .get("connectorDeliveryOutcome")
            .and_then(|v| v.as_str()),
        Some("failed")
    );
    assert_eq!(
        failed.payload.get("connectorKind").and_then(|v| v.as_str()),
        Some("slack")
    );

    let diagnostics = harness
        .store
        .list_connector_diagnostic_states("ten_slack", "slack-main", ts(2026, 5, 8, 12, 5))
        .expect("list diagnostics");
    assert_eq!(diagnostics.len(), 1, "expected one diagnostic state");
    assert_eq!(
        diagnostics[0].reason_code,
        DiagnosticReasonCode::ProviderUnavailable
    );
}

/// Go TestConformanceProfileDeclaresSlackUnsupportedSurfaces.
#[test]
fn conformance_profile_declares_slack_unsupported_surfaces() {
    let profile = conformance_profile(
        &Config {
            connector_id: "slack-main".to_string(),
            allowed_channel_ids: vec!["channel_selected".to_string()],
            allowed_dm_user_groups: vec!["group_allowed".to_string()],
            ..Config::default()
        },
        ts(2026, 5, 8, 13, 30),
    );
    assert_eq!(profile.connector_kind, "slack");
    for surface in [
        "marketplace_publication",
        "enterprise_grid_administration",
        "memory_based_team_context",
        "files",
        "voice_huddles",
        "canvases",
        "workflow_buttons",
        "interactive_blocks",
        "rich_media",
        "thinking_visibility",
        "incremental_visible_updates",
    ] {
        assert_eq!(
            profile.provider_surface_results.get(surface),
            Some(&SurfaceSupport::Unsupported),
            "surface {surface} must be unsupported"
        );
    }
    assert_eq!(
        profile
            .provider_surface_results
            .get("selected_channel_mention"),
        Some(&SurfaceSupport::Supported)
    );
    assert_eq!(
        profile.provider_surface_results.get("direct_message"),
        Some(&SurfaceSupport::Supported)
    );
}

/// Go NewRuntime: a disabled connector produces no runtime.
#[test]
fn new_runtime_disabled_returns_none() {
    let harness = harness();
    let cfg = Config {
        enabled: false,
        connector_id: "slack-main".to_string(),
        ..Config::default()
    };
    let runtime = new_runtime(
        cfg,
        Arc::new(Supervisor::new()),
        harness.loop_,
        None,
        None,
        None,
    )
    .expect("new runtime");
    assert!(runtime.is_none());
}

/// The fake transport reports final-only capabilities and round-trips replies.
#[test]
fn fake_transport_round_trips_replies() {
    let transport = FakeTransport::new(Vec::new());
    let reply = kura_imtypes::OutboundReply {
        connector_id: "slack-main".to_string(),
        channel_id: "dm_redacted".to_string(),
        content: "hello".to_string(),
        ..kura_imtypes::OutboundReply::default()
    };
    let sent = transport.send_reply(&reply).expect("send reply");
    assert_eq!(sent.external_message_id, "slack_reply_dm_redacted");
    assert_eq!(transport.sent_replies(), vec![reply]);
}
