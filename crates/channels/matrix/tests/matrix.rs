//! Behavioral tests for the kura-matrix crate (port of the Go package's
//! _test.go files): redaction, dedupe, replies, provider decisions, route
//! decisions, setup evaluation, conformance surfaces, unsupported kinds,
//! diagnostics freshness, readiness, smoke evidence, client transport REST
//! behavior (against a local TCP mock server), and runtime inbound routing.

use std::collections::HashMap;
use std::sync::{Arc, Mutex as StdMutex};

use chrono::{DateTime, Duration, TimeZone, Utc};
use kura_chat::Service;
use kura_checkpoints::Manager as CheckpointManager;
use kura_connectors::{
    ConformanceResultStatus, SurfaceSupport, Supervisor, core_invariant_areas,
};
use kura_events::Bus;
use kura_im::MessageLoop;
use kura_imtypes::OutboundReply;
use kura_llm::{
    Dispatcher, Provider, ProviderError, ProviderRequest, ProviderResponse, StreamEmitter, Usage,
};
use kura_matrix::*;
use kura_router::SessionRouter;
use kura_runtime::Manager as RuntimeManager;
use kura_store::matrix_setup::MatrixHostedSetupRecord;
use kura_store::SQLiteStore;
use futures::future::BoxFuture;

fn ts(y: i32, mo: u32, d: u32, h: u32, mi: u32, s: u32) -> DateTime<Utc> {
    Utc.with_ymd_and_hms(y, mo, d, h, mi, s).single().expect("valid timestamp")
}

// ---------------------------------------------------------------------------
// Mock Matrix homeserver (local TCP server)
// ---------------------------------------------------------------------------

mod test_server {
    use std::collections::HashMap;
    use std::io::{Read, Write};
    use std::net::{TcpListener, TcpStream};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::thread;

    pub struct TestRequest {
        pub method: String,
        /// The escaped request path (Go r.URL.EscapedPath() equivalent).
        pub escaped_path: String,
        pub headers: HashMap<String, String>,
        pub body: String,
    }

    pub struct TestResponse {
        pub status: u16,
        pub body: String,
    }

    /// A minimal single-threaded HTTP/1.1 server for transport tests.
    pub struct TestServer {
        pub base_url: String,
        join: Option<thread::JoinHandle<()>>,
        shutdown: Arc<AtomicBool>,
    }

    impl TestServer {
        pub fn start(handler: impl Fn(&TestRequest) -> TestResponse + Send + Sync + 'static) -> TestServer {
            let listener = TcpListener::bind("127.0.0.1:0").expect("bind test server");
            listener.set_nonblocking(true).expect("nonblocking listener");
            let addr = listener.local_addr().expect("local addr");
            let base_url = format!("http://{addr}");
            let handler = Arc::new(handler);
            let shutdown = Arc::new(AtomicBool::new(false));
            let shutdown_flag = Arc::clone(&shutdown);
            let join = thread::spawn(move || loop {
                if shutdown_flag.load(Ordering::SeqCst) {
                    break;
                }
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        // On macOS/BSD the accepted stream inherits the
                        // listener's non-blocking mode; restore blocking IO so
                        // reading the request never fails with WouldBlock when
                        // the client's bytes are still in flight (the CI-only
                        // status-line flake). A read timeout keeps the test
                        // bounded, and a bad connection never kills the server.
                        let _ = stream.set_nonblocking(false);
                        let _ = stream
                            .set_read_timeout(Some(std::time::Duration::from_secs(5)));
                        let handler = Arc::clone(&handler);
                        let _ = handle_connection(&mut stream, &*handler);
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(std::time::Duration::from_millis(10));
                    }
                    Err(_) => break,
                }
            });
            TestServer { base_url, join: Some(join), shutdown }
        }

        pub fn stop(&mut self) {
            if let Some(join) = self.join.take() {
                self.shutdown.store(true, Ordering::SeqCst);
                let _ = join.join();
            }
        }
    }

    impl Drop for TestServer {
        fn drop(&mut self) {
            self.stop();
        }
    }

    fn handle_connection(
        stream: &mut TcpStream,
        handler: &dyn Fn(&TestRequest) -> TestResponse,
    ) -> std::io::Result<()> {
        let _ = stream.set_read_timeout(Some(std::time::Duration::from_secs(5)));
        let mut head = Vec::new();
        let mut byte = [0u8; 1];
        loop {
            let n = stream.read(&mut byte)?;
            if n == 0 {
                return Ok(());
            }
            head.push(byte[0]);
            if head.ends_with(b"\r\n\r\n") {
                break;
            }
            if head.len() > 64 * 1024 {
                return Ok(());
            }
        }
        let head_str = String::from_utf8_lossy(&head).to_string();
        let mut lines = head_str.split("\r\n");
        let request_line = lines.next().unwrap_or_default().to_string();
        let mut parts = request_line.split_whitespace();
        let method = parts.next().unwrap_or_default().to_string();
        let target = parts.next().unwrap_or_default().to_string();
        let mut headers = HashMap::new();
        for line in lines {
            if let Some((k, v)) = line.split_once(':') {
                headers.insert(k.trim().to_lowercase(), v.trim().to_string());
            }
        }
        let content_length: usize = headers
            .get("content-length")
            .and_then(|v| v.parse().ok())
            .unwrap_or(0);
        let mut body = Vec::new();
        while body.len() < content_length {
            let mut chunk = vec![0u8; content_length - body.len()];
            let n = stream.read(&mut chunk)?;
            if n == 0 {
                break;
            }
            body.extend_from_slice(&chunk[..n]);
        }
        let body = String::from_utf8_lossy(&body).to_string();
        let escaped_path = target.split('?').next().unwrap_or(&target).to_string();
        let request = TestRequest { method, escaped_path, headers, body };
        let response = handler(&request);
        let body_bytes = response.body.as_bytes();
        let head_resp = format!(
            "HTTP/1.1 {} OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            response.status,
            body_bytes.len()
        );
        stream.write_all(head_resp.as_bytes())?;
        stream.write_all(body_bytes)?;
        stream.flush()?;
        Ok(())
    }
}

use test_server::{TestResponse, TestServer};

// ---------------------------------------------------------------------------
// Redaction
// ---------------------------------------------------------------------------

#[test]
fn redact_evidence_suppresses_secrets_and_raw_payloads() {
    let got = redact_evidence(&HashMap::from([
        ("accessToken".to_string(), "secret-token".to_string()),
        ("rawProviderPayload".to_string(), "{\"body\":\"hello\"}".to_string()),
        ("homeserver".to_string(), "matrix.example.org".to_string()),
        ("room".to_string(), "!room:example.org".to_string()),
    ]));
    assert_eq!(got.status.as_str(), "suppressed");
    assert!(!got.safe_evidence.contains_key("accessToken"));
    assert!(!got.safe_evidence.get("homeserver").unwrap_or(&String::new()).is_empty());
    assert!(!got.safe_evidence.get("room").unwrap_or(&String::new()).is_empty());
}

// ---------------------------------------------------------------------------
// Dedupe
// ---------------------------------------------------------------------------

#[test]
fn dedupe_uses_homeserver_conversation_and_event_id() {
    let cache = new_dedupe_cache();
    let first = InboundEvent {
        tenant_id: "ten".to_string(),
        connector_id: "matrix-main".to_string(),
        homeserver_id: "matrix.example.org".to_string(),
        conversation_id: "!room:example.org".to_string(),
        matrix_event_id: "$event".to_string(),
        ..InboundEvent::default()
    };
    let mut replayed = first.clone();
    replayed.sync_batch_id = "sync-2".to_string();

    assert!(!cache.mark_duplicate(&first), "first event should not be duplicate");
    assert!(
        cache.mark_duplicate(&replayed),
        "same homeserver/conversation/event should be duplicate despite different sync batch"
    );

    let mut other_room = first.clone();
    other_room.conversation_id = "!other:example.org".to_string();
    assert!(
        !cache.mark_duplicate(&other_room),
        "same event id in different conversation should not be duplicate"
    );
}

// ---------------------------------------------------------------------------
// Replies
// ---------------------------------------------------------------------------

#[test]
fn final_reply_outcome_separates_assistant_and_matrix_reply_truth() {
    let transport = FakeTransport::new(Vec::new());
    let outcome = send_final_reply(
        Some(&transport),
        &InboundEvent {
            tenant_id: "ten".to_string(),
            connector_id: "matrix-main".to_string(),
            conversation_id: "!room:example.org".to_string(),
            matrix_event_id: "$event".to_string(),
            conversation_type: ConversationType::Room,
            ..InboundEvent::default()
        },
        OutboundReply { content: "done".to_string(), ..OutboundReply::default() },
    );
    assert_eq!(outcome.assistant_execution_outcome, "succeeded");
    assert_eq!(outcome.matrix_reply_outcome, "sent");
    assert_eq!(outcome.reply_progression_level, "final_only");
}

// ---------------------------------------------------------------------------
// Provider decision
// ---------------------------------------------------------------------------

#[test]
fn provider_decision_selects_matrix_and_rejects_whatsapp() {
    let decision = phase52_provider_decision("owner@example.com", ts(2026, 5, 10, 10, 0, 0));
    validate_provider_decision(&decision).expect("decision should validate");
    assert_eq!(decision.selected_provider, CONNECTOR_KIND);
    assert_eq!(decision.rejected_provider, "whatsapp");
}

#[test]
fn provider_decision_blocks_unsafe_matrix_or_whatsapp_fallback() {
    let mut decision = phase52_provider_decision("owner@example.com", Utc::now());
    decision.unsafe_matrix_dependency = true;
    let err = validate_provider_decision(&decision).expect_err("unsafe dependency must fail");
    assert_eq!(err, ProviderDecisionError::UnsafeMatrixDependency);

    let mut decision = phase52_provider_decision("owner@example.com", Utc::now());
    decision.selected_provider = "whatsapp".to_string();
    let err = validate_provider_decision(&decision).expect_err("whatsapp must fail");
    assert_eq!(err, ProviderDecisionError::WhatsappOutOfScope);
}

// ---------------------------------------------------------------------------
// Routes
// ---------------------------------------------------------------------------

#[test]
fn decide_route_accepts_direct_and_room_invocation_gate() {
    let policy = normalize_route_policy(
        RoutePolicy {
            allowed_direct_users: vec!["@alice:example.org".to_string()],
            selected_rooms: vec![ConversationRoute {
                conversation_id: "!room:example.org".to_string(),
                conversation_type: ConversationType::Room,
                room_selection_state: RoomSelectionState::Selected,
                validation_state: RoutePolicyState::Valid,
                ..ConversationRoute::default()
            }],
            configured_commands: vec!["!kura".to_string()],
            validation_state: RoutePolicyState::Valid,
            ..RoutePolicy::default()
        },
        Utc::now(),
    );

    let direct = decide_route(
        &InboundEvent {
            homeserver_id: "matrix.example.org".to_string(),
            conversation_id: "@alice:example.org".to_string(),
            matrix_event_id: "$event1".to_string(),
            sender_id: "@alice:example.org".to_string(),
            conversation_type: ConversationType::DirectMessage,
            message_kind: MessageKind::UnencryptedText,
            text: "hello".to_string(),
            ..InboundEvent::default()
        },
        policy.clone(),
        "matrix.example.org",
        "@bot:example.org",
    );
    assert_eq!(direct.outcome, RouteOutcome::Accepted, "direct route should accept");

    let room = decide_route(
        &InboundEvent {
            homeserver_id: "matrix.example.org".to_string(),
            conversation_id: "!room:example.org".to_string(),
            matrix_event_id: "$event2".to_string(),
            sender_id: "@alice:example.org".to_string(),
            conversation_type: ConversationType::Room,
            message_kind: MessageKind::UnencryptedText,
            text: "@bot:example.org hello".to_string(),
            bot_mentioned: true,
            ..InboundEvent::default()
        },
        policy,
        "matrix.example.org",
        "@bot:example.org",
    );
    assert_eq!(room.outcome, RouteOutcome::Accepted);
    assert_eq!(room.normalized_text, "hello");
}

#[test]
fn decide_route_blocks_unsupported_and_ungated_rooms() {
    let policy = normalize_route_policy(
        RoutePolicy {
            selected_rooms: vec![ConversationRoute {
                conversation_id: "!room:example.org".to_string(),
                conversation_type: ConversationType::Room,
                room_selection_state: RoomSelectionState::Selected,
                validation_state: RoutePolicyState::Valid,
                ..ConversationRoute::default()
            }],
            validation_state: RoutePolicyState::Valid,
            ..RoutePolicy::default()
        },
        Utc::now(),
    );

    let encrypted = decide_route(
        &InboundEvent {
            homeserver_id: "matrix.example.org".to_string(),
            conversation_id: "!room:example.org".to_string(),
            matrix_event_id: "$event1".to_string(),
            sender_id: "@alice:example.org".to_string(),
            conversation_type: ConversationType::Room,
            message_kind: MessageKind::EncryptedUnsupported,
            ..InboundEvent::default()
        },
        policy.clone(),
        "matrix.example.org",
        "@bot:example.org",
    );
    assert_eq!(encrypted.outcome, RouteOutcome::Unsupported);

    let ungated = decide_route(
        &InboundEvent {
            homeserver_id: "matrix.example.org".to_string(),
            conversation_id: "!room:example.org".to_string(),
            matrix_event_id: "$event2".to_string(),
            sender_id: "@alice:example.org".to_string(),
            conversation_type: ConversationType::Room,
            message_kind: MessageKind::UnencryptedText,
            text: "hello".to_string(),
            ..InboundEvent::default()
        },
        policy,
        "matrix.example.org",
        "@bot:example.org",
    );
    assert_eq!(ungated.outcome, RouteOutcome::Ignored);
    assert_eq!(ungated.reason_code, "mention_required");
}

// ---------------------------------------------------------------------------
// Setup
// ---------------------------------------------------------------------------

#[test]
fn evaluate_hosted_setup_ready_requires_bot_homeserver_binding_route_and_conformance() {
    let now = Utc::now();
    let setup = evaluate_hosted_setup(HostedSetupInput {
        tenant_id: "ten_matrix".to_string(),
        connector_id: "matrix-main".to_string(),
        display_name: "Matrix Main".to_string(),
        bot_credential_state: BotCredentialState::Valid,
        homeserver_binding: HomeserverBinding {
            homeserver_binding_id: "matrix_hs_1".to_string(),
            homeserver_url: "https://matrix.example.org".to_string(),
            bot_user_id: "@bot:example.org".to_string(),
            authorization_state: AuthorizationState::Valid,
            homeserver_capability_state: HomeserverCapabilityState::Valid,
            ..HomeserverBinding::default()
        },
        route_policy: RoutePolicy {
            selected_rooms: vec![ConversationRoute {
                conversation_id: "!room:example.org".to_string(),
                conversation_type: ConversationType::Room,
                room_selection_state: RoomSelectionState::Selected,
                validation_state: RoutePolicyState::Valid,
                ..ConversationRoute::default()
            }],
            validation_state: RoutePolicyState::Valid,
            ..RoutePolicy::default()
        },
        provider_available: true,
        network_available: true,
        conformance_passed: true,
        started_at: now - Duration::minutes(4),
        validated_at: now,
        ..HostedSetupInput::default()
    });

    assert_eq!(setup.terminal_state, TerminalState::Ready);
    assert_eq!(setup.status.as_str(), "healthy");
    assert!(setup.delivery_eligible);
}

#[test]
fn evaluate_hosted_setup_returns_actionable_bounded_terminal_state() {
    let now = Utc::now();
    let setup = evaluate_hosted_setup(HostedSetupInput {
        tenant_id: "ten_matrix".to_string(),
        connector_id: "matrix-main".to_string(),
        display_name: "Matrix Main".to_string(),
        bot_credential_state: BotCredentialState::Invalid,
        provider_available: true,
        network_available: true,
        started_at: now - Duration::minutes(6),
        setup_timeout: Duration::minutes(5),
        validated_at: now,
        ..HostedSetupInput::default()
    });

    assert_eq!(setup.terminal_state, TerminalState::ActionRequired);
    assert_eq!(setup.reason_code, "auth_missing");
    assert_eq!(setup.setup_completed_within, Duration::minutes(5));
}

#[test]
fn evaluate_hosted_setup_rejects_hosted_homeserver_provisioning() {
    let setup = evaluate_hosted_setup(HostedSetupInput {
        tenant_id: "ten_matrix".to_string(),
        connector_id: "matrix-main".to_string(),
        display_name: "Matrix Main".to_string(),
        requested_hosted_homeserver: true,
        requested_account_provision: true,
        provider_available: true,
        network_available: true,
        validated_at: ts(2026, 5, 10, 10, 0, 0),
        ..HostedSetupInput::default()
    });
    assert_eq!(setup.terminal_state, TerminalState::ActionRequired);
    assert_eq!(setup.reason_code, "unsupported_capability");
}

// ---------------------------------------------------------------------------
// Conformance
// ---------------------------------------------------------------------------

#[test]
fn conformance_profile_declares_matrix_surfaces() {
    let profile = conformance_profile(
        &Config {
            connector_id: "matrix-main".to_string(),
            selected_room_ids: vec!["!room:example.org".to_string()],
            allowed_direct_user_ids: vec!["@user:example.org".to_string()],
            ..Config::default()
        },
        ts(2026, 5, 10, 10, 0, 0),
    );

    assert_eq!(profile.connector_kind, CONNECTOR_KIND);
    assert_eq!(profile.equivalent_durable_identity_rule_id, "matrix_homeserver_conversation_event_id");
    for area in core_invariant_areas() {
        assert_eq!(
            profile.core_invariant_results.get(&area).copied(),
            Some(ConformanceResultStatus::Pass),
            "core invariant {area:?} must pass"
        );
    }
    for (surface, want) in [
        ("tenant_provided_bot_setup", SurfaceSupport::Supported),
        ("kuraagent_hosted_homeserver", SurfaceSupport::Unsupported),
        ("matrix_account_provisioning", SurfaceSupport::Unsupported),
        ("direct_message", SurfaceSupport::Supported),
        ("allowed_room_mention", SurfaceSupport::Supported),
        ("allowed_room_command", SurfaceSupport::Supported),
        ("unencrypted_text", SurfaceSupport::Supported),
        ("encrypted_rooms", SurfaceSupport::Unsupported),
        ("undecryptable_events", SurfaceSupport::Unsupported),
        ("e2ee_key_session_management", SurfaceSupport::Unsupported),
        ("final_only_foreground_reply", SurfaceSupport::Supported),
        ("connector_backed_delivery", SurfaceSupport::Supported),
        ("whatsapp", SurfaceSupport::Unsupported),
        ("bridge_automation", SurfaceSupport::Unsupported),
        ("media", SurfaceSupport::Unsupported),
        ("voice", SurfaceSupport::Unsupported),
        ("calls", SurfaceSupport::Unsupported),
        ("thinking_visibility", SurfaceSupport::Unsupported),
        ("incremental_visible_updates", SurfaceSupport::Unsupported),
        ("blocked_route_classification", SurfaceSupport::Supported),
    ] {
        assert_eq!(
            profile.provider_surface_results.get(surface).copied(),
            Some(want),
            "surface {surface}"
        );
    }
}

// ---------------------------------------------------------------------------
// Unsupported kinds
// ---------------------------------------------------------------------------

#[test]
fn unsupported_message_kind_classifies_matrix_unsupported_surfaces() {
    for kind in [
        MessageKind::EncryptedUnsupported,
        MessageKind::UndecryptableUnsupported,
        MessageKind::Unsupported,
        MessageKind::MediaUnsupported,
        MessageKind::CallUnsupported,
        MessageKind::VoiceUnsupported,
        MessageKind::ReactionUnsupported,
        MessageKind::BridgeMetadataUnsupported,
        MessageKind::Unknown,
    ] {
        assert!(unsupported_message_kind(kind), "kind {kind:?} should be unsupported");
    }
    assert!(!unsupported_message_kind(MessageKind::UnencryptedText));
}

// ---------------------------------------------------------------------------
// Diagnostics
// ---------------------------------------------------------------------------

#[test]
fn matrix_diagnostics_freshness_and_redaction_suppression() {
    let now = Utc::now();
    let fresh = map_condition(
        MatrixCondition::RateLimited,
        DiagnosticInput {
            tenant_id: "ten_matrix".to_string(),
            connector_id: "matrix-main".to_string(),
            evidence_timestamp: now - Duration::minutes(1),
            now,
            redaction_reliable: true,
            safe_evidence: HashMap::from([("retryAfter".to_string(), "60s".to_string())]),
        },
    );
    assert_eq!(fresh.base.freshness_state.as_str(), "fresh");
    assert_eq!(fresh.base.redaction_status.as_str(), "redacted");
    assert_eq!(fresh.base.safe_evidence.get("retryAfter").map(String::as_str), Some("60s"));

    let suppressed = map_condition(
        MatrixCondition::ReplyFailed,
        DiagnosticInput {
            tenant_id: "ten_matrix".to_string(),
            connector_id: "matrix-main".to_string(),
            evidence_timestamp: now - Duration::minutes(30),
            now,
            redaction_reliable: false,
            safe_evidence: HashMap::from([("unsafe".to_string(), "dropped".to_string())]),
        },
    );
    assert_eq!(suppressed.base.freshness_state.as_str(), "stale");
    assert_eq!(suppressed.base.redaction_status.as_str(), "suppressed");
    assert!(suppressed.base.safe_evidence.is_empty());
}

#[test]
fn matrix_diagnostic_mapping_and_freshness() {
    let now = Utc::now();
    let diag = map_condition(
        MatrixCondition::HomeserverUnsupported,
        DiagnosticInput {
            tenant_id: "ten".to_string(),
            connector_id: "matrix-main".to_string(),
            evidence_timestamp: now - Duration::minutes(16),
            now,
            redaction_reliable: true,
            ..DiagnosticInput::default()
        },
    );
    assert_eq!(diag.base.reason_code.as_str(), "unsupported_capability");
    assert_eq!(diag.base.freshness_state.as_str(), "stale");
}

// ---------------------------------------------------------------------------
// Readiness
// ---------------------------------------------------------------------------

#[test]
fn validate_homeserver_binding_requires_exactly_one_tenant_scoped_bot() {
    let binding = normalize_homeserver_binding(
        "ten_matrix",
        "matrix-main",
        HomeserverBinding {
            homeserver_url: "https://matrix.example.org".to_string(),
            bot_user_id: "@bot:example.org".to_string(),
            authorization_state: AuthorizationState::Valid,
            homeserver_capability_state: HomeserverCapabilityState::Valid,
            ..HomeserverBinding::default()
        },
    );
    assert!(!binding.homeserver_binding_id.is_empty());
    assert_eq!(binding.tenant_id, "ten_matrix");
    assert_eq!(binding.connector_id, "matrix-main");
    validate_homeserver_binding(&binding).expect("binding should validate");

    let mut missing_bot = binding.clone();
    missing_bot.bot_user_id = String::new();
    assert!(validate_homeserver_binding(&missing_bot).is_err());
}

// ---------------------------------------------------------------------------
// Smoke evidence
// ---------------------------------------------------------------------------

#[test]
fn smoke_evidence_structured_skip_includes_required_risk_record() {
    let smoke = structured_skip_smoke_evidence(
        "ten",
        "matrix-main",
        "owner",
        "safe Matrix credentials unavailable",
        ts(2026, 5, 10, 10, 0, 0),
    );
    assert_eq!(smoke.status, SmokeStatus::Skipped);
    assert_eq!(smoke.authorization_mode, SmokeAuthorizationMode::Unavailable);
    assert!(!smoke.owner.is_empty());
    assert!(!smoke.reason.is_empty());
    assert!(!smoke.remaining_risk.is_empty());
    assert_eq!(smoke.retention_expires_at - smoke.validated_at, Duration::days(90));
}

// ---------------------------------------------------------------------------
// Client transport (mock homeserver)
// ---------------------------------------------------------------------------

#[test]
fn client_transport_sends_matrix_text_reply_with_bearer_token() {
    let saw_path: Arc<StdMutex<String>> = Arc::new(StdMutex::new(String::new()));
    let saw_auth: Arc<StdMutex<String>> = Arc::new(StdMutex::new(String::new()));
    let saw_body: Arc<StdMutex<serde_json::Value>> = Arc::new(StdMutex::new(serde_json::Value::Null));
    let server = {
        let saw_path = Arc::clone(&saw_path);
        let saw_auth = Arc::clone(&saw_auth);
        let saw_body = Arc::clone(&saw_body);
        TestServer::start(move |request| {
            *saw_path.lock().expect("lock") = request.escaped_path.clone();
            *saw_auth.lock().expect("lock") = request
                .headers
                .get("authorization")
                .cloned()
                .unwrap_or_default();
            *saw_body.lock().expect("lock") =
                serde_json::from_str(&request.body).unwrap_or(serde_json::Value::Null);
            TestResponse { status: 200, body: "{\"event_id\":\"$reply1\"}".to_string() }
        })
    };

    let transport = new_client_transport(ClientTransportConfig {
        connector_id: "matrix-main".to_string(),
        homeserver_url: server.base_url.clone(),
        bot_access_token: "matrix-token-do-not-leak".to_string(),
        ..ClientTransportConfig::default()
    })
    .expect("transport");
    let sent = Transport::send_reply(
        &transport,
        OutboundReply {
            connector_id: "matrix-main".to_string(),
            channel_id: "!room:example.org".to_string(),
            content: "hello".to_string(),
            reply_to_external_message_id: "$event1".to_string(),
        },
    )
    .expect("send reply");
    assert_eq!(sent.external_message_id, "$reply1");
    assert!(
        saw_path.lock().expect("lock").contains(
            "/_matrix/client/v3/rooms/%21room:example.org/send/m.room.message/kura_event1"
        ),
        "unexpected send path: {}",
        saw_path.lock().expect("lock")
    );
    assert_eq!(*saw_auth.lock().expect("lock"), "Bearer matrix-token-do-not-leak");
    let body = saw_body.lock().expect("lock").clone();
    assert_eq!(body["msgtype"], "m.text");
    assert_eq!(body["body"], "hello");
}

#[test]
fn client_transport_start_consumes_sync_text_events() {
    let server = TestServer::start(|request| {
        assert_eq!(request.escaped_path, "/_matrix/client/v3/sync");
        assert_eq!(
            request.headers.get("authorization").map(String::as_str),
            Some("Bearer matrix-token-do-not-leak")
        );
        TestResponse {
            status: 200,
            body: r#"{"next_batch":"batch_2","rooms":{"join":{"!room:example.org":{"timeline":{"events":[{"type":"m.room.message","event_id":"$event1","sender":"@alice:example.org","origin_server_ts":1778407200000,"content":{"msgtype":"m.text","body":"@bot:example.org hello"}}]}}}}}"#
                .to_string(),
        }
    });

    let transport = new_client_transport(ClientTransportConfig {
        connector_id: "matrix-main".to_string(),
        homeserver_url: server.base_url.clone(),
        bot_access_token: "matrix-token-do-not-leak".to_string(),
        max_sync_cycles: 1,
        ..ClientTransportConfig::default()
    })
    .expect("transport");
    let events: Arc<StdMutex<Vec<InboundEvent>>> = Arc::new(StdMutex::new(Vec::new()));
    {
        let events = Arc::clone(&events);
        transport
            .start(&move |event: InboundEvent| {
                events.lock().expect("lock").push(event);
            })
            .expect("start");
    }
    let events = events.lock().expect("lock").clone();
    assert_eq!(events.len(), 1);
    let event = &events[0];
    assert_eq!(event.connector_id, "matrix-main");
    assert_eq!(event.homeserver_id, "example.org");
    assert_eq!(event.conversation_id, "!room:example.org");
    assert_eq!(event.matrix_event_id, "$event1");
    assert_eq!(event.sync_batch_id, "batch_2");
    assert_eq!(event.message_kind, MessageKind::UnencryptedText);
    assert_eq!(event.conversation_type, ConversationType::Room);
    assert_eq!(event.text, "@bot:example.org hello");
    assert_eq!(event.received_at, ts(2026, 5, 10, 10, 0, 0));
}

#[test]
fn client_transport_start_classifies_allowed_direct_sender() {
    let server = TestServer::start(|request| {
        assert_eq!(request.escaped_path, "/_matrix/client/v3/sync");
        TestResponse {
            status: 200,
            body: r#"{"next_batch":"batch_direct","rooms":{"join":{"!dm:example.org":{"timeline":{"events":[{"type":"m.room.message","event_id":"$direct1","sender":"@alice:example.org","origin_server_ts":1778407200000,"content":{"msgtype":"m.text","body":"hello from dm"}}]}}}}}"#
                .to_string(),
        }
    });

    let transport = new_client_transport(ClientTransportConfig {
        connector_id: "matrix-main".to_string(),
        homeserver_url: server.base_url.clone(),
        bot_access_token: "matrix-token-do-not-leak".to_string(),
        allowed_direct_user_ids: vec!["@alice:example.org".to_string()],
        selected_room_ids: vec!["!room:example.org".to_string()],
        max_sync_cycles: 1,
        ..ClientTransportConfig::default()
    })
    .expect("transport");
    let events: Arc<StdMutex<Vec<InboundEvent>>> = Arc::new(StdMutex::new(Vec::new()));
    {
        let events = Arc::clone(&events);
        transport
            .start(&move |event: InboundEvent| {
                events.lock().expect("lock").push(event);
            })
            .expect("start");
    }
    let events = events.lock().expect("lock").clone();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].conversation_type, ConversationType::DirectMessage);
    assert_eq!(events[0].conversation_id, "!dm:example.org");
    assert_eq!(events[0].sender_id, "@alice:example.org");
}

#[test]
fn client_transport_validates_bot_identity_and_room_membership() {
    let server = TestServer::start(|request| match request.escaped_path.as_str() {
        "/_matrix/client/v3/account/whoami" => TestResponse {
            status: 200,
            body: "{\"user_id\":\"@bot:example.org\",\"device_id\":\"DEVICE1\"}".to_string(),
        },
        "/_matrix/client/v3/rooms/%21room:example.org/state/m.room.member/@bot:example.org" => {
            TestResponse { status: 200, body: "{\"membership\":\"join\"}".to_string() }
        }
        other => panic!("unexpected path: {other}"),
    });

    let transport = new_client_transport(ClientTransportConfig {
        connector_id: "matrix-main".to_string(),
        homeserver_url: server.base_url.clone(),
        bot_access_token: "matrix-token-do-not-leak".to_string(),
        ..ClientTransportConfig::default()
    })
    .expect("transport");
    let (binding, result) = transport.validate_homeserver_binding(HomeserverBinding {
        tenant_id: "ten_matrix".to_string(),
        connector_id: "matrix-main".to_string(),
        homeserver_binding_id: "matrix_hs_1".to_string(),
        homeserver_url: server.base_url.clone(),
        bot_user_id: "@bot:example.org".to_string(),
        ..HomeserverBinding::default()
    });
    result.expect("binding validation");
    assert_eq!(binding.authorization_state, AuthorizationState::Valid);
    assert_eq!(binding.homeserver_capability_state, HomeserverCapabilityState::Valid);
    assert_eq!(binding.bot_device_id, "DEVICE1");

    let (policy, result) = transport.validate_route_policy(RoutePolicy {
        tenant_id: "ten_matrix".to_string(),
        connector_id: "matrix-main".to_string(),
        homeserver_binding_id: "matrix_hs_1".to_string(),
        selected_rooms: vec![ConversationRoute {
            conversation_id: "!room:example.org".to_string(),
            conversation_type: ConversationType::Room,
            ..ConversationRoute::default()
        }],
        validation_state: RoutePolicyState::Valid,
        ..RoutePolicy::default()
    });
    result.expect("policy validation");
    assert!(has_ready_route_policy(&policy));
}

#[test]
fn client_transport_requires_access_token() {
    let transport = new_client_transport(ClientTransportConfig {
        connector_id: "matrix-main".to_string(),
        homeserver_url: "https://matrix.example.org".to_string(),
        ..ClientTransportConfig::default()
    })
    .expect("transport");
    let err = Transport::send_reply(
        &transport,
        OutboundReply {
            channel_id: "!room:example.org".to_string(),
            content: "hello".to_string(),
            ..OutboundReply::default()
        },
    )
    .expect_err("send reply must require a token");
    assert_eq!(err, "matrix bot access token is not configured");
}

#[test]
fn execute_safe_live_smoke_validates_credential_route_and_send_path() {
    let sent_body: Arc<StdMutex<serde_json::Value>> = Arc::new(StdMutex::new(serde_json::Value::Null));
    let server = {
        let sent_body = Arc::clone(&sent_body);
        TestServer::start(move |request| match request.escaped_path.as_str() {
            "/_matrix/client/v3/account/whoami" => TestResponse {
                status: 200,
                body: "{\"user_id\":\"@bot:example.org\",\"device_id\":\"DEVICE1\"}".to_string(),
            },
            "/_matrix/client/v3/rooms/%21room:example.org/state/m.room.member/@bot:example.org" => {
                TestResponse { status: 200, body: "{\"membership\":\"join\"}".to_string() }
            }
            _ => {
                assert_eq!(request.method, "PUT");
                *sent_body.lock().expect("lock") =
                    serde_json::from_str(&request.body).unwrap_or(serde_json::Value::Null);
                TestResponse { status: 200, body: "{\"event_id\":\"$smoke_reply\"}".to_string() }
            }
        })
    };

    let transport = new_client_transport(ClientTransportConfig {
        connector_id: "matrix-main".to_string(),
        homeserver_url: server.base_url.clone(),
        bot_access_token: "matrix-token-do-not-leak".to_string(),
        ..ClientTransportConfig::default()
    })
    .expect("transport");
    let now = Utc::now();
    let evidence = execute_safe_live_smoke(SafeLiveSmokeInput {
        tenant_id: "ten_matrix".to_string(),
        connector_id: "matrix-main".to_string(),
        owner: "operator".to_string(),
        now,
        transport,
        binding: HomeserverBinding {
            tenant_id: "ten_matrix".to_string(),
            connector_id: "matrix-main".to_string(),
            homeserver_binding_id: "matrix_hs_1".to_string(),
            homeserver_url: server.base_url.clone(),
            bot_user_id: "@bot:example.org".to_string(),
            ..HomeserverBinding::default()
        },
        route_policy: RoutePolicy {
            tenant_id: "ten_matrix".to_string(),
            connector_id: "matrix-main".to_string(),
            homeserver_binding_id: "matrix_hs_1".to_string(),
            selected_rooms: vec![ConversationRoute {
                conversation_id: "!room:example.org".to_string(),
                conversation_type: ConversationType::Room,
                ..ConversationRoute::default()
            }],
            validation_state: RoutePolicyState::Valid,
            ..RoutePolicy::default()
        },
        smoke_room_id: "!room:example.org".to_string(),
    })
    .expect("safe-live smoke");
    assert_eq!(evidence.status, SmokeStatus::Passed);
    assert_eq!(evidence.authorization_mode, SmokeAuthorizationMode::SafeLive);
    assert_eq!(
        evidence.safe_evidence.get("eventId").map(String::as_str),
        Some("$smoke_reply")
    );
    let body = sent_body.lock().expect("lock").clone();
    assert_eq!(body["msgtype"], "m.text");
    assert!(!body["body"].as_str().unwrap_or_default().is_empty());
}

// ---------------------------------------------------------------------------
// Runtime (unit)
// ---------------------------------------------------------------------------

#[test]
fn normalize_inbound_event_trims_identity_and_applies_defaults() {
    let now = Utc::now();
    let event = normalize_inbound_event(InboundEvent {
        tenant_id: " ten_matrix ".to_string(),
        connector_id: " matrix-main ".to_string(),
        homeserver_id: " example.org ".to_string(),
        conversation_id: " !room:example.org ".to_string(),
        matrix_event_id: " $event ".to_string(),
        sender_id: " @alice:example.org ".to_string(),
        conversation_type: ConversationType::Room,
        text: "  @bot:example.org hello  ".to_string(),
        received_at: now,
        ..InboundEvent::default()
    });
    assert_eq!(event.tenant_id, "ten_matrix");
    assert_eq!(event.connector_id, "matrix-main");
    assert_eq!(event.homeserver_id, "example.org");
    assert_eq!(event.message_kind, MessageKind::UnencryptedText);
    assert_eq!(event.text, "@bot:example.org hello");
}

#[test]
fn matrix_rollback_disabled_connector_blocks_delivery_eligibility() {
    let setup = evaluate_hosted_setup(HostedSetupInput {
        tenant_id: "ten_matrix".to_string(),
        connector_id: "matrix-main".to_string(),
        display_name: "Matrix Main".to_string(),
        cancelled: true,
        provider_available: true,
        network_available: true,
        conformance_passed: true,
        bot_credential_state: BotCredentialState::Valid,
        ..HostedSetupInput::default()
    });
    assert_eq!(setup.terminal_state, TerminalState::Cancelled);
    assert!(!setup.delivery_eligible);
}

// ---------------------------------------------------------------------------
// Runtime (integration through the message loop)
// ---------------------------------------------------------------------------

/// Go matrixRuntimeTestProvider: echoes the first message content with a
/// reply: prefix.
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

/// A store-backed message loop with the echo provider registered, mirroring
/// the Go runtime test wiring (Go chat.NewService / checkpoints / router).
fn loop_harness(
    provider: Arc<dyn Provider>,
) -> (Arc<SQLiteStore>, MessageLoop, Bus, tempfile::TempDir) {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = Arc::new(SQLiteStore::new(dir.path().to_str().expect("path")).expect("store"));
    let bus = Bus::new();
    let dispatcher = Arc::new(Dispatcher::new());
    dispatcher.register_provider(provider);
    dispatcher.set_default_provider("echo").expect("default provider");
    dispatcher.set_default_model("echo-v1");
    let chat = Service::new_service(dispatcher, None, None, Some(bus.clone()), None);
    let runtime_manager = Arc::new(RuntimeManager::new());
    let checkpoints = CheckpointManager::new(
        Arc::new(parking_lot::Mutex::new(
            SQLiteStore::new(dir.path().to_str().expect("path")).expect("store"),
        )),
        runtime_manager.clone(),
    );
    let message_loop = MessageLoop::new(
        SessionRouter::new(),
        runtime_manager,
        Some(checkpoints),
        Some(bus.clone()),
        store.clone(),
        chat,
    );
    (store, message_loop, bus, dir)
}

fn ready_hosted_setup_record(now: DateTime<Utc>) -> MatrixHostedSetupRecord {
    MatrixHostedSetupRecord {
        tenant_id: "ten_matrix_runtime".to_string(),
        connector_id: "matrix-main".to_string(),
        connector_kind: "matrix".to_string(),
        display_name: "Matrix Main".to_string(),
        status: "healthy".to_string(),
        terminal_state: "ready".to_string(),
        bot_credential_state: "valid".to_string(),
        homeserver_state: "reachable".to_string(),
        route_policy_state: "valid".to_string(),
        delivery_eligible: true,
        homeserver_binding_id: "matrix_homeserver_matrix-main".to_string(),
        reason_code: "healthy".to_string(),
        redaction_status: "redacted".to_string(),
        created_at: now,
        updated_at: now,
        validated_at: Some(now),
        retention_expires_at: now + Duration::days(90),
        homeserver_binding: None,
        route_policy: None,
    }
}

fn runtime_config(commands: bool) -> Config {
    let mut cfg = Config {
        enabled: true,
        connector_id: "matrix-main".to_string(),
        display_name: "Matrix Main".to_string(),
        homeserver_id: "matrix.example.org".to_string(),
        bot_user_id: "@bot:example.org".to_string(),
        selected_room_ids: vec!["!room:example.org".to_string()],
        ..Config::default()
    };
    if commands {
        cfg.configured_commands = vec!["!kura".to_string()];
    }
    cfg
}

fn runtime_event(now: DateTime<Utc>) -> InboundEvent {
    InboundEvent {
        homeserver_id: "matrix.example.org".to_string(),
        conversation_id: "!room:example.org".to_string(),
        matrix_event_id: "$event1".to_string(),
        sender_id: "@alice:example.org".to_string(),
        conversation_type: ConversationType::Room,
        message_kind: MessageKind::UnencryptedText,
        text: "@bot:example.org hello matrix".to_string(),
        received_at: now,
        ..InboundEvent::default()
    }
}

#[test]
fn runtime_routes_accepted_matrix_event_through_message_loop() {
    let (store, message_loop, bus, _dir) = loop_harness(Arc::new(EchoTestProvider));
    let now = Utc::now();
    store
        .save_matrix_hosted_setup(&ready_hosted_setup_record(now))
        .expect("save hosted setup");

    let transport = FakeTransport::new(vec![runtime_event(now)]);
    let transport_handle = transport.clone();
    let runtime = new_runtime(
        runtime_config(true),
        Arc::new(Supervisor::new()),
        message_loop,
        Some(store.clone()),
        Some(bus),
        Some(Box::new(transport)),
    )
    .expect("new runtime")
    .expect("runtime enabled");
    runtime.start("ten_matrix_runtime").expect("start");

    let replies = transport_handle.sent_replies();
    assert_eq!(replies.len(), 1);
    assert_eq!(replies[0].content, "reply:hello matrix");
    assert_eq!(replies[0].reply_to_external_message_id, "$event1");

    let runs = store.list_runs_all_tenants_for_test().expect("list runs");
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0].entrypoint, "matrix.message");

    let evidence = store
        .list_matrix_event_evidence("ten_matrix_runtime", "matrix-main", now, 10)
        .expect("list evidence");
    assert_eq!(evidence.len(), 1);
    assert_eq!(evidence[0].route_outcome, "accepted");
    assert_eq!(evidence[0].matrix_event_id, "$event1");
}

#[test]
fn runtime_classifies_persisted_matrix_event_replay_as_duplicate_after_restart() {
    let (store, message_loop, bus, _dir) = loop_harness(Arc::new(EchoTestProvider));
    let now = Utc::now();
    store
        .save_matrix_hosted_setup(&ready_hosted_setup_record(now))
        .expect("save hosted setup");

    let first_transport = FakeTransport::new(vec![runtime_event(now)]);
    let first_transport_handle = first_transport.clone();
    let first_runtime = new_runtime(
        runtime_config(false),
        Arc::new(Supervisor::new()),
        message_loop,
        Some(store.clone()),
        Some(bus.clone()),
        Some(Box::new(first_transport)),
    )
    .expect("new runtime")
    .expect("runtime enabled");
    first_runtime.start("ten_matrix_runtime").expect("first start");

    let second_transport = FakeTransport::new(vec![runtime_event(now)]);
    let second_transport_handle = second_transport.clone();
    let (_, second_loop, _, _) = loop_harness(Arc::new(EchoTestProvider));
    let second_runtime = new_runtime(
        runtime_config(false),
        Arc::new(Supervisor::new()),
        second_loop,
        Some(store.clone()),
        Some(bus),
        Some(Box::new(second_transport)),
    )
    .expect("new runtime")
    .expect("runtime enabled");
    second_runtime.start("ten_matrix_runtime").expect("second start");

    assert_eq!(first_transport_handle.sent_replies().len(), 1);
    assert_eq!(
        second_transport_handle.sent_replies().len(),
        0,
        "replay after restart must suppress the reply"
    );

    let evidence = store
        .list_matrix_event_evidence("ten_matrix_runtime", "matrix-main", now, 10)
        .expect("list evidence");
    assert_eq!(evidence.len(), 1);
    assert_eq!(evidence[0].route_outcome, "duplicate");
}
