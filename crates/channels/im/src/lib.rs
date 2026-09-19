//! Port of `daemon/internal/im/loop.go` — the connector MessageLoop.
//!
//! The loop drives one inbound IM turn end to end: durable dedupe, session
//! routing, thread lifecycle + group/room participation policy, run/step
//! creation, final or streaming chat reply, reply delivery, and the
//! connector/thread/runtime evidence events. See rs/MIGRATION.md for the
//! porting conventions; errors surface as `String` (the loop maps
//! RuntimeError/ChatError/store errors via `to_string()`, matching the Go
//! package's plain `error` returns).

#![allow(non_camel_case_types)]

use std::collections::HashMap;
use std::error::Error;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use chrono::{DateTime, Utc};
use kura_chat::{CancellationToken, ChatError, QueryInput, Service as ChatService, StreamChunk};
use kura_checkpoints::Manager as CheckpointManager;
use kura_connectors::{Connector, RedactionStatus as ConnectorRedactionStatus, Status};
use kura_events::{Bus, Event, Resource, Scope};
use kura_imtypes::{
    DeliveryDirection, DeliveryStatus, InboundMessage, MessageRecord, OutboundReply,
    ReplyCapabilities, ReplyEdit, SentReply, ThinkingSignal,
};
use kura_router::{RouteInput, Session, SessionKind, SessionRouter, SessionStatus};
use kura_runtime::{
    CreateRunInput, CreateStepInput, Manager as RuntimeManager, Run, Step, StepStatus,
    UpdateStepStatusInput,
};
use kura_store::SQLiteStore;
use kura_store::channel_management::{
    ForegroundReplyOutcome, route_policy_allows_conversation, route_policy_allows_sender,
    route_policy_is_valid,
};
use kura_threads::{
    ConversationShape, ConversationShapeResolutionInput, LifecycleState, ParticipationDecision,
    ParticipationDecisionValue, ParticipationEvaluationInput, RedactionStatus, RoutingOutcome,
    RuntimeProjection, RuntimeProjectionInput, RuntimeResourceKind, SessionSegment,
    SourceContinuationKey, SourceKind, SourceLinkage, Thread, build_runtime_projection,
    evaluate_participation, normalize_source_continuation_key, resolve_conversation_shape,
};
use serde_json::{Map, Value};

pub use kura_chat::QueryResult;

// ---------------------------------------------------------------------------
// Reply sender interfaces (Go ReplySender / ReplyProgressor)
// ---------------------------------------------------------------------------

/// Port of the Go ReplySender interface: delivers a final outbound reply.
///
/// The optional reply_progressor hook emulates Go's
/// replies.(ReplyProgressor) type assertion: implementors of
/// ReplyProgressor return Some(self), plain senders leave it None.
pub trait ReplySender: Send + Sync {
    fn send_reply(&self, reply: OutboundReply) -> Result<SentReply, String>;

    /// Go's replies.(ReplyProgressor) — Some(self) when the sender also
    /// implements ReplyProgressor.
    fn reply_progressor(&self) -> Option<&dyn ReplyProgressor> {
        None
    }
}

/// Port of the Go ReplyProgressor interface: streaming-capable senders can
/// report capabilities, emit thinking indicators, and edit previously sent
/// replies.
pub trait ReplyProgressor: ReplySender {
    fn reply_capabilities(&self) -> ReplyCapabilities;
    fn send_thinking(&self, signal: ThinkingSignal) -> Result<(), String>;
    fn edit_reply(&self, edit: ReplyEdit) -> Result<(), String>;
}

// ---------------------------------------------------------------------------
// Error classification (Go classifiedError / classifyError)
// ---------------------------------------------------------------------------

/// An error carrying a stable error class. Go's classifiedError interface
/// (the loop's classifyError returns the class of any classified error and
/// the empty string otherwise).
#[derive(Debug, thiserror::Error)]
#[error("{class}")]
pub struct ClassifiedError {
    class: String,
    #[source]
    source: Box<dyn Error + Send + Sync>,
}

impl ClassifiedError {
    /// Wraps source under the error class class.
    pub fn new(class: impl Into<String>, source: Box<dyn Error + Send + Sync>) -> Self {
        ClassifiedError {
            class: class.into(),
            source,
        }
    }

    /// Go ErrorClass() — the trimmed class.
    #[must_use]
    pub fn error_class(&self) -> String {
        self.class.trim().to_string()
    }
}

/// Go classifyError: the error class of a ClassifiedError, or the empty
/// string for None/unclassified errors.
#[must_use]
pub fn classify_error(err: Option<&(dyn Error + 'static)>) -> String {
    let Some(err) = err else {
        return String::new();
    };
    if let Some(classified) = err.downcast_ref::<ClassifiedError>() {
        return classified.error_class();
    }
    String::new()
}

/// Go safeReplyFailureReason: the error class when one exists, otherwise
/// the stable reply_failed reason.
#[must_use]
pub fn safe_reply_failure_reason(err: Option<&(dyn Error + 'static)>) -> String {
    let class = classify_error(err);
    if !class.is_empty() {
        class
    } else {
        "reply_failed".to_string()
    }
}

// ---------------------------------------------------------------------------
// ProcessResult + MessageLoop
// ---------------------------------------------------------------------------

/// Outcome of one processed inbound turn. Mirrors Go ProcessResult.
#[derive(Debug, Clone)]
pub struct ProcessResult {
    pub session: Session,
    pub run: Run,
    pub step: Step,
    pub reply: String,
    pub outcome: String,
    pub reason_code: String,
    pub duplicate: bool,
}

/// The connector message loop. Go MessageLoop.
pub struct MessageLoop {
    router: SessionRouter,
    runtime: Arc<RuntimeManager>,
    checkpoints: Option<CheckpointManager>,
    event_bus: Option<Bus>,
    store: Arc<SQLiteStore>,
    chat: ChatService,
}

impl MessageLoop {
    /// Go NewMessageLoop.
    #[must_use]
    pub fn new(
        session_router: SessionRouter,
        runtime_manager: Arc<RuntimeManager>,
        checkpoint_manager: Option<CheckpointManager>,
        event_bus: Option<Bus>,
        sqlite_store: Arc<SQLiteStore>,
        chat_service: ChatService,
    ) -> MessageLoop {
        MessageLoop {
            router: session_router,
            runtime: runtime_manager,
            checkpoints: checkpoint_manager,
            event_bus,
            store: sqlite_store,
            chat: chat_service,
        }
    }
}

// ---------------------------------------------------------------------------
// ID helpers (Go newDeliveryID / newEventID / shortThreadHash)
// ---------------------------------------------------------------------------

/// Go newDeliveryID: delivery_ + 8 random bytes hex-encoded.
#[must_use]
pub fn new_delivery_id() -> String {
    let hex = uuid::Uuid::new_v4().simple().to_string();
    if hex.len() < 16 {
        return "delivery_fallback".to_string();
    }
    format!("delivery_{}", &hex[..16])
}

/// Go newEventID: evt_ + 8 random bytes hex-encoded.
#[must_use]
pub fn new_event_id() -> String {
    let hex = uuid::Uuid::new_v4().simple().to_string();
    if hex.len() < 16 {
        return "evt_fallback".to_string();
    }
    format!("evt_{}", &hex[..16])
}

/// Go shortThreadHash: first 24 hex chars of the SHA-256 of the value.
#[must_use]
pub fn short_thread_hash(value: &str) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(value.as_bytes());
    let sum = hasher.finalize();
    hex_encode(&sum)[..24].to_string()
}

fn hex_encode(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(HEX[(byte >> 4) as usize] as char);
        out.push(HEX[(byte & 0x0f) as usize] as char);
    }
    out
}

// ---------------------------------------------------------------------------
// Free helpers (Go package-level functions)
// ---------------------------------------------------------------------------

/// Go coalesceTrimmed: first trimmed non-empty value.
#[must_use]
pub fn coalesce_trimmed(values: &[&str]) -> String {
    for value in values {
        let trimmed = value.trim();
        if !trimmed.is_empty() {
            return trimmed.to_string();
        }
    }
    String::new()
}

/// Go inboundConnectorAccountID.
#[must_use]
pub fn inbound_connector_account_id(inbound: &InboundMessage) -> String {
    coalesce_trimmed(&[&inbound.connector_account_id, &inbound.account_id])
}

/// Go inboundChannelOrConversationID.
#[must_use]
pub fn inbound_channel_or_conversation_id(inbound: &InboundMessage) -> String {
    coalesce_trimmed(&[
        &inbound.channel_or_conversation_id,
        &inbound.channel_id,
        &inbound.peer_id,
    ])
}

/// Go inboundProviderMessageID.
#[must_use]
pub fn inbound_provider_message_id(inbound: &InboundMessage) -> String {
    coalesce_trimmed(&[&inbound.provider_message_id, &inbound.external_message_id])
}

/// Go conversationShapeForIngressSource.
#[must_use]
pub fn conversation_shape_for_ingress_source(
    source_kind: SourceKind,
    connector_kind: &str,
    inbound: &InboundMessage,
) -> ConversationShape {
    match source_kind {
        SourceKind::Shell => return ConversationShape::Web,
        SourceKind::Channel => {}
        // Go returns the empty string (zero shape) for every other source kind;
        // the Rust port maps it to Unsupported so the shape evidence resolves
        // identically (empty claim + Channel-classified source -> unsupported).
        _ => return ConversationShape::Unsupported,
    }
    if inbound.direct || inbound.kind == SessionKind::Direct {
        return ConversationShape::DirectMessage;
    }
    if inbound.kind != SessionKind::Group {
        return ConversationShape::Unsupported;
    }
    match connector_kind.trim().to_lowercase().as_str() {
        "matrix" | "slack" => ConversationShape::Room,
        "discord" => {
            if !inbound.guild_id.trim().is_empty() {
                ConversationShape::Room
            } else {
                ConversationShape::Group
            }
        }
        _ => {
            if !inbound.guild_id.trim().is_empty() {
                ConversationShape::Room
            } else {
                ConversationShape::Group
            }
        }
    }
}

/// Go connectorEnforcesGroupRoomParticipationPolicy: both group-room surfaces
/// (mention evidence + allowlist evidence) must be supported.
#[must_use]
pub fn connector_enforces_group_room_participation_policy(connector: &Connector) -> bool {
    if connector.capability_profile.is_empty() {
        return false;
    }
    connector_surface_supported(
        &connector.capability_profile,
        kura_connectors::GROUP_ROOM_SURFACE_MENTION_EVIDENCE,
    ) && connector_surface_supported(
        &connector.capability_profile,
        kura_connectors::GROUP_ROOM_SURFACE_ALLOWLIST_EVIDENCE,
    )
}

/// Go connectorSurfaceSupported: a capability-profile surface key is supported
/// when its value is the supported literal, a true boolean, or a serialized
/// supported enum.
#[must_use]
pub fn connector_surface_supported(profile: &Map<String, Value>, key: &str) -> bool {
    let Some(value) = profile.get(key) else {
        return false;
    };
    match value {
        Value::String(typed) => typed.trim().eq_ignore_ascii_case("supported"),
        Value::Bool(typed) => *typed,
        _ => false,
    }
}

/// Go connectorSource.
#[must_use]
pub fn connector_source(kind: &str) -> String {
    let kind = kind.trim();
    if kind.is_empty() {
        return "connector".to_string();
    }
    format!("connector.{kind}")
}

/// Go matrixRouteSurface.
#[must_use]
pub fn matrix_route_surface(record: &MessageRecord) -> &'static str {
    if !record.peer_id.trim().is_empty() && record.channel_id.trim().is_empty() {
        return "direct";
    }
    "room"
}

/// Go foregroundReplyOutcomeStatus.
#[must_use]
pub fn foreground_reply_outcome_status(status: DeliveryStatus) -> &'static str {
    match status {
        DeliveryStatus::Replied => "sent",
        DeliveryStatus::Partial => "partial",
        DeliveryStatus::Failed => "failed",
        _ => "processing",
    }
}

/// Go replyToExternalMessageID.
#[must_use]
pub fn reply_to_external_message_id(inbound: &InboundMessage) -> String {
    let reply_to = inbound.reply_to_message_id.trim();
    if !reply_to.is_empty() {
        return reply_to.to_string();
    }
    inbound.external_message_id.clone()
}

/// Go threadIDForSource.
#[must_use]
pub fn thread_id_for_source(key: &SourceContinuationKey) -> String {
    format!("thr_src_{}", short_thread_hash(&key.to_string()))
}

/// Go threadSourceLinkageID.
#[must_use]
pub fn thread_source_linkage_id(record: &MessageRecord, outcome: RoutingOutcome) -> String {
    format!(
        "src_{}",
        short_thread_hash(&format!(
            "{}:{}",
            record.delivery_id,
            routing_outcome_str(outcome)
        ))
    )
}

/// Go bindingChannelScopeRef: the stable connector-qualified channel identity
/// used to resolve channel-scoped binding rules (FR-006).
#[must_use]
pub fn binding_channel_scope_ref(record: &MessageRecord) -> String {
    let mut channel = record.channel_id.trim().to_string();
    if channel.is_empty() {
        channel = record.channel_or_conversation_id.trim().to_string();
    }
    if channel.is_empty() {
        return String::new();
    }
    let connector_id = record.connector_id.trim();
    if connector_id.is_empty() {
        return channel;
    }
    format!("{connector_id}:{channel}")
}

/// Go sourceContinuationKey: normalizes the tenant/connector/source identity
/// into the stable continuation key.
pub fn source_continuation_key(
    connector: &Connector,
    inbound: &InboundMessage,
) -> Result<SourceContinuationKey, kura_threads::ThreadsError> {
    normalize_source_continuation_key(&SourceContinuationKey {
        tenant_id: coalesce_trimmed(&[&inbound.tenant_id, &connector.tenant_id]),
        connector_id: coalesce_trimmed(&[&connector.connector_id, &inbound.connector_id]),
        source_account_id: inbound_connector_account_id(inbound),
        source_conversation_id: inbound_channel_or_conversation_id(inbound),
    })
}

/// Go classifyRoutingOutcome: maps routing error text to the source routing
/// outcome bucket.
#[must_use]
pub fn classify_routing_outcome(err: &dyn Error) -> RoutingOutcome {
    let lower = err.to_string().to_lowercase();
    if lower.contains("disabled") {
        RoutingOutcome::Disabled
    } else if lower.contains("unsupported") {
        RoutingOutcome::Unsupported
    } else if lower.contains("stale") {
        RoutingOutcome::StaleSource
    } else if lower.contains("tenant") {
        RoutingOutcome::InaccessibleTenantBinding
    } else if lower.contains("source") || lower.contains("routing key") {
        RoutingOutcome::UnknownSource
    } else {
        RoutingOutcome::Failed
    }
}

/// Go splitReplyContent: splits the reply on rune boundaries into parts of at
/// most max_message_length runes; non-positive limits return one part.
#[must_use]
pub fn split_reply_content(reply: &str, max_message_length: i64) -> Vec<String> {
    if max_message_length <= 0 {
        return vec![reply.to_string()];
    }
    let max = max_message_length as usize;
    let chars: Vec<char> = reply.chars().collect();
    if chars.len() <= max {
        return vec![reply.to_string()];
    }
    let mut parts = Vec::with_capacity((chars.len() + max - 1) / max);
    let mut start = 0;
    while start < chars.len() {
        let end = (start + max).min(chars.len());
        parts.push(chars[start..end].iter().collect());
        start = end;
    }
    parts
}

/// Go appendPartialMarker.
#[must_use]
pub fn append_partial_marker(reply: &str) -> String {
    const SUFFIX: &str = "\n\n[response interrupted]";
    let reply = reply.trim();
    if reply.is_empty() {
        return reply.to_string();
    }
    if reply.ends_with(SUFFIX) {
        return reply.to_string();
    }
    format!("{reply}{SUFFIX}")
}

/// Extracts the provider error text from a chat exec error: the
/// ChatError::Dispatch payload already carries the provider message-or-code
/// (Go surfaces the ProviderError whose Error() is the message), while other
/// errors keep their Display text.
fn chat_exec_error_text(err: ChatError) -> String {
    match err {
        ChatError::Dispatch(message) => message,
        other => other.to_string(),
    }
}
/// Go partialDeliveryError: the provider error message (or code) when the
/// cause is a provider error, else the error text. The Rust chat service
/// surfaces provider errors through ChatError::Dispatch carrying the provider
/// message-or-code, so the string itself is the faithful text.
#[must_use]
pub fn partial_delivery_error(err: Option<&str>) -> String {
    match err {
        None => String::new(),
        Some(err) => err.to_string(),
    }
}

/// Go string(threads.RoutingOutcome).
#[must_use]
pub fn routing_outcome_str(outcome: RoutingOutcome) -> &'static str {
    match outcome {
        RoutingOutcome::Accepted => "accepted",
        RoutingOutcome::Ignored => "ignored",
        RoutingOutcome::Blocked => "blocked",
        RoutingOutcome::Duplicate => "duplicate",
        RoutingOutcome::Disabled => "disabled",
        RoutingOutcome::Unsupported => "unsupported",
        RoutingOutcome::Failed => "failed",
        RoutingOutcome::UnknownSource => "unknown_source",
        RoutingOutcome::StaleSource => "stale_source",
        RoutingOutcome::InaccessibleTenantBinding => "inaccessible_tenant_binding",
    }
}

/// Go string(threads.ParticipationDecision).
#[must_use]
pub fn participation_value_str(decision: ParticipationDecisionValue) -> &'static str {
    match decision {
        ParticipationDecisionValue::Accepted => "accepted",
        ParticipationDecisionValue::Ignored => "ignored",
        ParticipationDecisionValue::Blocked => "blocked",
        ParticipationDecisionValue::Denied => "denied",
        ParticipationDecisionValue::Duplicate => "duplicate",
        ParticipationDecisionValue::Unsupported => "unsupported",
        ParticipationDecisionValue::Failed => "failed",
    }
}

/// Go string(threads.RedactionStatus).
#[must_use]
pub fn redaction_status_str(status: RedactionStatus) -> &'static str {
    match status {
        RedactionStatus::Redacted => "redacted",
        RedactionStatus::Suppressed => "suppressed",
        RedactionStatus::RedactionFailed => "redaction_failed",
    }
}

/// Go string(router.SessionStatus); the only wire value is active.
#[must_use]
pub fn session_status_str(status: SessionStatus) -> &'static str {
    match status {
        SessionStatus::Active => "active",
    }
}

fn is_unset_time(dt: &DateTime<Utc>) -> bool {
    *dt == DateTime::<Utc>::MIN_UTC
}

/// Builds a serde object payload from a json!-style value.
fn object(value: Value) -> Map<String, Value> {
    value.as_object().cloned().unwrap_or_default()
}

/// Builds a string map (Go map[string]string) from a json!-style object.
fn string_map(value: Value) -> HashMap<String, String> {
    let mut map = HashMap::new();
    if let Some(obj) = value.as_object() {
        for (key, value) in obj {
            map.insert(
                key.clone(),
                match value {
                    Value::String(s) => s.clone(),
                    Value::Bool(b) => b.to_string(),
                    Value::Number(n) => n.to_string(),
                    _ => value.to_string(),
                },
            );
        }
    }
    map
}

impl MessageLoop {
    /// Go ProcessSingleTurn: drives one inbound turn through dedupe, routing,
    /// thread lifecycle, participation policy, run/step creation, reply
    /// execution/delivery, and evidence publishing.
    pub fn process_single_turn(
        &self,
        connector: &Connector,
        inbound: &InboundMessage,
        replies: &dyn ReplySender,
        cancel: &CancellationToken,
    ) -> Result<ProcessResult, String> {
        if inbound.external_message_id.trim().is_empty() {
            return Err("external message id is required".to_string());
        }
        if inbound.content.trim().is_empty() {
            return Err("content is required".to_string());
        }

        let now = if is_unset_time(&inbound.received_at) {
            Utc::now()
        } else {
            inbound.received_at
        };

        let inbound_record = MessageRecord {
            delivery_id: new_delivery_id(),
            tenant_id: inbound.tenant_id.clone(),
            connector_id: connector.connector_id.clone(),
            direction: DeliveryDirection::Inbound,
            external_message_id: inbound.external_message_id.clone(),
            connector_account_id: inbound_connector_account_id(inbound),
            channel_or_conversation_id: inbound_channel_or_conversation_id(inbound),
            provider_message_id: inbound_provider_message_id(inbound),
            equivalent_rule_id: inbound.equivalent_rule_id.clone(),
            channel_id: inbound.channel_id.clone(),
            peer_id: inbound.peer_id.clone(),
            thread_id: inbound.thread_id.clone(),
            author_id: inbound.author_id.clone(),
            content: inbound.content.clone(),
            status: DeliveryStatus::Received,
            reply_to_external_message_id: inbound.reply_to_message_id.clone(),
            created_at: now,
            updated_at: now,
            ..MessageRecord::default()
        };
        let (mut persisted_inbound, created) = self
            .store
            .create_connector_message_if_absent(&inbound_record)
            .map_err(|e| e.to_string())?;
        if !created {
            let _ = self.record_duplicate_thread_evidence(connector, inbound, &persisted_inbound);
            let _ = self.publish_connector_event(
                "connector.inbound_duplicate_detected",
                connector,
                &empty_session(),
                "",
                "",
                object(serde_json::json!({
                    "tenantId": persisted_inbound.tenant_id,
                    "connectorId": connector.connector_id,
                    "connectorAccountId": persisted_inbound.connector_account_id,
                    "channelOrConversationId": persisted_inbound.channel_or_conversation_id,
                    "providerMessageId": persisted_inbound.provider_message_id,
                    "equivalentRuleId": persisted_inbound.equivalent_rule_id,
                    "existingDeliveryId": persisted_inbound.delivery_id,
                    "redactionStatus": "redacted",
                })),
            );
            let _ = self.publish_matrix_route_outcome(
                connector,
                &empty_session(),
                &persisted_inbound,
                "duplicate",
                "duplicate_inbound",
            );
            return Ok(ProcessResult {
                session: empty_session(),
                run: Run::default(),
                step: Step::default(),
                reply: String::new(),
                outcome: "duplicate".to_string(),
                reason_code: "duplicate_inbound".to_string(),
                duplicate: true,
            });
        }

        if self.block_archived_source_continuation(connector, inbound, &mut persisted_inbound)? {
            return Ok(ProcessResult {
                session: empty_session(),
                run: Run::default(),
                step: Step::default(),
                reply: String::new(),
                outcome: "blocked".to_string(),
                reason_code: "thread_archived".to_string(),
                duplicate: false,
            });
        }

        let (session, created_session) = match self.route_session(inbound) {
            Ok(pair) => pair,
            Err(err) => {
                persisted_inbound.status = DeliveryStatus::Failed;
                persisted_inbound.error = err.to_string();
                persisted_inbound.updated_at = Utc::now();
                let _ = self.store.upsert_connector_message(&persisted_inbound);
                let _ = self.record_routing_only_source_evidence(
                    connector,
                    inbound,
                    &persisted_inbound,
                    classify_routing_outcome(&err),
                    &classify_error(Some(&err)),
                );
                return Err(err.to_string());
            }
        };
        if let Err(err) = self.persist_session(&session) {
            persisted_inbound.status = DeliveryStatus::Failed;
            persisted_inbound.error = err.to_string();
            persisted_inbound.updated_at = Utc::now();
            let _ = self.store.upsert_connector_message(&persisted_inbound);
            return Err(err);
        }
        let (thread, segment_id) = match self.ensure_thread_lifecycle_for_inbound(
            connector,
            inbound,
            &session,
            &persisted_inbound,
        ) {
            Ok(pair) => pair,
            Err(err) => {
                persisted_inbound.status = DeliveryStatus::Failed;
                persisted_inbound.error = err.to_string();
                persisted_inbound.updated_at = Utc::now();
                let _ = self.store.upsert_connector_message(&persisted_inbound);
                return Err(err);
            }
        };
        persisted_inbound.session_id = session.session_id.clone();
        if !thread.thread_id.is_empty() {
            persisted_inbound.thread_id = thread.thread_id.clone();
            persisted_inbound.thread_session_segment_id = segment_id.clone();
        }
        persisted_inbound.updated_at = Utc::now();
        self.store.upsert_connector_message(&persisted_inbound)?;
        let (participation_decision, participation_enforced) = self
            .apply_group_room_participation_policy(
                connector,
                inbound,
                &thread,
                &segment_id,
                &persisted_inbound,
            )?;
        if participation_enforced
            && participation_decision.decision != ParticipationDecisionValue::Accepted
        {
            persisted_inbound.error = participation_decision.reason_code.clone();
            persisted_inbound.updated_at = Utc::now();
            self.store.upsert_connector_message(&persisted_inbound)?;
            return Ok(ProcessResult {
                session: session.clone(),
                run: Run::default(),
                step: Step::default(),
                reply: String::new(),
                outcome: participation_value_str(participation_decision.decision).to_string(),
                reason_code: participation_decision.reason_code.clone(),
                duplicate: false,
            });
        }
        self.publish_session_route_events(connector, &session, created_session, inbound)?;
        self.publish_matrix_route_outcome(
            connector,
            &session,
            &persisted_inbound,
            "accepted",
            "accepted",
        )?;
        self.publish_connector_event(
            "connector.ingress_accepted",
            connector,
            &session,
            "",
            "",
            object(serde_json::json!({
                "messageId": inbound.external_message_id,
                "channelId": inbound.channel_id,
                "authorId": inbound.author_id,
                "direct": inbound.direct,
            })),
        )?;

        let (run, step) = match self.create_run_and_step(connector, &session, inbound) {
            Ok(pair) => pair,
            Err(err) => {
                persisted_inbound.status = DeliveryStatus::Failed;
                persisted_inbound.error = err.to_string();
                persisted_inbound.updated_at = Utc::now();
                let _ = self.store.upsert_connector_message(&persisted_inbound);
                return Err(err);
            }
        };
        persisted_inbound.run_id = run.run_id.clone();
        persisted_inbound.status = DeliveryStatus::Processing;
        persisted_inbound.updated_at = Utc::now();
        self.store.upsert_connector_message(&persisted_inbound)?;

        self.update_step_status(
            &run.run_id,
            &step.step_id,
            UpdateStepStatusInput {
                status: StepStatus::Planning,
                output: None,
            },
        )?;
        self.update_step_status(
            &run.run_id,
            &step.step_id,
            UpdateStepStatusInput {
                status: StepStatus::CallingModel,
                output: None,
            },
        )?;

        let event_scope = Scope {
            session_id: session.session_id.clone(),
            run_id: run.run_id.clone(),
            connector_id: connector.connector_id.clone(),
            ..Scope::default()
        };
        let progressor = replies.reply_progressor();
        let mut capabilities = ReplyCapabilities::default();
        if let Some(progressor) = progressor {
            capabilities = progressor.reply_capabilities();
        }
        let stop_flag = Arc::new(AtomicBool::new(false));
        let thinking_started = self.start_thinking_progress(
            connector,
            &session,
            &run.run_id,
            &step.step_id,
            inbound,
            progressor,
            &capabilities,
            &stop_flag,
        );

        std::thread::scope(|thread_scope| -> Result<ProcessResult, String> {
            if thinking_started {
                let flag = Arc::clone(&stop_flag);
                let signal = ThinkingSignal {
                    connector_id: connector.connector_id.clone(),
                    channel_id: inbound.channel_id.clone(),
                };
                let progressor_ref = progressor.expect("thinking_started implies a progressor");
                // The ticker cannot capture &MessageLoop (the SQLite connection is not
                // Sync), so it receives the Send pieces it needs: the event bus for the
                // thinking_failed evidence and the scope identifiers.
                let bus = self.event_bus.clone();
                let connector_id = connector.connector_id.clone();
                let session_id = session.session_id.clone();
                let run_id = run.run_id.clone();
                let step_id = step.step_id.clone();
                let message_id = inbound.external_message_id.clone();
                let channel_id = inbound.channel_id.clone();
                thread_scope.spawn(move || {
                    tick_thinking(
                        bus,
                        connector_id,
                        session_id,
                        run_id,
                        step_id,
                        message_id,
                        channel_id,
                        progressor_ref,
                        signal,
                        flag,
                    );
                });
            }
            let stop = ThinkingStop {
                flag: Arc::clone(&stop_flag),
            };
            let (query_result, outbound_record, send_err) = self.execute_reply_path(
                connector,
                &session,
                &run,
                &step,
                inbound,
                &persisted_inbound,
                replies,
                progressor,
                &capabilities,
                event_scope,
                &stop,
                cancel,
            )?;
            if let Some(send_err) = send_err {
                return self.finish_failed_reply(
                    connector,
                    &session,
                    &run,
                    &step,
                    inbound,
                    &thread,
                    &segment_id,
                    &mut persisted_inbound,
                    outbound_record,
                    query_result,
                    send_err,
                );
            }

            stop.stop();
            let final_step = self.update_step_status(
                &run.run_id,
                &step.step_id,
                UpdateStepStatusInput {
                    status: StepStatus::Completed,
                    output: Some(object(serde_json::json!({
                        "reply": query_result.dispatch.output,
                        "replyMessageId": outbound_record.external_message_id,
                        "llmDispatchId": query_result.dispatch.dispatch_id,
                        "llmProvider": query_result.dispatch.provider,
                        "llmModel": query_result.dispatch.model,
                        "llmUsage": serde_json::to_value(&query_result.dispatch.usage).unwrap_or(Value::Null),
                        "replyToExternalMessageId": reply_to_external_message_id(inbound),
                    })).into()),
                },
            )?;
            let run = self
                .runtime
                .get_run(&run.run_id)
                .unwrap_or_else(|| run.clone());
            self.record_thread_runtime_projections(
                &thread,
                &segment_id,
                &session,
                &run,
                &persisted_inbound,
                &outbound_record,
                run.status.as_str(),
                "accepted",
            )?;
            Ok(ProcessResult {
                session: session.clone(),
                run,
                step: final_step,
                reply: query_result.dispatch.output,
                outcome: "accepted".to_string(),
                reason_code: "accepted".to_string(),
                duplicate: false,
            })
        })
    }

    /// Go failure branch of ProcessSingleTurn: marks the step/inbound record
    /// failed (or partial), records thread runtime projections, publishes the
    /// connector reply-failure evidence, and returns the failure.
    fn finish_failed_reply(
        &self,
        connector: &Connector,
        session: &Session,
        run: &Run,
        step: &Step,
        inbound: &InboundMessage,
        thread: &Thread,
        segment_id: &str,
        persisted_inbound: &mut MessageRecord,
        mut outbound_record: MessageRecord,
        query_result: QueryResult,
        send_err: String,
    ) -> Result<ProcessResult, String> {
        let partial_reply =
            query_result.dispatch.partial || outbound_record.status == DeliveryStatus::Partial;
        // String errors are never ClassifiedError, so classifyError is "" and
        // the reason falls back to the stable reply_failed (Go behavior for
        // plain send errors).
        let safe_reason = safe_reply_failure_reason(None);
        let error_class = classify_error(None);
        if !outbound_record.delivery_id.is_empty() && !partial_reply {
            outbound_record.status = DeliveryStatus::Failed;
            outbound_record.error = safe_reason.clone();
            outbound_record.updated_at = Utc::now();
            let _ = self.store.upsert_connector_message(&outbound_record);
        }
        let _ = self.update_step_status(
            &run.run_id,
            &step.step_id,
            UpdateStepStatusInput {
                status: StepStatus::Failed,
                output: Some(
                    object(serde_json::json!({
                        "reply": query_result.dispatch.output,
                        "partial": partial_reply,
                        "replyStatus": outbound_record.status.as_str(),
                        "reasonCode": safe_reason,
                        "errorClass": error_class,
                    }))
                    .into(),
                ),
            },
        );
        persisted_inbound.status = if partial_reply {
            DeliveryStatus::Partial
        } else {
            DeliveryStatus::Failed
        };
        persisted_inbound.error = safe_reason.clone();
        persisted_inbound.updated_at = Utc::now();
        let _ = self.store.upsert_connector_message(persisted_inbound);
        let _ = self.record_thread_runtime_projections(
            thread,
            segment_id,
            session,
            run,
            persisted_inbound,
            &outbound_record,
            persisted_inbound.status.as_str(),
            &safe_reason,
        );
        if !partial_reply {
            let _ = self.record_channel_foreground_reply_outcome(
                connector,
                session,
                &outbound_record,
                "failed",
                &safe_reason,
                string_map(serde_json::json!({
                    "errorClass": error_class,
                    "messageId": inbound.external_message_id,
                })),
            );
            let mut payload = object(serde_json::json!({
                "messageId": inbound.external_message_id,
                "replyMessageId": outbound_record.external_message_id,
                "assistantExecutionOutcome": "succeeded",
                "connectorDeliveryOutcome": "failed",
                "connectorKind": connector.kind,
                "reasonCode": safe_reason,
                "errorClass": error_class,
                "redactionStatus": "redacted",
            }));
            if connector.kind == "discord" {
                payload.insert(
                    "discordDeliveryOutcome".to_string(),
                    Value::String("failed".to_string()),
                );
            }
            let _ = self.publish_connector_event(
                "connector.reply_failed",
                connector,
                session,
                &run.run_id,
                &step.step_id,
                payload,
            );
        }

        // The Go method returns the partially-built ProcessResult alongside the
        // send error; the Result-shaped port surfaces the error and leaves the
        // refreshed run/step observable through the runtime manager.
        Err(send_err)
    }
}

impl MessageLoop {
    /// Go blockArchivedSourceContinuation: rejects new work when the source's
    /// current thread is archived, recording the blocked linkage evidence.
    fn block_archived_source_continuation(
        &self,
        connector: &Connector,
        inbound: &InboundMessage,
        persisted_inbound: &mut MessageRecord,
    ) -> Result<bool, String> {
        let key = match source_continuation_key(connector, inbound) {
            Ok(key) => key,
            Err(_) => return Ok(false),
        };
        let current = match self.store.get_current_thread_for_source(&key)? {
            Some(thread) => thread,
            None => return Ok(false),
        };
        if current.lifecycle_state != LifecycleState::Archived {
            return Ok(false);
        }
        let now = Utc::now();
        persisted_inbound.thread_id = current.thread_id.clone();
        persisted_inbound.thread_session_segment_id = current.current_session_segment_id.clone();
        persisted_inbound.status = DeliveryStatus::Failed;
        persisted_inbound.error = "thread_archived".to_string();
        persisted_inbound.updated_at = now;
        self.store.upsert_connector_message(persisted_inbound)?;
        self.save_thread_source_linkage(&SourceLinkage {
            source_linkage_id: thread_source_linkage_id(persisted_inbound, RoutingOutcome::Blocked),
            thread_id: current.thread_id.clone(),
            tenant_id: current.tenant_id.clone(),
            source_kind: SourceKind::Channel,
            connector_id: connector.connector_id.clone(),
            connector_kind: connector.kind.clone(),
            source_account_id: key.source_account_id.clone(),
            source_conversation_id: key.source_conversation_id.clone(),
            source_message_id: inbound_provider_message_id(inbound),
            routing_outcome: RoutingOutcome::Blocked,
            current: false,
            linked_at: Some(now),
            retention_expires_at: None,
            redaction_status: RedactionStatus::Redacted,
        })?;
        Ok(true)
    }

    /// Go ensureThreadLifecycleForInbound: binds the source to its current
    /// thread (or creates one), records the session segment, the accepted
    /// source linkage, and the conversation-shape evidence.
    fn ensure_thread_lifecycle_for_inbound(
        &self,
        connector: &Connector,
        inbound: &InboundMessage,
        session: &Session,
        persisted_inbound: &MessageRecord,
    ) -> Result<(Thread, String), String> {
        let key = match source_continuation_key(connector, inbound) {
            Ok(key) => key,
            Err(_) => return Ok((zero_thread(), String::new())),
        };
        let now = Utc::now();
        let (current, segment_id) = match self.store.get_current_thread_for_source(&key)? {
            Some(mut current) => {
                let segment_id = if current.current_session_segment_id.trim().is_empty() {
                    let segment_id = format!("seg_{}", session.session_id);
                    current.current_session_segment_id = segment_id.clone();
                    segment_id
                } else {
                    current.current_session_segment_id.clone()
                };
                current.last_activity_at = now;
                current.updated_at = now;
                self.store.upsert_thread(&current)?;
                (current, segment_id)
            }
            None => {
                let segment_id = format!("seg_{}", session.session_id);
                let thread = Thread {
                    thread_id: thread_id_for_source(&key),
                    tenant_id: key.tenant_id.clone(),
                    lifecycle_state: LifecycleState::Active,
                    current_session_segment_id: segment_id.clone(),
                    source_kind: SourceKind::Channel,
                    source_summary: format!(
                        "{} / {}",
                        connector.display_name,
                        inbound_channel_or_conversation_id(inbound)
                    ),
                    last_activity_at: now,
                    created_at: now,
                    updated_at: now,
                    retention_expires_at: Some(
                        self.store
                            .thread_retention_expiry(&key.tenant_id, now)
                            .unwrap_or_else(|_| now + chrono::Duration::days(90)),
                    ),
                    redaction_status: RedactionStatus::Redacted,
                };
                self.store.upsert_thread(&thread)?;
                (thread, segment_id)
            }
        };
        self.store.upsert_thread_session_segment(&SessionSegment {
            session_segment_id: segment_id.clone(),
            thread_id: current.thread_id.clone(),
            tenant_id: current.tenant_id.clone(),
            session_id: session.session_id.clone(),
            generation: session.generation as i32,
            state: "active".to_string(),
            started_at: session.created_at,
            ended_at: None,
            last_active_at: now,
            reset_from_session_segment_id: String::new(),
            partial_evidence: false,
        })?;
        self.save_thread_source_linkage(&SourceLinkage {
            source_linkage_id: thread_source_linkage_id(
                persisted_inbound,
                RoutingOutcome::Accepted,
            ),
            thread_id: current.thread_id.clone(),
            tenant_id: current.tenant_id.clone(),
            source_kind: SourceKind::Channel,
            connector_id: connector.connector_id.clone(),
            connector_kind: connector.kind.clone(),
            source_account_id: key.source_account_id.clone(),
            source_conversation_id: key.source_conversation_id.clone(),
            source_message_id: inbound_provider_message_id(inbound),
            routing_outcome: RoutingOutcome::Accepted,
            current: true,
            linked_at: Some(now),
            retention_expires_at: None,
            redaction_status: RedactionStatus::Redacted,
        })?;
        let shape = resolve_conversation_shape(&ConversationShapeResolutionInput {
            tenant_id: current.tenant_id.clone(),
            thread_id: current.thread_id.clone(),
            session_segment_id: segment_id.clone(),
            source_kind: SourceKind::Channel,
            connector_id: connector.connector_id.clone(),
            connector_kind: connector.kind.clone(),
            source_account_id: key.source_account_id.clone(),
            source_conversation_id: key.source_conversation_id.clone(),
            source_conversation_summary: current.source_summary.clone(),
            claimed_shape: Some(conversation_shape_for_ingress_source(
                SourceKind::Channel,
                &connector.kind,
                inbound,
            )),
            now: Some(now),
        });
        self.store.save_conversation_shape_evidence(&shape)?;
        Ok((current, segment_id))
    }

    /// Go applyGroupRoomParticipationPolicy: evaluates and persists the
    /// participation decision for group/room traffic when the connector
    /// enforces the policy.
    fn apply_group_room_participation_policy(
        &self,
        connector: &Connector,
        inbound: &InboundMessage,
        thread: &Thread,
        segment_id: &str,
        persisted_inbound: &MessageRecord,
    ) -> Result<(ParticipationDecision, bool), String> {
        let shape =
            conversation_shape_for_ingress_source(SourceKind::Channel, &connector.kind, inbound);
        if shape != ConversationShape::Group && shape != ConversationShape::Room {
            return Ok((zero_participation_decision(), false));
        }
        if !connector_enforces_group_room_participation_policy(connector) {
            return Ok((zero_participation_decision(), false));
        }
        let (allowlist_eligible, permission_allowed, policy_id) =
            self.evaluate_group_room_route_policy(connector, inbound)?;
        let mut decision = evaluate_participation(&ParticipationEvaluationInput {
            shape,
            allowlist_eligible,
            qualifying_mention: inbound.mentioned,
            permission_allowed,
            duplicate: false,
            unsupported: false,
            redaction_allowed: true,
            occurred_at: Some(persisted_inbound.created_at),
            safe_summary: "Group or room message evaluated by participation policy".to_string(),
        });
        decision.tenant_id = thread.tenant_id.clone();
        decision.thread_id = thread.thread_id.clone();
        decision.session_segment_id = segment_id.to_string();
        decision.policy_id = policy_id;
        decision.connector_id = connector.connector_id.clone();
        decision.connector_kind = connector.kind.clone();
        decision.source_account_id = inbound_connector_account_id(inbound);
        decision.source_conversation_id = inbound_channel_or_conversation_id(inbound);
        decision.source_message_id = inbound_provider_message_id(inbound);
        self.store.save_participation_decision(&decision)?;
        Ok((decision, true))
    }

    /// Go evaluateGroupRoomRoutePolicy: resolves the allowlist/permission
    /// eligibility for a group/room message under the connector's route policy.
    fn evaluate_group_room_route_policy(
        &self,
        connector: &Connector,
        inbound: &InboundMessage,
    ) -> Result<(bool, bool, String), String> {
        if connector.status == Status::Disabled
            || connector.status == Status::Failed
            || connector.status == Status::BackingOff
        {
            return Ok((false, false, String::new()));
        }
        let tenant_id = coalesce_trimmed(&[&inbound.tenant_id, &connector.tenant_id]);
        let policy = match self
            .store
            .get_channel_route_policy(&tenant_id, &connector.connector_id)?
        {
            Some(policy) => policy,
            None => return Ok((false, false, String::new())),
        };
        if !route_policy_is_valid(&policy) {
            return Ok((false, false, String::new()));
        }
        let source_conversation_id = inbound_channel_or_conversation_id(inbound);
        let allowlist_eligible = route_policy_allows_conversation(&policy, &source_conversation_id);
        let permission_allowed = route_policy_allows_sender(&policy, inbound.author_id.trim());
        Ok((
            allowlist_eligible,
            permission_allowed,
            policy.route_policy_id,
        ))
    }

    /// Go recordDuplicateThreadEvidence: records the duplicate source linkage
    /// (or routing-only evidence when the source key is invalid).
    fn record_duplicate_thread_evidence(
        &self,
        connector: &Connector,
        inbound: &InboundMessage,
        persisted_inbound: &MessageRecord,
    ) -> Result<(), String> {
        let key = match source_continuation_key(connector, inbound) {
            Ok(key) => key,
            Err(_) => {
                return self.record_routing_only_source_evidence(
                    connector,
                    inbound,
                    persisted_inbound,
                    RoutingOutcome::UnknownSource,
                    "invalid_source_key",
                );
            }
        };
        let current = match self.store.get_current_thread_for_source(&key)? {
            Some(thread) => thread,
            None => return Ok(()),
        };
        self.save_thread_source_linkage(&SourceLinkage {
            source_linkage_id: thread_source_linkage_id(
                persisted_inbound,
                RoutingOutcome::Duplicate,
            ),
            thread_id: current.thread_id.clone(),
            tenant_id: current.tenant_id.clone(),
            source_kind: SourceKind::Channel,
            connector_id: connector.connector_id.clone(),
            connector_kind: connector.kind.clone(),
            source_account_id: key.source_account_id.clone(),
            source_conversation_id: key.source_conversation_id.clone(),
            source_message_id: inbound_provider_message_id(inbound),
            routing_outcome: RoutingOutcome::Duplicate,
            current: false,
            linked_at: Some(Utc::now()),
            retention_expires_at: None,
            redaction_status: RedactionStatus::Redacted,
        })?;
        Ok(())
    }

    /// Go recordRoutingOnlySourceEvidence: persists an ingress thread + source
    /// linkage for routing outcomes that never reached assistant work.
    fn record_routing_only_source_evidence(
        &self,
        connector: &Connector,
        inbound: &InboundMessage,
        persisted_inbound: &MessageRecord,
        outcome: RoutingOutcome,
        reason_code: &str,
    ) -> Result<(), String> {
        let _ = reason_code;
        let tenant_id = coalesce_trimmed(&[&inbound.tenant_id, &connector.tenant_id]);
        if tenant_id.is_empty() {
            return Ok(());
        }
        let now = Utc::now();
        let thread = Thread {
            thread_id: format!(
                "thr_ingress_{}",
                short_thread_hash(&format!(
                    "{}{}",
                    persisted_inbound.delivery_id,
                    routing_outcome_str(outcome)
                ))
            ),
            tenant_id: tenant_id.clone(),
            lifecycle_state: LifecycleState::Active,
            current_session_segment_id: String::new(),
            source_kind: SourceKind::Channel,
            source_summary: format!("{} / routing evidence", connector.display_name),
            last_activity_at: now,
            created_at: now,
            updated_at: now,
            retention_expires_at: Some(
                self.store
                    .thread_retention_expiry(&tenant_id, now)
                    .unwrap_or_else(|_| now + chrono::Duration::days(90)),
            ),
            redaction_status: RedactionStatus::Redacted,
        };
        self.store.upsert_thread(&thread)?;
        self.save_thread_source_linkage(&SourceLinkage {
            source_linkage_id: thread_source_linkage_id(persisted_inbound, outcome),
            thread_id: thread.thread_id.clone(),
            tenant_id,
            source_kind: SourceKind::Channel,
            connector_id: connector.connector_id.clone(),
            connector_kind: connector.kind.clone(),
            source_account_id: inbound_connector_account_id(inbound),
            source_conversation_id: inbound_channel_or_conversation_id(inbound),
            source_message_id: inbound_provider_message_id(inbound),
            routing_outcome: outcome,
            current: false,
            linked_at: Some(now),
            retention_expires_at: None,
            redaction_status: RedactionStatus::Redacted,
        })?;
        Ok(())
    }

    /// Go recordThreadRuntimeProjections: writes the session/run/message/reply
    /// runtime projections for the thread segment.
    fn record_thread_runtime_projections(
        &self,
        thread: &Thread,
        segment_id: &str,
        session: &Session,
        run: &Run,
        inbound_record: &MessageRecord,
        outbound_record: &MessageRecord,
        status: &str,
        reason_code: &str,
    ) -> Result<(), String> {
        if thread.thread_id.is_empty() {
            return Ok(());
        }
        let now = Utc::now();
        let mut projections = vec![
            build_runtime_projection(&RuntimeProjectionInput {
                projection_id: format!("rtp_session_{}", session.session_id),
                thread_id: thread.thread_id.clone(),
                tenant_id: thread.tenant_id.clone(),
                session_segment_id: segment_id.to_string(),
                resource_kind: RuntimeResourceKind::Session,
                resource_id: session.session_id.clone(),
                status: session_status_str(session.status).to_string(),
                reason_code: reason_code.to_string(),
                occurred_at: now,
                route: format!("/v1/sessions/{}", session.session_id),
                safe_summary: "Session routed".to_string(),
                retention_expires_at: None,
                redaction_status: None,
            }),
            build_runtime_projection(&RuntimeProjectionInput {
                projection_id: format!("rtp_connector_message_{}", inbound_record.delivery_id),
                thread_id: thread.thread_id.clone(),
                tenant_id: thread.tenant_id.clone(),
                session_segment_id: segment_id.to_string(),
                resource_kind: RuntimeResourceKind::ConnectorMessage,
                resource_id: inbound_record.delivery_id.clone(),
                status: inbound_record.status.as_str().to_string(),
                reason_code: reason_code.to_string(),
                occurred_at: inbound_record.created_at,
                route: String::new(),
                safe_summary: format!("Inbound connector message {status}"),
                retention_expires_at: None,
                redaction_status: None,
            }),
        ];
        if !run.run_id.is_empty() {
            projections.push(build_runtime_projection(&RuntimeProjectionInput {
                projection_id: format!("rtp_run_{}", run.run_id),
                thread_id: thread.thread_id.clone(),
                tenant_id: thread.tenant_id.clone(),
                session_segment_id: segment_id.to_string(),
                resource_kind: RuntimeResourceKind::Run,
                resource_id: run.run_id.clone(),
                status: run.status.as_str().to_string(),
                reason_code: reason_code.to_string(),
                occurred_at: run.created_at,
                route: format!("/v1/runs/{}", run.run_id),
                safe_summary: format!("Assistant run {}", run.status.as_str()),
                retention_expires_at: None,
                redaction_status: None,
            }));
        }
        if !outbound_record.delivery_id.is_empty() {
            projections.push(build_runtime_projection(&RuntimeProjectionInput {
                projection_id: format!("rtp_foreground_reply_{}", outbound_record.delivery_id),
                thread_id: thread.thread_id.clone(),
                tenant_id: thread.tenant_id.clone(),
                session_segment_id: segment_id.to_string(),
                resource_kind: RuntimeResourceKind::ForegroundReply,
                resource_id: outbound_record.delivery_id.clone(),
                status: outbound_record.status.as_str().to_string(),
                reason_code: reason_code.to_string(),
                occurred_at: outbound_record.created_at,
                route: String::new(),
                safe_summary: format!("Foreground reply {}", outbound_record.status.as_str()),
                retention_expires_at: None,
                redaction_status: None,
            }));
        }
        for projection in projections {
            self.save_thread_runtime_projection(&projection)?;
        }
        Ok(())
    }

    /// Go saveThreadSourceLinkage: persists the linkage and publishes the
    /// thread.source_linked event.
    fn save_thread_source_linkage(&self, linkage: &SourceLinkage) -> Result<(), String> {
        self.store.save_thread_source_linkage(linkage)?;
        if let Some(bus) = &self.event_bus {
            bus.publish(thread_source_linked_event(linkage));
        }
        Ok(())
    }

    /// Go saveThreadRuntimeProjection: persists the projection and publishes
    /// the thread.runtime_projection_recorded event.
    fn save_thread_runtime_projection(&self, projection: &RuntimeProjection) -> Result<(), String> {
        self.store.save_thread_runtime_projection(projection)?;
        if let Some(bus) = &self.event_bus {
            bus.publish(thread_runtime_projection_event(projection));
        }
        Ok(())
    }
}

/// Go events.ThreadSourceLinkedEvent.
#[must_use]
pub fn thread_source_linked_event(link: &SourceLinkage) -> Event {
    let occurred_at = link.linked_at.unwrap_or_else(Utc::now);
    Event {
        tenant_id: link.tenant_id.clone(),
        category: "thread".to_string(),
        name: "thread.source_linked".to_string(),
        occurred_at,
        scope: Scope::default(),
        resource: Resource {
            kind: "thread_source_linkage".to_string(),
            id: link.source_linkage_id.clone(),
        },
        payload: object(serde_json::json!({
            "tenantId": link.tenant_id,
            "threadId": link.thread_id,
            "sourceLinkageId": link.source_linkage_id,
            "routingOutcome": routing_outcome_str(link.routing_outcome),
            "redactionStatus": redaction_status_str(link.redaction_status),
        })),
        ..Event::default()
    }
}

/// Go events.ThreadRuntimeProjectionEvent.
#[must_use]
pub fn thread_runtime_projection_event(projection: &RuntimeProjection) -> Event {
    Event {
        tenant_id: projection.tenant_id.clone(),
        category: "thread".to_string(),
        name: "thread.runtime_projection_recorded".to_string(),
        occurred_at: projection.occurred_at,
        scope: Scope {
            session_id: projection.session_segment_id.clone(),
            ..Scope::default()
        },
        resource: Resource {
            kind: "thread_runtime_projection".to_string(),
            id: projection.runtime_projection_id.clone(),
        },
        payload: object(serde_json::json!({
            "tenantId": projection.tenant_id,
            "threadId": projection.thread_id,
            "sessionSegmentId": projection.session_segment_id,
            "runtimeProjectionId": projection.runtime_projection_id,
            "resourceKind": runtime_resource_str(projection.resource_kind),
            "resourceId": projection.resource_id,
            "status": projection.status,
            "redactionStatus": redaction_status_str(projection.redaction_status),
        })),
        ..Event::default()
    }
}

/// Zero-valued session for the duplicate/blocked return paths (Go
/// router.Session{}).
fn empty_session() -> Session {
    let now = Utc::now();
    Session {
        session_id: String::new(),
        kind: SessionKind::Direct,
        status: SessionStatus::Active,
        channel: String::new(),
        account_id: String::new(),
        peer_id: String::new(),
        thread_id: String::new(),
        routing_key: String::new(),
        generation: 0,
        created_at: now,
        updated_at: now,
        last_active_at: now,
        last_reset_at: None,
        active_profile_projection: None,
    }
}

/// Zero-valued thread (Go threads.Thread{}).
fn zero_thread() -> Thread {
    let now = Utc::now();
    Thread {
        thread_id: String::new(),
        tenant_id: String::new(),
        lifecycle_state: LifecycleState::Active,
        current_session_segment_id: String::new(),
        source_kind: SourceKind::Channel,
        source_summary: String::new(),
        last_activity_at: now,
        created_at: now,
        updated_at: now,
        retention_expires_at: None,
        redaction_status: RedactionStatus::Redacted,
    }
}

/// Zero-valued participation decision (Go threads.ParticipationDecision{}); the
/// content is never read when the enforced flag is false.
fn zero_participation_decision() -> ParticipationDecision {
    let now = Utc::now();
    ParticipationDecision {
        participation_decision_id: String::new(),
        tenant_id: String::new(),
        thread_id: String::new(),
        session_segment_id: String::new(),
        connector_id: String::new(),
        connector_kind: String::new(),
        source_account_id: String::new(),
        source_conversation_id: String::new(),
        source_message_id: String::new(),
        conversation_shape: ConversationShape::Unknown,
        policy_id: String::new(),
        mention_status: kura_threads::MentionStatus::Missing,
        allowlist_status: kura_threads::AllowlistStatus::NotAllowlisted,
        decision: ParticipationDecisionValue::Accepted,
        reason_code: String::new(),
        created_assistant_work: false,
        occurred_at: Some(now),
        retention_expires_at: None,
        redaction_status: RedactionStatus::Redacted,
        safe_summary: String::new(),
    }
}

/// Go string(threads.RuntimeResourceKind).
#[must_use]
pub fn runtime_resource_str(kind: RuntimeResourceKind) -> &'static str {
    match kind {
        RuntimeResourceKind::Session => "session",
        RuntimeResourceKind::Run => "run",
        RuntimeResourceKind::Workflow => "workflow",
        RuntimeResourceKind::Approval => "approval",
        RuntimeResourceKind::ForegroundReply => "foreground_reply",
        RuntimeResourceKind::BackgroundDelivery => "background_delivery",
        RuntimeResourceKind::ConnectorMessage => "connector_message",
    }
}

impl MessageLoop {
    /// Go executeReplyPath: chooses the streaming path when the sender supports
    /// progression, else the final-reply path.
    fn execute_reply_path(
        &self,
        connector: &Connector,
        session: &Session,
        run: &Run,
        step: &Step,
        inbound: &InboundMessage,
        persisted_inbound: &MessageRecord,
        replies: &dyn ReplySender,
        progressor: Option<&dyn ReplyProgressor>,
        capabilities: &ReplyCapabilities,
        scope: Scope,
        stop_thinking: &ThinkingStop,
        cancel: &CancellationToken,
    ) -> Result<(QueryResult, MessageRecord, Option<String>), String> {
        if capabilities.supports_streaming && progressor.is_some() {
            return self.execute_streaming_reply(
                connector,
                session,
                run,
                step,
                inbound,
                persisted_inbound,
                progressor.expect("checked above"),
                capabilities,
                scope,
                stop_thinking,
                cancel,
            );
        }
        self.execute_final_reply(
            connector,
            session,
            run,
            step,
            inbound,
            persisted_inbound,
            replies,
            capabilities,
            scope,
            stop_thinking,
            cancel,
        )
    }

    /// Go executeFinalReply: runs the non-streaming query and delivers the
    /// reply (splitting long replies into parts).
    fn execute_final_reply(
        &self,
        connector: &Connector,
        session: &Session,
        run: &Run,
        step: &Step,
        inbound: &InboundMessage,
        persisted_inbound: &MessageRecord,
        replies: &dyn ReplySender,
        capabilities: &ReplyCapabilities,
        scope: Scope,
        stop_thinking: &ThinkingStop,
        cancel: &CancellationToken,
    ) -> Result<(QueryResult, MessageRecord, Option<String>), String> {
        let execution = self
            .chat
            .query(
                QueryInput {
                    query: inbound.content.clone(),
                    tenant_id: persisted_inbound.tenant_id.clone(),
                    thread_id: persisted_inbound.thread_id.clone(),
                    scope,
                    source_kind: Some(SourceKind::Channel),
                    source_linkage_id: thread_source_linkage_id(
                        persisted_inbound,
                        RoutingOutcome::Accepted,
                    ),
                    source_message_id: inbound_provider_message_id(inbound),
                    source_timestamp: Some(persisted_inbound.created_at),
                    source_event_key: format!("connector:{}", persisted_inbound.delivery_id),
                    channel_scope_ref: binding_channel_scope_ref(persisted_inbound),
                    account_scope_ref: persisted_inbound.connector_account_id.trim().to_string(),
                    run_id: run.run_id.clone(),
                    ..QueryInput::default()
                },
                cancel,
            )
            .map_err(|e| e.to_string())?;
        let query_result = execution.result;
        if let Some(query_err) = execution.exec_error.map(chat_exec_error_text) {
            return Ok((query_result, MessageRecord::default(), Some(query_err)));
        }

        self.update_step_status(
            &run.run_id,
            &step.step_id,
            UpdateStepStatusInput {
                status: StepStatus::ExecutingTool,
                output: None,
            },
        )?;

        let mut reply_parts = split_reply_content(
            &query_result.dispatch.output,
            capabilities.max_message_length,
        );
        if reply_parts.is_empty() {
            reply_parts = vec![query_result.dispatch.output.clone()];
        }

        let mut outbound_record = MessageRecord::default();
        let mut reply_message_ids: Vec<String> = Vec::new();
        for (index, reply_part) in reply_parts.iter().enumerate() {
            let mut record = new_outbound_record(
                connector,
                session,
                run,
                inbound,
                persisted_inbound,
                reply_part,
            );
            self.store.upsert_connector_message(&record)?;

            let sent_reply = replies.send_reply(OutboundReply {
                connector_id: connector.connector_id.clone(),
                channel_id: inbound.channel_id.clone(),
                content: reply_part.clone(),
                reply_to_external_message_id: reply_to_external_message_id(inbound),
            });
            let sent = match sent_reply {
                Ok(sent) => sent,
                Err(send_err) => return Ok((query_result, record, Some(send_err))),
            };

            stop_thinking.stop();
            record.external_message_id = sent.external_message_id.clone();
            record.status = DeliveryStatus::Replied;
            record.foreground_outcome_status =
                foreground_reply_outcome_status(record.status).to_string();
            record.updated_at = Utc::now();
            self.store.upsert_connector_message(&record)?;
            if index == 0 {
                outbound_record = record.clone();
            }
            reply_message_ids.push(sent.external_message_id);
        }

        let _ = self.publish_connector_event(
            "connector.reply_sent",
            connector,
            session,
            &run.run_id,
            &step.step_id,
            object(serde_json::json!({
                "messageId": inbound.external_message_id,
                "replyMessageId": outbound_record.external_message_id,
                "replyMessageIds": reply_message_ids,
                "partCount": reply_message_ids.len(),
            })),
        );
        let _ = self.record_channel_foreground_reply_outcome(
            connector,
            session,
            &outbound_record,
            "sent",
            "reply_sent",
            string_map(serde_json::json!({
                "messageId": inbound.external_message_id,
                "replyMessageId": outbound_record.external_message_id,
                "replyMessageIds": reply_message_ids.join(","),
            })),
        );
        Ok((query_result, outbound_record, None))
    }

    /// Go executeStreamingReply: runs the streaming query, accumulating chunks
    /// through the reply progress and flushing parts to the connector.
    fn execute_streaming_reply(
        &self,
        connector: &Connector,
        session: &Session,
        run: &Run,
        step: &Step,
        inbound: &InboundMessage,
        persisted_inbound: &MessageRecord,
        progressor: &dyn ReplyProgressor,
        capabilities: &ReplyCapabilities,
        scope: Scope,
        stop_thinking: &ThinkingStop,
        cancel: &CancellationToken,
    ) -> Result<(QueryResult, MessageRecord, Option<String>), String> {
        let mut progress = stream_reply_progress {
            progressor,
            connector,
            session,
            run_id: run.run_id.clone(),
            inbound,
            response_to: persisted_inbound.clone(),
            record: MessageRecord::default(),
            records: Vec::new(),
            last_flushed: String::new(),
            last_flush_at: DateTime::<Utc>::MIN_UTC,
            flush_interval: Duration::from_millis(500),
            max_reply_length: capabilities.max_message_length,
            stop_thinking: Some(stop_thinking.clone()),
            partial_err: None,
            err: None,
            pending: Vec::new(),
        };

        let mut progress_err: Option<String> = None;
        let execution = self
            .chat
            .stream(
                QueryInput {
                    query: inbound.content.clone(),
                    tenant_id: persisted_inbound.tenant_id.clone(),
                    thread_id: persisted_inbound.thread_id.clone(),
                    scope,
                    source_kind: Some(SourceKind::Channel),
                    source_linkage_id: thread_source_linkage_id(
                        persisted_inbound,
                        RoutingOutcome::Accepted,
                    ),
                    source_message_id: inbound_provider_message_id(inbound),
                    source_timestamp: Some(persisted_inbound.created_at),
                    source_event_key: format!("connector:{}", persisted_inbound.delivery_id),
                    channel_scope_ref: binding_channel_scope_ref(persisted_inbound),
                    account_scope_ref: persisted_inbound.connector_account_id.trim().to_string(),
                    run_id: run.run_id.clone(),
                    ..QueryInput::default()
                },
                cancel,
                Some(|chunk: StreamChunk| -> Result<(), ChatError> {
                    if progress_err.is_some() {
                        return Ok(());
                    }
                    if let Err(err) = progress.on_chunk(&chunk.reply) {
                        progress_err = Some(err);
                    }
                    Ok(())
                }),
            )
            .map_err(|e| e.to_string())?;
        let query_result = execution.result;
        if let Some(query_err) = execution.exec_error.map(chat_exec_error_text) {
            if query_result.dispatch.partial && !query_result.dispatch.output.trim().is_empty() {
                if let Err(err) =
                    progress.complete_partial(&query_result.dispatch.output, &query_err)
                {
                    self.apply_deferred(
                        connector,
                        session,
                        &run.run_id,
                        &step.step_id,
                        progress.pending,
                    )?;
                    return Ok((query_result, progress.record.clone(), Some(err)));
                }
            }
            self.apply_deferred(
                connector,
                session,
                &run.run_id,
                &step.step_id,
                progress.pending,
            )?;
            return Ok((query_result, progress.record.clone(), Some(query_err)));
        }

        self.update_step_status(
            &run.run_id,
            &step.step_id,
            UpdateStepStatusInput {
                status: StepStatus::ExecutingTool,
                output: None,
            },
        )?;
        if let Err(err) = progress.complete(&query_result.dispatch.output) {
            self.apply_deferred(
                connector,
                session,
                &run.run_id,
                &step.step_id,
                progress.pending,
            )?;
            return Ok((query_result, progress.record.clone(), Some(err)));
        }
        if let Some(progress_err) = progress_err {
            self.apply_deferred(
                connector,
                session,
                &run.run_id,
                &step.step_id,
                progress.pending,
            )?;
            return Ok((query_result, progress.record.clone(), Some(progress_err)));
        }
        self.apply_deferred(
            connector,
            session,
            &run.run_id,
            &step.step_id,
            progress.pending,
        )?;
        Ok((query_result, progress.record.clone(), None))
    }
}

/// Go newOutboundRecord: builds the outbound delivery record for one reply
/// part. Free function so the streaming flush (which must be Send) can
/// build records without holding the loop.
pub fn new_outbound_record(
    connector: &Connector,
    session: &Session,
    run: &Run,
    inbound: &InboundMessage,
    persisted_inbound: &MessageRecord,
    content: &str,
) -> MessageRecord {
    let now = Utc::now();
    MessageRecord {
        delivery_id: new_delivery_id(),
        tenant_id: persisted_inbound.tenant_id.clone(),
        connector_id: connector.connector_id.clone(),
        direction: DeliveryDirection::Outbound,
        session_id: session.session_id.clone(),
        run_id: run.run_id.clone(),
        channel_id: inbound.channel_id.clone(),
        peer_id: inbound.peer_id.clone(),
        thread_id: persisted_inbound.thread_id.clone(),
        thread_session_segment_id: persisted_inbound.thread_session_segment_id.clone(),
        content: content.to_string(),
        status: DeliveryStatus::Processing,
        foreground_outcome_status: foreground_reply_outcome_status(DeliveryStatus::Processing)
            .to_string(),
        response_to_delivery_id: persisted_inbound.delivery_id.clone(),
        reply_to_external_message_id: reply_to_external_message_id(inbound),
        created_at: now,
        updated_at: now,
        ..MessageRecord::default()
    }
}

impl MessageLoop {
    /// Go startThinkingProgress: sends the initial thinking indicator and
    /// returns whether the periodic keep-alive ticker should run.
    fn start_thinking_progress(
        &self,
        connector: &Connector,
        session: &Session,
        run_id: &str,
        step_id: &str,
        inbound: &InboundMessage,
        progressor: Option<&dyn ReplyProgressor>,
        capabilities: &ReplyCapabilities,
        stop_flag: &Arc<AtomicBool>,
    ) -> bool {
        let _ = stop_flag;
        let Some(progressor) = progressor else {
            return false;
        };
        if !capabilities.supports_thinking {
            return false;
        }
        let signal = ThinkingSignal {
            connector_id: connector.connector_id.clone(),
            channel_id: inbound.channel_id.clone(),
        };
        match progressor.send_thinking(signal) {
            Ok(()) => {
                let _ = self.publish_connector_event(
                    "connector.thinking_started",
                    connector,
                    session,
                    run_id,
                    step_id,
                    object(serde_json::json!({
                        "messageId": inbound.external_message_id,
                        "channelId": inbound.channel_id,
                    })),
                );
                true
            }
            Err(err) => {
                let _ = self.publish_connector_event(
                    "connector.thinking_failed",
                    connector,
                    session,
                    run_id,
                    step_id,
                    object(serde_json::json!({
                        "messageId": inbound.external_message_id,
                        "channelId": inbound.channel_id,
                        "error": err,
                        "errorClass": classify_error(None),
                    })),
                );
                false
            }
        }
    }

    /// Go routeSession.
    fn route_session(
        &self,
        inbound: &InboundMessage,
    ) -> Result<(Session, bool), kura_router::RouterError> {
        self.router.route(RouteInput {
            kind: inbound.kind,
            channel: inbound.connector_kind.clone(),
            account_id: inbound.account_id.clone(),
            peer_id: inbound.peer_id.clone(),
            thread_id: inbound.thread_id.clone(),
        })
    }

    /// Go createRunAndStep: creates and persists the run + step, writing
    /// checkpoints and publishing the run.created/step.created events.
    fn create_run_and_step(
        &self,
        connector: &Connector,
        session: &Session,
        inbound: &InboundMessage,
    ) -> Result<(Run, Step), String> {
        let run = self
            .runtime
            .create_run(CreateRunInput {
                session_id: session.session_id.clone(),
                entrypoint: format!("{}.message", connector.kind),
                goal: inbound.content.clone(),
                ..CreateRunInput::default()
            })
            .map_err(|e| e.to_string())?;
        self.store.upsert_run(&run)?;
        self.persist_checkpoint(&run.run_id)?;
        self.publish_runtime_event(
            "run.created",
            "run",
            &run.run_id,
            Scope {
                session_id: session.session_id.clone(),
                run_id: run.run_id.clone(),
                connector_id: connector.connector_id.clone(),
                ..Scope::default()
            },
            object(serde_json::json!({
                "entrypoint": run.entrypoint,
                "goal": run.goal,
                "status": run.status.as_str(),
                "source": connector_source(&connector.kind),
                "messageId": inbound.external_message_id,
            })),
        )?;

        let step = self
            .runtime
            .create_step(
                &run.run_id,
                CreateStepInput {
                    title: "reply to connector message".to_string(),
                    kind: "chat_query".to_string(),
                    input: Some(
                        object(serde_json::json!({
                            "messageId": inbound.external_message_id,
                            "content": inbound.content,
                        }))
                        .into(),
                    ),
                    ..CreateStepInput::default()
                },
            )
            .map_err(|e| e.to_string())?;
        self.store.upsert_step(&step)?;
        self.persist_checkpoint(&run.run_id)?;
        self.publish_runtime_event(
            "step.created",
            "step",
            &step.step_id,
            Scope {
                session_id: session.session_id.clone(),
                run_id: run.run_id.clone(),
                step_id: step.step_id.clone(),
                connector_id: connector.connector_id.clone(),
                ..Scope::default()
            },
            object(serde_json::json!({
                "title": step.title,
                "kind": step.kind,
                "status": step.status.as_str(),
                "messageId": inbound.external_message_id,
            })),
        )?;
        Ok((run, step))
    }

    /// Go updateStepStatus: transitions the step, persists step/run changes,
    /// checkpoints, and publishes the step/run status events.
    fn update_step_status(
        &self,
        run_id: &str,
        step_id: &str,
        input: UpdateStepStatusInput,
    ) -> Result<Step, String> {
        let (step, run_update) = self
            .runtime
            .update_step_status_and_reconcile_run(run_id, step_id, input)
            .map_err(|e| e.to_string())?;
        self.store.upsert_step(&step)?;
        if let Some(run_update) = &run_update {
            self.store.upsert_run(run_update)?;
        }
        self.persist_checkpoint(run_id)?;

        let run = self.runtime.get_run(run_id);
        let run_session_id = run
            .as_ref()
            .map(|run| run.session_id.clone())
            .unwrap_or_default();
        self.publish_runtime_event(
            "step.status_changed",
            "step",
            step_id,
            Scope {
                session_id: run_session_id.clone(),
                run_id: run_id.to_string(),
                step_id: step_id.to_string(),
                ..Scope::default()
            },
            object(serde_json::json!({ "status": step.status.as_str() })),
        )?;
        if let Some(run_update) = run_update {
            self.publish_runtime_event(
                "run.status_changed",
                "run",
                run_id,
                Scope {
                    session_id: run_session_id,
                    run_id: run_id.to_string(),
                    ..Scope::default()
                },
                object(serde_json::json!({ "status": run_update.status.as_str() })),
            )?;
        }
        Ok(step)
    }

    /// Go persistCheckpoint.
    fn persist_checkpoint(&self, run_id: &str) -> Result<(), String> {
        if let Some(checkpoints) = &self.checkpoints {
            checkpoints.save_run_checkpoint(run_id)?;
        }
        Ok(())
    }

    /// Go persistSession.
    fn persist_session(&self, session: &Session) -> Result<(), String> {
        self.store.upsert_session(session)?;
        Ok(())
    }

    /// Go publishSessionRouteEvents.
    fn publish_session_route_events(
        &self,
        connector: &Connector,
        session: &Session,
        created: bool,
        inbound: &InboundMessage,
    ) -> Result<(), String> {
        if created {
            self.publish_runtime_event(
                "session.created",
                "session",
                &session.session_id,
                Scope {
                    session_id: session.session_id.clone(),
                    ..Scope::default()
                },
                object(serde_json::json!({
                    "kind": session.kind.as_str(),
                    "channel": session.channel,
                    "routingKey": session.routing_key,
                    "generation": session.generation,
                    "source": connector_source(&connector.kind),
                    "connectorId": connector.connector_id,
                    "messageId": inbound.external_message_id,
                })),
            )?;
        }
        self.publish_runtime_event(
            "session.routed",
            "session",
            &session.session_id,
            Scope {
                session_id: session.session_id.clone(),
                ..Scope::default()
            },
            object(serde_json::json!({
                "kind": session.kind.as_str(),
                "channel": session.channel,
                "routingKey": session.routing_key,
                "generation": session.generation,
                "source": connector_source(&connector.kind),
                "connectorId": connector.connector_id,
                "messageId": inbound.external_message_id,
            })),
        )?;
        Ok(())
    }

    /// Go publishMatrixRouteOutcome: matrix-only route outcome evidence.
    fn publish_matrix_route_outcome(
        &self,
        connector: &Connector,
        session: &Session,
        record: &MessageRecord,
        outcome: &str,
        reason_code: &str,
    ) -> Result<(), String> {
        if connector.kind != "matrix" {
            return Ok(());
        }
        self.publish_connector_event(
            "connector.route_outcome_recorded",
            connector,
            session,
            "",
            "",
            object(serde_json::json!({
                "tenantId": record.tenant_id,
                "connectorId": connector.connector_id,
                "homeserverId": record.connector_account_id,
                "conversationId": record.channel_or_conversation_id,
                "matrixEventId": record.provider_message_id,
                "outcome": outcome,
                "reasonCode": reason_code,
                "surface": matrix_route_surface(record),
                "messageDeliveryId": record.delivery_id,
                "connectorAccountId": record.connector_account_id,
                "channelOrConversationId": record.channel_or_conversation_id,
                "providerMessageId": record.provider_message_id,
                "equivalentRuleId": record.equivalent_rule_id,
                "redactionStatus": "redacted",
            })),
        )?;
        Ok(())
    }

    /// Go publishConnectorEvent.
    fn publish_connector_event(
        &self,
        name: &str,
        connector: &Connector,
        session: &Session,
        run_id: &str,
        step_id: &str,
        payload: Map<String, Value>,
    ) -> Result<Event, String> {
        let scope = Scope {
            session_id: session.session_id.clone(),
            run_id: run_id.to_string(),
            step_id: step_id.to_string(),
            connector_id: connector.connector_id.clone(),
            ..Scope::default()
        };
        self.publish_event(
            "connector",
            name,
            "connector",
            &connector.connector_id,
            scope,
            payload,
        )
    }

    /// Go recordChannelForegroundReplyOutcome.
    fn record_channel_foreground_reply_outcome(
        &self,
        connector: &Connector,
        _session: &Session,
        record: &MessageRecord,
        status: &str,
        reason_code: &str,
        safe_evidence: HashMap<String, String>,
    ) -> Result<(), String> {
        let tenant_id = record.tenant_id.trim().to_string();
        let tenant_id = if tenant_id.is_empty() {
            connector.tenant_id.trim().to_string()
        } else {
            tenant_id
        };
        if tenant_id.is_empty() {
            return Ok(());
        }
        let outcome_id = if record.delivery_id.trim().is_empty() {
            new_delivery_id()
        } else {
            record.delivery_id.clone()
        };
        let now = Utc::now();
        self.store
            .save_channel_foreground_reply_outcome(&ForegroundReplyOutcome {
                reply_outcome_id: outcome_id,
                tenant_id,
                connector_id: connector.connector_id.clone(),
                routing_decision_id: String::new(),
                status: status.to_string(),
                reason_code: reason_code.to_string(),
                occurred_at: now,
                safe_evidence,
                redaction_status: ConnectorRedactionStatus::Redacted,
                retention_expires_at: now + chrono::Duration::days(90),
            })?;
        Ok(())
    }

    /// Go publishRuntimeEvent.
    fn publish_runtime_event(
        &self,
        name: &str,
        resource_kind: &str,
        resource_id: &str,
        scope: Scope,
        payload: Map<String, Value>,
    ) -> Result<Event, String> {
        let category = match resource_kind {
            "session" => "session",
            "step" => "step",
            _ => "run",
        };
        self.publish_event(category, name, resource_kind, resource_id, scope, payload)
    }

    /// Go publishEvent: normalizes the envelope, persists through the store,
    /// and publishes on the bus.
    fn publish_event(
        &self,
        category: &str,
        name: &str,
        resource_kind: &str,
        resource_id: &str,
        scope: Scope,
        payload: Map<String, Value>,
    ) -> Result<Event, String> {
        let Some(bus) = &self.event_bus else {
            return Ok(Event::default());
        };
        let mut event = Event {
            category: category.to_string(),
            name: name.to_string(),
            scope,
            resource: Resource {
                kind: resource_kind.to_string(),
                id: resource_id.to_string(),
            },
            payload,
            ..Event::default()
        };
        if event.event_id.is_empty() {
            event.event_id = new_event_id();
        }
        if event.occurred_at == DateTime::<Utc>::MIN_UTC {
            event.occurred_at = Utc::now();
        }
        let event = self.store.append_event(&event)?;
        Ok(bus.publish(event))
    }

    /// Applies deferred streaming side effects: upserts outbound records,
    /// publishes connector events, and saves foreground reply outcomes.
    fn apply_deferred(
        &self,
        connector: &Connector,
        session: &Session,
        run_id: &str,
        step_id: &str,
        pending: Vec<DeferredOp>,
    ) -> Result<(), String> {
        for op in pending {
            match op {
                DeferredOp::UpsertRecord(record) => {
                    self.store.upsert_connector_message(&record)?;
                }
                DeferredOp::ConnectorEvent { name, payload } => {
                    let _ = self.publish_connector_event(
                        &name, connector, session, run_id, step_id, payload,
                    );
                }
                DeferredOp::ReplyOutcome {
                    record,
                    status,
                    reason,
                    evidence,
                } => {
                    let _ = self.record_channel_foreground_reply_outcome(
                        connector, session, &record, &status, &reason, evidence,
                    );
                }
            }
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Thinking keep-alive ticker (Go startThinkingProgress goroutine)
// ---------------------------------------------------------------------------

/// Stop handle for the periodic thinking ticker. Go's context cancel func.
#[derive(Clone)]
pub struct ThinkingStop {
    flag: Arc<AtomicBool>,
}

impl ThinkingStop {
    /// Cancels the ticker (idempotent).
    pub fn stop(&self) {
        self.flag.store(true, Ordering::SeqCst);
    }
}

/// The periodic thinking keep-alive loop: sends a thinking signal every 4
/// seconds until stopped, publishing connector.thinking_failed on error. The
/// scoped thread exits within one tick after the stop flag is set.
fn tick_thinking(
    bus: Option<Bus>,
    connector_id: String,
    session_id: String,
    run_id: String,
    step_id: String,
    message_id: String,
    channel_id: String,
    progressor: &dyn ReplyProgressor,
    signal: ThinkingSignal,
    flag: Arc<AtomicBool>,
) {
    let tick = Duration::from_millis(100);
    let interval = Duration::from_secs(4);
    let mut elapsed = Duration::ZERO;
    loop {
        if flag.load(Ordering::SeqCst) {
            return;
        }
        std::thread::sleep(tick);
        elapsed += tick;
        if elapsed >= interval {
            elapsed = Duration::ZERO;
            match progressor.send_thinking(signal.clone()) {
                Ok(()) => {}
                Err(err) => {
                    if let Some(bus) = &bus {
                        // Published on the bus only: the ticker cannot reach the loop
                        // store handle (SQLite is not Sync), so this periodic-failure
                        // evidence is not appended to the event ledger.
                        let event = Event {
                            category: "connector".to_string(),
                            name: "connector.thinking_failed".to_string(),
                            scope: Scope {
                                session_id: session_id.clone(),
                                run_id: run_id.clone(),
                                step_id: step_id.clone(),
                                connector_id: connector_id.clone(),
                                ..Scope::default()
                            },
                            resource: Resource {
                                kind: "connector".to_string(),
                                id: connector_id.clone(),
                            },
                            payload: object(serde_json::json!({
                                "messageId": message_id,
                                "channelId": channel_id,
                                "error": err,
                                "errorClass": classify_error(None),
                            })),
                            ..Event::default()
                        };
                        bus.publish(event);
                    }
                    return;
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Streaming reply progress (Go streamReplyProgress)
// ---------------------------------------------------------------------------

/// Go streamReplyMode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum stream_reply_mode {
    Progress,
    Complete,
    Partial,
}

/// Go streamReplyProgress: accumulates streamed reply chunks, flushes parts to
/// the connector transport (send/edit), and publishes stream lifecycle events.
struct stream_reply_progress<'a> {
    progressor: &'a dyn ReplyProgressor,
    connector: &'a Connector,
    session: &'a Session,
    run_id: String,
    inbound: &'a InboundMessage,
    response_to: MessageRecord,
    record: MessageRecord,
    records: Vec<MessageRecord>,
    last_flushed: String,
    last_flush_at: DateTime<Utc>,
    flush_interval: Duration,
    max_reply_length: i64,
    stop_thinking: Option<ThinkingStop>,
    partial_err: Option<String>,
    err: Option<String>,
    /// Store/bus side effects deferred until the stream returns (the
    /// streaming emitter must be Send and the SQLite store is not Sync).
    pending: Vec<DeferredOp>,
}

/// Deferred store/bus side effects produced by the streaming flush; applied by
/// the loop after the stream returns because the stream emitter must be Send.
enum DeferredOp {
    UpsertRecord(MessageRecord),
    ConnectorEvent {
        name: String,
        payload: Map<String, Value>,
    },
    ReplyOutcome {
        record: MessageRecord,
        status: String,
        reason: String,
        evidence: HashMap<String, String>,
    },
}

impl<'a> stream_reply_progress<'a> {
    /// Go OnChunk.
    fn on_chunk(&mut self, reply: &str) -> Result<(), String> {
        if self.err.is_some() {
            return Ok(());
        }
        self.flush(reply, stream_reply_mode::Progress)
    }

    /// Go Complete.
    fn complete(&mut self, reply: &str) -> Result<(), String> {
        if let Some(err) = &self.err {
            return Err(err.clone());
        }
        self.flush(reply, stream_reply_mode::Complete)
    }

    /// Go CompletePartial.
    fn complete_partial(&mut self, reply: &str, cause: &str) -> Result<(), String> {
        if let Some(err) = &self.err {
            return Err(err.clone());
        }
        self.partial_err = Some(cause.to_string());
        self.flush(&append_partial_marker(reply), stream_reply_mode::Partial)
    }
}

impl<'a> stream_reply_progress<'a> {
    /// Go flush: trims/dedupes the accumulated reply, splits it into channel-
    /// sized parts, sends new parts, edits existing ones, and publishes the
    /// stream lifecycle events.
    fn flush(&mut self, reply: &str, mode: stream_reply_mode) -> Result<(), String> {
        let reply = reply.trim().to_string();
        if reply.is_empty() {
            return Ok(());
        }
        let force = mode != stream_reply_mode::Progress;
        if reply == self.last_flushed && !force {
            return Ok(());
        }
        if !force
            && self.last_flush_at != DateTime::<Utc>::MIN_UTC
            && Utc::now().signed_duration_since(self.last_flush_at)
                < chrono::Duration::from_std(self.flush_interval).unwrap_or_default()
        {
            return Ok(());
        }

        let now = Utc::now();
        let reply_parts = split_reply_content(&reply, self.max_reply_length);
        let started_now = self.records.is_empty();
        let mut reply_message_ids: Vec<String> = Vec::new();
        for (index, reply_part) in reply_parts.iter().enumerate() {
            if index >= self.records.len() {
                let mut record = new_outbound_record(
                    self.connector,
                    self.session,
                    &Run {
                        run_id: self.run_id.clone(),
                        ..Run::default()
                    },
                    self.inbound,
                    &self.response_to,
                    reply_part,
                );
                record.status = DeliveryStatus::Streaming;

                let sent_reply = self.progressor.send_reply(OutboundReply {
                    connector_id: self.connector.connector_id.clone(),
                    channel_id: self.inbound.channel_id.clone(),
                    content: reply_part.clone(),
                    reply_to_external_message_id: reply_to_external_message_id(self.inbound),
                });
                let sent = match sent_reply {
                    Ok(sent) => sent,
                    Err(err) => {
                        self.err = Some(err.clone());
                        return Err(err);
                    }
                };
                if let Some(stop) = self.stop_thinking.take() {
                    stop.stop();
                }
                record.external_message_id = sent.external_message_id.clone();
                record.content = reply_part.clone();
                record.updated_at = now;
                if mode == stream_reply_mode::Complete {
                    record.status = DeliveryStatus::Replied;
                } else if mode == stream_reply_mode::Partial {
                    record.status = DeliveryStatus::Partial;
                    record.error = partial_delivery_error(self.partial_err.as_deref());
                }
                self.pending.push(DeferredOp::UpsertRecord(record.clone()));
                self.records.push(record.clone());
                if index == 0 {
                    self.record = record.clone();
                }
                reply_message_ids.push(record.external_message_id.clone());
                continue;
            }

            let mut record = self.records[index].clone();
            reply_message_ids.push(record.external_message_id.clone());
            if reply_part != &record.content {
                if let Err(err) = self.progressor.edit_reply(ReplyEdit {
                    connector_id: self.connector.connector_id.clone(),
                    channel_id: self.inbound.channel_id.clone(),
                    external_message_id: record.external_message_id.clone(),
                    content: reply_part.clone(),
                }) {
                    self.err = Some(err.clone());
                    return Err(err);
                }
                record.content = reply_part.clone();
                record.updated_at = now;
            }
            record.status = DeliveryStatus::Streaming;
            if mode == stream_reply_mode::Complete {
                record.status = DeliveryStatus::Replied;
                record.error = String::new();
            } else if mode == stream_reply_mode::Partial {
                record.status = DeliveryStatus::Partial;
                record.error = partial_delivery_error(self.partial_err.as_deref());
            }
            self.pending.push(DeferredOp::UpsertRecord(record.clone()));
            self.records[index] = record.clone();
            if index == 0 {
                self.record = record;
            }
        }

        let event_name = if started_now {
            "connector.reply_stream_started"
        } else if mode == stream_reply_mode::Complete {
            "connector.reply_sent"
        } else if mode == stream_reply_mode::Partial {
            "connector.reply_partial"
        } else {
            "connector.reply_stream_updated"
        };
        let mut payload = object(serde_json::json!({
            "messageId": self.inbound.external_message_id,
            "replyMessageId": self.record.external_message_id,
            "replyMessageIds": reply_message_ids,
            "partCount": reply_message_ids.len(),
            "contentLength": reply.len(),
        }));
        if mode == stream_reply_mode::Partial {
            payload.insert(
                "error".to_string(),
                Value::String(partial_delivery_error(self.partial_err.as_deref())),
            );
            payload.insert(
                "errorClass".to_string(),
                Value::String(classify_error(None)),
            );
        }
        self.pending.push(DeferredOp::ConnectorEvent {
            name: event_name.to_string(),
            payload,
        });
        if mode == stream_reply_mode::Complete {
            self.pending.push(DeferredOp::ReplyOutcome {
                record: self.record.clone(),
                status: "sent".to_string(),
                reason: "reply_sent".to_string(),
                evidence: string_map(serde_json::json!({
                    "messageId": self.inbound.external_message_id,
                    "replyMessageId": self.record.external_message_id,
                    "replyMessageIds": reply_message_ids.join(","),
                })),
            });
        } else if mode == stream_reply_mode::Partial {
            self.pending.push(DeferredOp::ReplyOutcome {
                record: self.record.clone(),
                status: "partial".to_string(),
                reason: safe_reply_failure_reason(None),
                evidence: string_map(serde_json::json!({
                    "errorClass": classify_error(None),
                    "messageId": self.inbound.external_message_id,
                })),
            });
        }
        self.last_flushed = reply;
        self.last_flush_at = now;
        Ok(())
    }
}
