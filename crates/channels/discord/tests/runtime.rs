//! Runtime integration tests (port of runtime_test.go): the FakeTransport
//! harness drives the Runtime through start/inbound-route/persistence paths
//! against a real SQLite store, the kura-im message loop, and the event bus.

use std::sync::Arc;

use chrono::{TimeZone, Utc};
use kura_chat::Service as ChatService;
use kura_checkpoints::Manager as CheckpointManager;
use kura_connectors::{DiagnosticReasonCode, Status, Supervisor};
use kura_discord::{
    Config, DestinationType, DestinationValidation, DestinationValidationState, DiscordError,
    DestinationValidator, Runtime, Transport,
};
use kura_events::{Bus, Filter};
use kura_im::{MessageLoop, ReplyProgressor, ReplySender};
use kura_imtypes::{InboundMessage, OutboundReply, ReplyCapabilities, ReplyEdit, SentReply, ThinkingSignal};
use kura_llm::{
    Dispatcher, Provider, ProviderError, ProviderRequest, ProviderResponse, StreamChunk,
    StreamEmitter, Usage,
};
use kura_router::{SessionKind, SessionRouter};
use kura_store::SQLiteStore;

// ---------------------------------------------------------------------------
// Echo provider producing "reply:" + first message content (Go testProvider).
// ---------------------------------------------------------------------------

struct ReplyEchoProvider;

impl Provider for ReplyEchoProvider {
    fn name(&self) -> &str {
        "echo"
    }

    fn complete<'a>(
        &'a self,
        request: ProviderRequest,
    ) -> futures::future::BoxFuture<'a, Result<ProviderResponse, ProviderError>> {
        Box::pin(async move {
            let content = request
                .messages
                .first()
                .map(|message| message.content.clone())
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
    ) -> futures::future::BoxFuture<'a, Result<ProviderResponse, ProviderError>> {
        Box::pin(async move {
            let content = request
                .messages
                .first()
                .map(|message| message.content.clone())
                .unwrap_or_default();
            let output = format!("reply:{content}");
            emit(StreamChunk { delta: output.clone(), ..StreamChunk::default() })?;
            Ok(ProviderResponse {
            tool_calls: Vec::new(),
                output,
                finish_reason: "stop".to_string(),
                usage: Usage { input_tokens: 1, output_tokens: 1, total_tokens: 2 },
            })
        })
    }
}

// ---------------------------------------------------------------------------
// FakeTransport (Go fakeTransport)
// ---------------------------------------------------------------------------

#[derive(Clone)]
struct FakeTransport {
    inner: Arc<FakeInner>,
}

struct FakeInner {
    handler: parking_lot::Mutex<Option<Arc<dyn Fn(InboundMessage) + Send + Sync>>>,
    sent: parking_lot::Mutex<Vec<OutboundReply>>,
    edited: parking_lot::Mutex<Vec<ReplyEdit>>,
    thinking: parking_lot::Mutex<Vec<ThinkingSignal>>,
    closed: parking_lot::Mutex<bool>,
    start_err: parking_lot::Mutex<Option<DiscordError>>,
    caps: ReplyCapabilities,
    validations: parking_lot::Mutex<Option<Vec<DestinationValidation>>>,
    validate_err: parking_lot::Mutex<Option<DiscordError>>,
}

impl FakeTransport {
    fn new(caps: ReplyCapabilities) -> Self {
        FakeTransport {
            inner: Arc::new(FakeInner {
                handler: parking_lot::Mutex::new(None),
                sent: parking_lot::Mutex::new(Vec::new()),
                edited: parking_lot::Mutex::new(Vec::new()),
                thinking: parking_lot::Mutex::new(Vec::new()),
                closed: parking_lot::Mutex::new(false),
                start_err: parking_lot::Mutex::new(None),
                caps,
                validations: parking_lot::Mutex::new(None),
                validate_err: parking_lot::Mutex::new(None),
            }),
        }
    }

    fn with_start_err(err: DiscordError) -> Self {
        let transport = FakeTransport::new(ReplyCapabilities {
            supports_thinking: true,
            supports_streaming: true,
            max_message_length: 2000,
        });
        *transport.inner.start_err.lock() = Some(err);
        transport
    }

    fn with_validations(validations: Vec<DestinationValidation>) -> Self {
        let transport = FakeTransport::new(ReplyCapabilities {
            supports_thinking: true,
            supports_streaming: true,
            max_message_length: 2000,
        });
        *transport.inner.validations.lock() = Some(validations);
        transport
    }

    fn invoke(&self, inbound: InboundMessage) {
        let handler = self.inner.handler.lock().clone();
        if let Some(handler) = handler {
            handler(inbound);
        }
    }

    fn sent(&self) -> Vec<OutboundReply> {
        self.inner.sent.lock().clone()
    }

    fn thinking(&self) -> Vec<ThinkingSignal> {
        self.inner.thinking.lock().clone()
    }

    fn edited(&self) -> Vec<ReplyEdit> {
        self.inner.edited.lock().clone()
    }

    fn is_closed(&self) -> bool {
        *self.inner.closed.lock()
    }
}

impl ReplySender for FakeTransport {
    fn send_reply(&self, reply: OutboundReply) -> Result<SentReply, String> {
        self.inner.sent.lock().push(reply);
        Ok(SentReply { external_message_id: "discord_reply_1".to_string() })
    }

    fn reply_progressor(&self) -> Option<&dyn ReplyProgressor> {
        Some(self)
    }
}

impl ReplyProgressor for FakeTransport {
    fn reply_capabilities(&self) -> ReplyCapabilities {
        self.inner.caps.clone()
    }

    fn send_thinking(&self, signal: ThinkingSignal) -> Result<(), String> {
        self.inner.thinking.lock().push(signal);
        Ok(())
    }

    fn edit_reply(&self, edit: ReplyEdit) -> Result<(), String> {
        self.inner.edited.lock().push(edit);
        Ok(())
    }
}

impl Transport for FakeTransport {
    fn start(
        &self,
        handle: Arc<dyn Fn(InboundMessage) + Send + Sync>,
    ) -> Result<(), DiscordError> {
        if let Some(err) = self.inner.start_err.lock().as_ref() {
            return Err(err.clone());
        }
        *self.inner.handler.lock() = Some(handle);
        Ok(())
    }

    fn close(&self) -> Result<(), DiscordError> {
        *self.inner.closed.lock() = true;
        *self.inner.handler.lock() = None;
        Ok(())
    }

    fn destination_validator(&self) -> Option<&dyn DestinationValidator> {
        Some(self)
    }
}

impl DestinationValidator for FakeTransport {
    fn validate_destinations(
        &self,
        destinations: Vec<DestinationValidation>,
    ) -> Result<Vec<DestinationValidation>, DiscordError> {
        if let Some(err) = self.inner.validate_err.lock().as_ref() {
            return Err(err.clone());
        }
        if let Some(validations) = self.inner.validations.lock().as_ref() {
            return Ok(validations.clone());
        }
        Ok(destinations)
    }
}

// ---------------------------------------------------------------------------
// Harness
// ---------------------------------------------------------------------------

struct Harness {
    store: Arc<SQLiteStore>,
    bus: Bus,
    loop_: Arc<MessageLoop>,
    _dir: tempfile::TempDir,
}

fn harness() -> Harness {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = Arc::new(SQLiteStore::new(dir.path().to_str().expect("path")).expect("store"));
    let bus = Bus::new();
    let dispatcher = Arc::new(Dispatcher::new());
    dispatcher.register_provider(Arc::new(ReplyEchoProvider));
    dispatcher.set_default_provider("echo").expect("default provider");
    dispatcher.set_default_model("echo-v1");
    let chat = ChatService::new_service(dispatcher, None, None, Some(bus.clone()), None);
    let runtime = Arc::new(kura_runtime::Manager::new());
    let checkpoints = CheckpointManager::new(
        Arc::new(parking_lot::Mutex::new(
            SQLiteStore::new(dir.path().to_str().expect("path")).expect("store"),
        )),
        runtime.clone(),
    );
    let loop_ = Arc::new(MessageLoop::new(
        SessionRouter::new(),
        runtime.clone(),
        Some(checkpoints),
        Some(bus.clone()),
        store.clone(),
        chat,
    ));
    Harness { store, bus, loop_, _dir: dir }
}

fn start_runtime(
    harness: &Harness,
    cfg: Config,
    transport: FakeTransport,
) -> (Runtime, FakeTransport) {
    let runtime = kura_discord::new_runtime(
        cfg,
        None,
        Arc::new(Supervisor::new()),
        harness.loop_.clone(),
        Some(harness.store.clone()),
        Some(harness.bus.clone()),
        Some(Box::new(transport.clone())),
    )
    .expect("new runtime")
    .expect("runtime enabled");
    (runtime, transport)
}

fn direct_inbound(external_id: &str, content: &str) -> InboundMessage {
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

fn base_cfg() -> Config {
    Config {
        enabled: true,
        connector_id: "discord-main".to_string(),
        display_name: "Discord Main".to_string(),
        delivery_mode: "gateway".to_string(),
        bot_token: "secret".to_string(),
        require_mention: true,
        respond_in_dm: true,
        tenant_id: "ten_discord".to_string(),
        ..Config::default()
    }
}

// Go TestRuntimeProcessesDirectMessageEndToEnd
#[test]
fn runtime_processes_direct_message_end_to_end() {
    let harness = harness();
    let caps = ReplyCapabilities { supports_thinking: true, supports_streaming: true, max_message_length: 2000 };
    let (runtime, transport) = start_runtime(&harness, base_cfg(), FakeTransport::new(caps));
    runtime.start().expect("start");

    transport.invoke(direct_inbound("discord_msg_1", "hello"));
    runtime.drain_pending();

    assert!(!transport.thinking().is_empty(), "expected at least one thinking signal");
    let sent = transport.sent();
    assert_eq!(sent.len(), 1, "expected 1 reply");
    assert_eq!(sent[0].content, "reply:hello");
    assert!(transport.edited().is_empty(), "expected no edit for single-chunk stream");

    let items = harness.store.list_connectors().expect("list connectors");
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].status, Status::Healthy);
}

// Go TestRuntimeIgnoresGuildMessageWithoutMentionWhenRequired
#[test]
fn runtime_ignores_guild_message_without_mention_when_required() {
    let harness = harness();
    let caps = ReplyCapabilities { supports_thinking: true, supports_streaming: true, max_message_length: 2000 };
    let (runtime, transport) = start_runtime(&harness, base_cfg(), FakeTransport::new(caps));
    runtime.start().expect("start");

    transport.invoke(InboundMessage {
        connector_id: "discord-main".to_string(),
        connector_kind: "discord".to_string(),
        external_message_id: "discord_msg_2".to_string(),
        account_id: "bot_1".to_string(),
        channel_id: "channel_1".to_string(),
        guild_id: "guild_1".to_string(),
        peer_id: "channel_1".to_string(),
        thread_id: "channel_1".to_string(),
        author_id: "user_1".to_string(),
        content: "hello".to_string(),
        kind: SessionKind::Group,
        direct: false,
        mentioned: false,
        received_at: Utc::now(),
        ..InboundMessage::default()
    });
    runtime.drain_pending();

    assert!(transport.sent().is_empty(), "expected guild message without mention to be ignored");
    let connector_events = harness.bus.list(&Filter { category: "connector".to_string(), ..Filter::default() });
    assert!(!connector_events.is_empty(), "expected route outcome event");
    let last = connector_events.last().expect("last event");
    assert_eq!(last.name, "connector.route_outcome_recorded");
    assert_eq!(last.payload.get("outcome").and_then(|v| v.as_str()), Some("ignored"));
    assert_eq!(last.payload.get("reasonCode").and_then(|v| v.as_str()), Some("mention_required"));
}

// Go TestNewRuntimeRejectsMissingBotToken
#[test]
fn new_runtime_rejects_missing_bot_token() {
    let harness = harness();
    let result = kura_discord::new_runtime(
        Config {
            enabled: true,
            connector_id: "discord-main".to_string(),
            display_name: "Discord Main".to_string(),
            ..Config::default()
        },
        None,
        Arc::new(Supervisor::new()),
        harness.loop_.clone(),
        Some(harness.store.clone()),
        Some(harness.bus.clone()),
        Some(Box::new(FakeTransport::new(ReplyCapabilities::default()))),
    );
    assert!(matches!(result, Err(DiscordError::BotTokenRequired)));
}

// Go TestRuntimePublishesClassifiedFailureWhenTransportStartFails
#[test]
fn runtime_publishes_classified_failure_when_transport_start_fails() {
    let harness = harness();
    let transport = FakeTransport::with_start_err(DiscordError::Other("401 Unauthorized".to_string()));
    let runtime = kura_discord::new_runtime(
        base_cfg(),
        None,
        Arc::new(Supervisor::new()),
        harness.loop_.clone(),
        Some(harness.store.clone()),
        Some(harness.bus.clone()),
        Some(Box::new(transport)),
    )
    .expect("new runtime")
    .expect("runtime enabled");

    let err = runtime.start().expect_err("expected transport start failure");
    assert!(err.to_string().contains("401 Unauthorized"));

    let connector_events = harness.bus.list(&Filter { category: "connector".to_string(), ..Filter::default() });
    assert!(!connector_events.is_empty(), "expected connector failure event");
    let last = connector_events.last().expect("last event");
    assert_eq!(last.name, "connector.failed");
    assert_eq!(last.payload.get("errorClass").and_then(|v| v.as_str()), Some("auth_error"));
}

// Go TestRuntimeDoesNotMarkHostedReadyWithoutDestinationValidationEvidence
#[test]
fn runtime_does_not_mark_hosted_ready_without_destination_validation_evidence() {
    let harness = harness();
    let mut cfg = base_cfg();
    cfg.allowed_guild_ids = vec!["guild_1".to_string()];
    cfg.allowed_channel_ids = vec!["channel_1".to_string()];
    let caps = ReplyCapabilities { supports_thinking: true, supports_streaming: true, max_message_length: 2000 };
    let (runtime, _transport) = start_runtime(&harness, cfg, FakeTransport::new(caps));
    runtime.start().expect("start");

    let setup = harness
        .store
        .get_discord_hosted_setup("ten_discord", "discord-main")
        .expect("get setup");
    let setup = setup.expect("setup exists");
    assert!(!setup.hosted_ready);
    assert_eq!(setup.readiness_state, "degraded_needs_repair");
    assert_eq!(setup.reason_code, "destination_validation_failed");
}

// Go TestRuntimeMarksHostedReadyWithValidatedDestinationEvidence
#[test]
fn runtime_marks_hosted_ready_with_validated_destination_evidence() {
    let harness = harness();
    let mut cfg = base_cfg();
    cfg.allowed_guild_ids = vec!["guild_1".to_string()];
    cfg.allowed_channel_ids = vec!["channel_1".to_string()];
    let now = Utc.with_ymd_and_hms(2026, 5, 7, 10, 0, 0).single().expect("ts");
    let validations = vec![
        DestinationValidation {
            connector_id: "discord-main".to_string(),
            destination_id: "guild_1".to_string(),
            destination_type: DestinationType::Guild,
            selected: true,
            validation_state: DestinationValidationState::Valid,
            reason_code: "healthy".to_string(),
            validated_at: now,
            redaction_status: kura_connectors::RedactionStatus::Redacted,
            safe_evidence: [("source".to_string(), "gateway_state".to_string())].into(),
            ..DestinationValidation::default()
        },
        DestinationValidation {
            connector_id: "discord-main".to_string(),
            destination_id: "channel_1".to_string(),
            destination_type: DestinationType::Channel,
            selected: true,
            validation_state: DestinationValidationState::Valid,
            reason_code: "healthy".to_string(),
            validated_at: now,
            redaction_status: kura_connectors::RedactionStatus::Redacted,
            safe_evidence: [("source".to_string(), "gateway_state".to_string())].into(),
            ..DestinationValidation::default()
        },
    ];
    let (runtime, _transport) = start_runtime(&harness, cfg, FakeTransport::with_validations(validations));
    runtime.start().expect("start");

    let setup = harness
        .store
        .get_discord_hosted_setup("ten_discord", "discord-main")
        .expect("get setup")
        .expect("setup exists");
    assert!(setup.hosted_ready);
    assert_eq!(setup.readiness_state, "hosted_ready");

    let results = harness
        .store
        .list_connector_conformance_results("ten_discord", "discord-main", Utc::now())
        .expect("list conformance results");
    let passed_core = results.iter().filter(|result| result.result == kura_connectors::ConformanceResultStatus::Pass).count();
    assert!(passed_core >= kura_connectors::core_invariant_areas().len());
}

// Go TestRuntimeBlocksInboundMissingDurableIdentity
#[test]
fn runtime_blocks_inbound_missing_durable_identity() {
    let harness = harness();
    let mut cfg = base_cfg();
    cfg.require_mention = false;
    let caps = ReplyCapabilities { supports_thinking: true, supports_streaming: true, max_message_length: 2000 };
    let (runtime, transport) = start_runtime(&harness, cfg, FakeTransport::new(caps));
    runtime.start().expect("start");

    transport.invoke(InboundMessage {
        connector_id: "discord-main".to_string(),
        connector_kind: "discord".to_string(),
        external_message_id: "discord_msg_missing_account".to_string(),
        channel_id: "dm_1".to_string(),
        peer_id: "user_1".to_string(),
        author_id: "user_1".to_string(),
        content: "hello".to_string(),
        kind: SessionKind::Direct,
        direct: true,
        received_at: Utc::now(),
        ..InboundMessage::default()
    });
    runtime.drain_pending();

    assert!(transport.sent().is_empty(), "expected no replies for missing durable identity");
    let connector_events = harness.bus.list(&Filter { category: "connector".to_string(), ..Filter::default() });
    assert!(!connector_events.is_empty(), "expected blocked route event");
    let last = connector_events.last().expect("last event");
    assert_eq!(last.name, "connector.route_outcome_recorded");
    assert_eq!(last.payload.get("outcome").and_then(|v| v.as_str()), Some("blocked"));
    assert_eq!(last.payload.get("reasonCode").and_then(|v| v.as_str()), Some("missing_durable_identity"));

    let diagnostics = harness
        .store
        .list_connector_diagnostic_states("ten_discord", "discord-main", Utc::now())
        .expect("list diagnostics");
    assert!(!diagnostics.is_empty());
    assert_eq!(diagnostics.last().expect("last diagnostic").reason_code, DiagnosticReasonCode::BlockedRoute);
}

#[test]
fn runtime_close_marks_transport_closed() {
    let harness = harness();
    let caps = ReplyCapabilities { supports_thinking: true, supports_streaming: true, max_message_length: 2000 };
    let (runtime, transport) = start_runtime(&harness, base_cfg(), FakeTransport::new(caps));
    runtime.start().expect("start");
    runtime.close().expect("close");
    assert!(transport.is_closed());
}

