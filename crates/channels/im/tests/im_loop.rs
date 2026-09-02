//! Behavioral tests for the connector MessageLoop (port of
//! daemon/internal/im/loop_test.go). The chat service is built with the
//! dispatcher + event bus and no store handle, so continuity assembly is
//! skipped and the injected providers drive deterministic replies.

use std::sync::atomic::{AtomicI32, Ordering as AtomicOrdering};
use std::sync::{Arc, Mutex};

use chrono::Utc;
use kura_chat::{CancellationToken, Service};
use kura_checkpoints::Manager as CheckpointManager;
use kura_connectors::{Connector, RedactionStatus, Status};
use kura_events::{Bus, Filter};
use kura_im::{MessageLoop, ReplyProgressor, ReplySender};
use kura_imtypes::{
    DeliveryDirection, DeliveryStatus, InboundMessage, OutboundReply, ReplyCapabilities,
    ReplyEdit, SentReply, ThinkingSignal,
};
use kura_llm::{
    Dispatcher, Provider, ProviderError, ProviderRequest, ProviderResponse, StreamEmitter,
    Usage,
};
use kura_router::{SessionKind, SessionRouter};
use kura_runtime::{Manager as RuntimeManager, RunStatus, StepStatus};
use kura_store::channel_management::RoutePolicy;
use kura_store::SQLiteStore;
use kura_threads::{
    LifecycleActionKind, LifecycleMutationInput, LifecycleState, ParticipationDecisionValue,
    RoutingOutcome,
};
use futures::future::BoxFuture;

// ---------------------------------------------------------------------------
// Test providers (Go loopTestProvider / loopChunkedProvider / loopLongProvider /
// loopPartialFailureProvider)
// ---------------------------------------------------------------------------

/// Go loopTestProvider: echoes the first message content with a reply: prefix.
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
                usage: Usage { input_tokens: 1, output_tokens: 1, total_tokens: 2 },
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
                ..Default::default()
            })?;
            emit(kura_llm::StreamChunk {
                delta: content.clone(),
                output: format!("reply:{content}"),
                finish_reason: "stop".to_string(),
                usage: Some(Usage { input_tokens: 1, output_tokens: 1, total_tokens: 2 }),
                ..Default::default()
            })?;
            Ok(ProviderResponse {
            tool_calls: Vec::new(),
                output: format!("reply:{content}"),
                finish_reason: "stop".to_string(),
                usage: Usage { input_tokens: 1, output_tokens: 1, total_tokens: 2 },
            })
        })
    }
}

/// Go loopLongProvider: streams the output in three segments.
struct LongTestProvider;

impl Provider for LongTestProvider {
    fn name(&self) -> &str {
        "echo"
    }

    fn complete<'a>(
        &'a self,
        request: ProviderRequest,
    ) -> BoxFuture<'a, Result<ProviderResponse, ProviderError>> {
        let output = request
            .messages
            .first()
            .map(|m| format!("reply:{}", m.content))
            .unwrap_or_default();
        Box::pin(async move {
            let runes = output.chars().count() as i64;
            Ok(ProviderResponse {
            tool_calls: Vec::new(),
                output,
                finish_reason: "stop".to_string(),
                usage: Usage { input_tokens: 1, output_tokens: runes, total_tokens: runes + 1 },
            })
        })
    }

    fn stream<'a>(
        &'a self,
        request: ProviderRequest,
        emit: StreamEmitter<'a>,
    ) -> BoxFuture<'a, Result<ProviderResponse, ProviderError>> {
        let output = request
            .messages
            .first()
            .map(|m| format!("reply:{}", m.content))
            .unwrap_or_default();
        Box::pin(async move {
            let runes: Vec<char> = output.chars().collect();
            let segments = [
                runes[..12].iter().collect::<String>(),
                runes[12..24].iter().collect::<String>(),
                runes[24..].iter().collect::<String>(),
            ];
            for segment in segments {
                emit(kura_llm::StreamChunk {
                    delta: segment,
                    ..Default::default()
                })?;
            }
            let count = runes.len() as i64;
            Ok(ProviderResponse {
            tool_calls: Vec::new(),
                output,
                finish_reason: "stop".to_string(),
                usage: Usage { input_tokens: 1, output_tokens: count, total_tokens: count + 1 },
            })
        })
    }
}

/// Go loopPartialFailureProvider: streams visible output, then fails with an
/// idle-timeout provider error.
struct PartialFailureTestProvider;

impl Provider for PartialFailureTestProvider {
    fn name(&self) -> &str {
        "echo"
    }

    fn complete<'a>(
        &'a self,
        _request: ProviderRequest,
    ) -> BoxFuture<'a, Result<ProviderResponse, ProviderError>> {
        Box::pin(async { Err(ProviderError::provider("idle_timeout", "stream stalled", true)) })
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
                ..Default::default()
            })?;
            emit(kura_llm::StreamChunk {
                delta: content.clone(),
                output: format!("reply:{content}"),
                ..Default::default()
            })?;
            Err(ProviderError::provider("idle_timeout", "stream stalled", true))
        })
    }
}

// ---------------------------------------------------------------------------
// Reply senders (Go loopTestReplySender / loopProgressReplySender)
// ---------------------------------------------------------------------------

/// Go loopTestReplySender.
struct TestReplySender {
    last: Mutex<Option<OutboundReply>>,
    err: Option<String>,
}

impl Default for TestReplySender {
    fn default() -> Self {
        TestReplySender { last: Mutex::new(None), err: None }
    }
}

impl TestReplySender {
    fn last(&self) -> Option<OutboundReply> {
        self.last.lock().expect("lock").clone()
    }
}

impl ReplySender for TestReplySender {
    fn send_reply(&self, reply: OutboundReply) -> Result<SentReply, String> {
        *self.last.lock().expect("lock") = Some(reply);
        if let Some(err) = &self.err {
            return Err(err.clone());
        }
        Ok(SentReply { external_message_id: "discord_reply_1".to_string() })
    }
}

/// Go loopProgressReplySender.
struct ProgressReplySender {
    sent: Mutex<Vec<OutboundReply>>,
    edited: Mutex<Vec<ReplyEdit>>,
    thinking: Mutex<Vec<ThinkingSignal>>,
    edit_err: Option<String>,
    max_len: i64,
    next_id: AtomicI32,
}

impl Default for ProgressReplySender {
    fn default() -> Self {
        ProgressReplySender {
            sent: Mutex::new(Vec::new()),
            edited: Mutex::new(Vec::new()),
            thinking: Mutex::new(Vec::new()),
            edit_err: None,
            max_len: 0,
            next_id: AtomicI32::new(0),
        }
    }
}

impl ProgressReplySender {
    fn sent(&self) -> Vec<OutboundReply> {
        self.sent.lock().expect("lock").clone()
    }

    fn edited(&self) -> Vec<ReplyEdit> {
        self.edited.lock().expect("lock").clone()
    }

    fn thinking(&self) -> Vec<ThinkingSignal> {
        self.thinking.lock().expect("lock").clone()
    }
}

impl ReplySender for ProgressReplySender {
    fn send_reply(&self, reply: OutboundReply) -> Result<SentReply, String> {
        let id = self.next_id.fetch_add(1, AtomicOrdering::SeqCst) + 1;
        self.sent.lock().expect("lock").push(reply);
        Ok(SentReply { external_message_id: format!("discord_reply_{id}") })
    }

    fn reply_progressor(&self) -> Option<&dyn ReplyProgressor> {
        Some(self)
    }
}

impl ReplyProgressor for ProgressReplySender {
    fn reply_capabilities(&self) -> ReplyCapabilities {
        let max_len = if self.max_len <= 0 { 2000 } else { self.max_len };
        ReplyCapabilities {
            supports_thinking: true,
            supports_streaming: true,
            max_message_length: max_len,
        }
    }

    fn send_thinking(&self, signal: ThinkingSignal) -> Result<(), String> {
        self.thinking.lock().expect("lock").push(signal);
        Ok(())
    }

    fn edit_reply(&self, edit: ReplyEdit) -> Result<(), String> {
        if let Some(err) = &self.edit_err {
            return Err(err.clone());
        }
        self.edited.lock().expect("lock").push(edit);
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Harness + fixtures
// ---------------------------------------------------------------------------

/// Builds a store-backed loop with the given provider registered as "echo".
fn test_harness(
    provider: Arc<dyn Provider>,
) -> (Arc<SQLiteStore>, MessageLoop, Bus, Arc<RuntimeManager>, tempfile::TempDir) {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = Arc::new(SQLiteStore::new(dir.path().to_str().expect("path")).expect("store"));
    let bus = Bus::new();
    let dispatcher = Arc::new(Dispatcher::new());
    dispatcher.register_provider(provider);
    dispatcher.set_default_provider("echo").expect("default provider");
    dispatcher.set_default_model("echo-v1");
    let chat = Service::new_service(dispatcher, None, None, Some(bus.clone()), None);
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
    (store, loop_, bus, runtime, dir)
}

fn discord_connector() -> Connector {
    Connector {
        connector_id: "discord-main".to_string(),
        kind: "discord".to_string(),
        display_name: "Discord Main".to_string(),
        status: Status::Healthy,
        created_at: Utc::now(),
        updated_at: Utc::now(),
        ..Connector::default()
    }
}

fn discord_inbound(external_id: &str, content: &str) -> InboundMessage {
    InboundMessage {
        connector_id: "discord-main".to_string(),
        connector_kind: "discord".to_string(),
        external_message_id: external_id.to_string(),
        account_id: "bot_1".to_string(),
        channel_id: "dm_1".to_string(),
        peer_id: "user_1".to_string(),
        author_id: "user_1".to_string(),
        content: content.to_string(),
        kind: SessionKind::Direct,
        direct: true,
        received_at: Utc::now(),
        ..InboundMessage::default()
    }
}

fn slack_connector(tenant_id: &str) -> Connector {
    Connector {
        tenant_id: tenant_id.to_string(),
        connector_id: "slack-main".to_string(),
        kind: "slack".to_string(),
        display_name: "Slack Main".to_string(),
        status: Status::Healthy,
        created_at: Utc::now(),
        updated_at: Utc::now(),
        ..Connector::default()
    }
}

fn slack_group_inbound(external_id: &str, content: &str, tenant_id: &str) -> InboundMessage {
    InboundMessage {
        tenant_id: tenant_id.to_string(),
        connector_id: "slack-main".to_string(),
        connector_kind: "slack".to_string(),
        external_message_id: external_id.to_string(),
        account_id: "workspace_redacted".to_string(),
        connector_account_id: "workspace_redacted".to_string(),
        channel_id: "channel_redacted".to_string(),
        channel_or_conversation_id: "channel_redacted".to_string(),
        provider_message_id: external_id.to_string(),
        equivalent_rule_id: "slack_workspace_conversation_message_id".to_string(),
        peer_id: "channel_redacted".to_string(),
        thread_id: "provider_thread_root".to_string(),
        author_id: "user_1".to_string(),
        content: content.to_string(),
        kind: SessionKind::Group,
        received_at: Utc::now(),
        ..InboundMessage::default()
    }
}

// ---------------------------------------------------------------------------
// Tests (Go TestMessageLoopProcessesSingleTurnAndDeduplicates)
// ---------------------------------------------------------------------------

#[test]
fn message_loop_processes_single_turn_and_deduplicates() {
    let (_store, loop_, _bus, _runtime, _dir) = test_harness(Arc::new(EchoTestProvider));
    let sender = TestReplySender::default();
    let cancel = CancellationToken::new();
    let connector = discord_connector();
    let inbound = discord_inbound("discord_msg_1", "hello");

    let result = loop_
        .process_single_turn(&connector, &inbound, &sender, &cancel)
        .expect("process first turn");
    assert!(!result.duplicate, "first inbound must not be deduplicated");
    assert_eq!(result.run.status, RunStatus::Completed);
    assert_eq!(result.step.status, StepStatus::Completed);
    assert_eq!(sender.last().expect("reply").content, "reply:hello");

    let second = loop_
        .process_single_turn(&connector, &inbound, &sender, &cancel)
        .expect("process duplicate turn");
    assert!(second.duplicate, "duplicate inbound must be ignored");
    assert_eq!(second.outcome, "duplicate");
    assert_eq!(second.reason_code, "duplicate_inbound");
}

// ---------------------------------------------------------------------------
// Go TestMessageLoopRecordsThreadLifecycleEvidenceForAcceptedDuplicateAndBlocked
// ---------------------------------------------------------------------------

#[test]
fn message_loop_records_thread_lifecycle_evidence_for_accepted_duplicate_and_blocked() {
    let (store, loop_, _bus, _runtime, _dir) = test_harness(Arc::new(EchoTestProvider));
    let sender = TestReplySender::default();
    let cancel = CancellationToken::new();
    let connector = slack_connector("ten_thread");
    let inbound = slack_group_inbound("slack_msg_thread_1", "hello", "ten_thread");

    let result = loop_
        .process_single_turn(&connector, &inbound, &sender, &cancel)
        .expect("accepted turn");
    assert_eq!(result.outcome, "accepted");

    let persisted = store
        .get_connector_message_by_external_id_for_tenant(
            "ten_thread",
            "slack-main",
            DeliveryDirection::Inbound,
            "slack_msg_thread_1",
        )
        .expect("get inbound")
        .expect("found");
    assert!(!persisted.thread_id.is_empty(), "accepted message must bind a thread");
    assert!(
        !persisted.thread_session_segment_id.is_empty(),
        "accepted message must bind a session segment"
    );

    let detail = store
        .get_thread_detail_for_tenant("ten_thread", &persisted.thread_id)
        .expect("thread detail")
        .expect("found");
    assert_eq!(detail.source_linkages.len(), 1, "expected one accepted source linkage");
    assert_eq!(
        detail.source_linkages[0].routing_outcome,
        RoutingOutcome::Accepted,
        "expected accepted source linkage"
    );
    assert!(
        detail.runtime_projections.len() >= 3,
        "expected session/run/message runtime projections, got {}",
        detail.runtime_projections.len()
    );

    let duplicate = loop_
        .process_single_turn(&connector, &inbound, &sender, &cancel)
        .expect("duplicate turn");
    assert!(duplicate.duplicate, "expected duplicate to be suppressed");
    let detail = store
        .get_thread_detail_for_tenant("ten_thread", &persisted.thread_id)
        .expect("thread detail duplicate")
        .expect("found");
    assert!(
        detail
            .source_linkages
            .iter()
            .any(|linkage| linkage.routing_outcome == RoutingOutcome::Duplicate),
        "expected duplicate source linkage"
    );

    let archive = store
        .apply_thread_lifecycle_action(
            "ten_thread",
            &persisted.thread_id,
            LifecycleActionKind::Archive,
            &LifecycleMutationInput {
                actor_principal_id: "prn_1".to_string(),
                reason_code: String::new(),
                audit_event_id: "audit_archive_thread".to_string(),
                now: Some(Utc::now()),
                new_segment_id: String::new(),
            },
        )
        .expect("archive thread")
        .expect("found");
    assert_eq!(archive.thread.lifecycle_state, LifecycleState::Archived);

    let mut blocked_inbound = inbound.clone();
    blocked_inbound.external_message_id = "slack_msg_thread_2".to_string();
    blocked_inbound.provider_message_id = "slack_msg_thread_2".to_string();
    blocked_inbound.content = "blocked".to_string();
    let blocked = loop_
        .process_single_turn(&connector, &blocked_inbound, &sender, &cancel)
        .expect("blocked turn");
    assert_eq!(blocked.outcome, "blocked");
    assert_eq!(blocked.reason_code, "thread_archived");
    assert!(blocked.run.run_id.is_empty(), "archived-thread continuation must not create a run");
}

// ---------------------------------------------------------------------------
// Go TestMessageLoopAppliesGroupRoomParticipationPolicyBeforeAssistantWork
// ---------------------------------------------------------------------------

#[test]
fn message_loop_applies_group_room_participation_policy_before_assistant_work() {
    let (store, loop_, _bus, _runtime, _dir) = test_harness(Arc::new(EchoTestProvider));
    let sender = TestReplySender::default();
    let cancel = CancellationToken::new();

    let mut connector = slack_connector("ten_participation");
    connector.capability_profile = serde_json::json!({
        "group_room_mention_evidence": "supported",
        "group_room_allowlist_evidence": "supported",
    })
    .as_object()
    .expect("object")
    .clone();

    store
        .save_channel_route_policy(&RoutePolicy {
            route_policy_id: "route_policy_participation".to_string(),
            tenant_id: "ten_participation".to_string(),
            connector_id: "slack-main".to_string(),
            eligible_senders: Vec::new(),
            eligible_conversations: Vec::new(),
            eligible_rooms: vec!["channel_redacted".to_string()],
            eligible_channels: vec!["channel_redacted".to_string()],
            invocation_gates: Vec::new(),
            background_delivery_eligible: false,
            validation_state: "valid".to_string(),
            reason_code: String::new(),
            validated_at: Utc::now(),
            audit_event_id: String::new(),
            redaction_status: RedactionStatus::Redacted,
        })
        .expect("save route policy");

    let inbound = slack_group_inbound("room_policy_msg_1", "ambient room chatter", "ten_participation");

    let ignored = loop_
        .process_single_turn(&connector, &inbound, &sender, &cancel)
        .expect("ignored turn");
    assert_eq!(ignored.outcome, "ignored");
    assert_eq!(ignored.reason_code, "missing_qualifying_mention");
    assert!(ignored.run.run_id.is_empty(), "ignored message must not create a run");
    assert!(sender.last().is_none(), "ignored message must not send a reply");

    let persisted = store
        .get_connector_message_by_external_id_for_tenant(
            "ten_participation",
            "slack-main",
            DeliveryDirection::Inbound,
            "room_policy_msg_1",
        )
        .expect("get ignored inbound")
        .expect("found");
    let decisions = store
        .list_participation_decisions_for_thread("ten_participation", &persisted.thread_id, 10)
        .expect("list decisions");
    assert_eq!(decisions.len(), 1);
    assert_eq!(decisions[0].decision, ParticipationDecisionValue::Ignored);
    assert!(!decisions[0].created_assistant_work);

    let mut mentioned = inbound.clone();
    mentioned.external_message_id = "room_policy_msg_2".to_string();
    mentioned.provider_message_id = "room_policy_msg_2".to_string();
    mentioned.content = "@kura please respond".to_string();
    mentioned.mentioned = true;
    let accepted = loop_
        .process_single_turn(&connector, &mentioned, &sender, &cancel)
        .expect("mentioned turn");
    assert_eq!(accepted.outcome, "accepted");
    assert!(!accepted.run.run_id.is_empty(), "mentioned message must create a run");
    let persisted_accepted = store
        .get_connector_message_by_external_id_for_tenant(
            "ten_participation",
            "slack-main",
            DeliveryDirection::Inbound,
            "room_policy_msg_2",
        )
        .expect("get accepted inbound")
        .expect("found");
    let decisions = store
        .list_participation_decisions_for_thread("ten_participation", &persisted_accepted.thread_id, 10)
        .expect("list accepted decisions");
    assert!(
        decisions
            .iter()
            .any(|decision| decision.source_message_id == "room_policy_msg_2"
                && decision.decision == ParticipationDecisionValue::Accepted
                && decision.created_assistant_work),
        "expected accepted participation decision with assistant work"
    );

    let mut blocked = mentioned.clone();
    blocked.external_message_id = "room_policy_msg_3".to_string();
    blocked.provider_message_id = "room_policy_msg_3".to_string();
    blocked.channel_id = "channel_not_allowlisted".to_string();
    blocked.channel_or_conversation_id = "channel_not_allowlisted".to_string();
    blocked.peer_id = "channel_not_allowlisted".to_string();
    let blocked = loop_
        .process_single_turn(&connector, &blocked, &sender, &cancel)
        .expect("blocked turn");
    assert_eq!(blocked.outcome, "blocked");
    assert_eq!(blocked.reason_code, "not_allowlisted");
    assert!(blocked.run.run_id.is_empty(), "not-allowlisted message must not create a run");
}

// ---------------------------------------------------------------------------
// Go TestMessageLoopSplitsLongStreamingReplyWithinChannelLimit
// ---------------------------------------------------------------------------

#[test]
fn message_loop_splits_long_streaming_reply_within_channel_limit() {
    let (_store, loop_, _bus, _runtime, _dir) = test_harness(Arc::new(LongTestProvider));
    let sender = ProgressReplySender { max_len: 10, ..Default::default() };
    let cancel = CancellationToken::new();
    let connector = discord_connector();
    let long_prompt = "abcdefghij".repeat(4);
    let inbound = discord_inbound("discord_msg_stream_long_1", &long_prompt);

    let result = loop_
        .process_single_turn(&connector, &inbound, &sender, &cancel)
        .expect("streamed turn");
    assert_eq!(result.run.status, RunStatus::Completed);
    let sent = sender.sent();
    assert!(sent.len() >= 2, "expected multipart send for long reply, got {}", sent.len());
    for (index, sent_reply) in sent.iter().enumerate() {
        assert!(
            sent_reply.content.chars().count() <= 10,
            "chunk {index} exceeds max length: {:?}",
            sent_reply.content
        );
    }
}

// ---------------------------------------------------------------------------
// Go TestMessageLoopMarksFailureWhenReplySendFails
// ---------------------------------------------------------------------------

#[test]
fn message_loop_marks_failure_when_reply_send_fails() {
    let (store, loop_, bus, runtime, _dir) = test_harness(Arc::new(EchoTestProvider));
    let sender = TestReplySender { err: Some("discord send failed".to_string()), ..Default::default() };
    let cancel = CancellationToken::new();
    let connector = discord_connector();
    let mut inbound = discord_inbound("discord_msg_fail_1", "hello");
    inbound.tenant_id = "ten_discord".to_string();

    let err = loop_
        .process_single_turn(&connector, &inbound, &sender, &cancel)
        .expect_err("send failure must surface");
    assert_eq!(err, "discord send failed");

    let runs = runtime.list_runs();
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0].status, RunStatus::Failed, "run must be failed after send failure");

    let connector_events = bus.list(&Filter { category: "connector".to_string(), ..Filter::default() });
    let failed = connector_events
        .iter()
        .find(|event| event.name == "connector.reply_failed")
        .expect("connector.reply_failed event");
    assert!(
        !failed.payload.contains_key("error"),
        "reply failure event must not expose the raw provider error"
    );
    assert_eq!(
        failed.payload.get("reasonCode").and_then(|v| v.as_str()),
        Some("reply_failed")
    );
    assert_eq!(
        failed.payload.get("redactionStatus").and_then(|v| v.as_str()),
        Some("redacted")
    );

    let persisted = store
        .get_connector_message_by_external_id_for_tenant(
            "ten_discord",
            "discord-main",
            DeliveryDirection::Inbound,
            "discord_msg_fail_1",
        )
        .expect("get inbound")
        .expect("found");
    assert_eq!(persisted.status, DeliveryStatus::Failed);
    assert!(
        !persisted.error.contains("discord send failed"),
        "persisted error must not leak the raw send error: {:?}",
        persisted.error
    );

    let outcomes = store
        .list_channel_foreground_reply_outcomes("ten_discord", "discord-main", Utc::now())
        .expect("list reply outcomes");
    assert_eq!(outcomes.len(), 1);
    assert_eq!(outcomes[0].status, "failed");
    assert_eq!(outcomes[0].reason_code, "reply_failed");
}

// ---------------------------------------------------------------------------
// Go TestMessageLoopPreservesPartialReplyWhenProviderStreamFailsAfterVisibleOutput
// ---------------------------------------------------------------------------

#[test]
fn message_loop_preserves_partial_reply_when_provider_stream_fails_after_visible_output() {
    let (store, loop_, bus, runtime, _dir) = test_harness(Arc::new(PartialFailureTestProvider));
    let sender = ProgressReplySender::default();
    let cancel = CancellationToken::new();
    let connector = discord_connector();
    let inbound = discord_inbound("discord_msg_stream_partial_1", "hello");

    let err = loop_
        .process_single_turn(&connector, &inbound, &sender, &cancel)
        .expect_err("provider partial failure must surface");
    assert_eq!(err, "stream stalled");

    let runs = runtime.list_runs();
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0].status, RunStatus::Failed, "run must be failed after partial failure");

    let edited = sender.edited();
    assert!(!edited.is_empty(), "expected visible streamed edits before failure");
    assert!(
        edited.last().expect("last edit").content.contains("[response interrupted]"),
        "expected partial reply marker, got {:?}",
        edited.last().expect("last edit").content
    );

    let connector_events = bus.list(&Filter { category: "connector".to_string(), ..Filter::default() });
    let partial_count = connector_events
        .iter()
        .filter(|event| event.name == "connector.reply_partial")
        .count();
    let failed_count = connector_events
        .iter()
        .filter(|event| event.name == "connector.reply_failed")
        .count();
    assert_eq!(partial_count, 1, "expected exactly one connector.reply_partial event");
    assert_eq!(failed_count, 0, "expected no connector.reply_failed for partial failure");

    // The chat service in this harness runs without a store handle, so the
    // dispatch lifecycle surfaces through the llm events (the Go test reads
    // the persisted dispatch; the event carries the same status/partial fields).
    let llm_events = bus.list(&Filter { category: "llm".to_string(), ..Filter::default() });
    assert!(!llm_events.is_empty(), "expected llm events to be recorded");
    let last_llm = llm_events.last().expect("last llm event");
    assert_eq!(last_llm.name, "llm.dispatch.partial_failed");
    assert_eq!(
        last_llm.payload.get("status").and_then(|v| v.as_str()),
        Some("partial_failed")
    );
    assert_eq!(
        last_llm.payload.get("partial").and_then(|v| v.as_bool()),
        Some(true)
    );

    let outbound = store
        .get_connector_message_by_external_id(
            "discord-main",
            DeliveryDirection::Outbound,
            "discord_reply_1",
        )
        .expect("get outbound")
        .expect("found");
    assert_eq!(outbound.status, DeliveryStatus::Partial, "expected partial outbound record");
}

// ---------------------------------------------------------------------------
// Pure helper tests (Go TestConversationShapeForIngressSourceMapsWebOriginatedSurface
// + classify/split helpers)
// ---------------------------------------------------------------------------

#[test]
fn conversation_shape_for_ingress_source_maps_web_originated_surface() {
    let shape = kura_im::conversation_shape_for_ingress_source(
        kura_threads::SourceKind::Shell,
        "web",
        &InboundMessage::default(),
    );
    assert_eq!(shape, kura_threads::ConversationShape::Web);
}

#[test]
fn split_reply_content_respects_rune_limit() {
    use kura_im::split_reply_content;
    assert_eq!(split_reply_content("hello", 0), vec!["hello".to_string()]);
    assert_eq!(split_reply_content("hello", 10), vec!["hello".to_string()]);
    assert_eq!(split_reply_content("hello", 2), vec!["he".to_string(), "ll".to_string(), "o".to_string()]);
    // Multi-byte runes split on rune boundaries, not bytes.
    let parts = split_reply_content("he\u{00e9}llo", 3);
    assert_eq!(parts.join(""), "he\u{00e9}llo");
}

#[test]
fn classify_error_returns_class_for_classified_errors() {
    use kura_im::{ClassifiedError, classify_error};
    use std::io;

    let plain: Box<dyn std::error::Error + Send + Sync> = Box::new(io::Error::new(io::ErrorKind::Other, "nope"));
    assert_eq!(classify_error(Some(plain.as_ref())), "");
    assert_eq!(classify_error(None), "");

    let classified: Box<dyn std::error::Error + Send + Sync> = Box::new(ClassifiedError::new(
        "connector.timeout",
        Box::new(io::Error::new(io::ErrorKind::TimedOut, "dial timed out")),
    ));
    assert_eq!(classify_error(Some(classified.as_ref())), "connector.timeout");
    assert_eq!(kura_im::safe_reply_failure_reason(Some(classified.as_ref())), "connector.timeout");
    assert_eq!(kura_im::safe_reply_failure_reason(None), "reply_failed");
}
