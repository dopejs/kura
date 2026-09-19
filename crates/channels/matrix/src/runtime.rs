//! Port of daemon/internal/connectors/matrix/runtime.go: the Matrix connector
//! runtime — supervisor registration, hosted-setup readiness gating, route
//! decisions with durable dedupe, event-evidence persistence, route-outcome
//! bus events, and message-loop dispatch.
//!
//! Tenant context is passed explicitly: the tenant id given to
//! [Runtime::start] fills event tenant ids that arrive empty, falling back to
//! the store's default personal tenant exactly where Go consulted the context
//! and then the store.

use std::collections::HashSet;
use std::sync::Arc;

use chrono::{Duration, Utc};
use kura_connectors::{
    Connector, DiagnosticReasonCode, MATRIX_DURABLE_IDENTITY_RULE_ID, RedactionStatus,
    RegisterInput, Status, Supervisor,
};
use kura_events::{Bus, Event, Resource, Scope};
use kura_im::MessageLoop;
use kura_imtypes::InboundMessage;
use kura_router::SessionKind;
use kura_store::SQLiteStore;
use kura_store::matrix_setup::MatrixEventEvidenceRecord;
use parking_lot::Mutex;

use crate::is_unset_time;
use crate::routes::{decide_route, normalize_route_policy};
use crate::transport::{FakeTransport, Transport, TransportReplySender};
use crate::types::{
    CONNECTOR_KIND, Config, ConversationRoute, ConversationType, InboundEvent, MessageKind,
    RoomSelectionState, RouteDecision, RouteOutcome, RoutePolicy, RoutePolicyState, TerminalState,
};

/// Go `Runtime`.
pub struct Runtime {
    cfg: Config,
    tenant_id: Mutex<String>,
    supervisor: Arc<Supervisor>,
    message_loop: MessageLoop,
    store: Option<Arc<SQLiteStore>>,
    event_bus: Option<Bus>,
    transport: Box<dyn Transport>,
    policy: RoutePolicy,
    seen: Mutex<HashSet<String>>,
    started: Mutex<bool>,
}

/// Go `NewRuntime`. Returns `Ok(None)` when the connector is disabled.
pub fn new_runtime(
    cfg: Config,
    supervisor: Arc<Supervisor>,
    message_loop: MessageLoop,
    sqlite_store: Option<Arc<SQLiteStore>>,
    event_bus: Option<Bus>,
    transport: Option<Box<dyn Transport>>,
) -> Result<Option<Runtime>, String> {
    if !cfg.enabled {
        return Ok(None);
    }
    if cfg.connector_id.trim().is_empty() {
        return Err("matrix connector id is required".to_string());
    }
    if cfg.display_name.trim().is_empty() {
        return Err("matrix display name is required".to_string());
    }
    let transport = transport.unwrap_or_else(|| Box::new(FakeTransport::new(Vec::new())));
    Ok(Some(Runtime {
        policy: route_policy_from_config(&cfg),
        cfg,
        supervisor,
        message_loop,
        store: sqlite_store,
        event_bus,
        transport,
        tenant_id: Mutex::new(String::new()),
        seen: Mutex::new(HashSet::new()),
        started: Mutex::new(false),
    }))
}

impl Runtime {
    /// Go `Start(ctx)`: registers the connector with the supervisor, upserts
    /// it, and drives the transport's inbound loop. `tenant_id` replaces the
    /// Go tenant context.
    pub fn start(&self, tenant_id: &str) -> Result<(), String> {
        *self.tenant_id.lock() = tenant_id.trim().to_string();
        let mut started = self.started.lock();
        if *started {
            return Ok(());
        }
        *started = true;
        drop(started);
        let (connector, _) = self
            .supervisor
            .register(RegisterInput {
                tenant_id: self.runtime_tenant_id(),
                connector_id: self.cfg.connector_id.clone(),
                kind: CONNECTOR_KIND.to_string(),
                display_name: self.cfg.display_name.clone(),
                secret_refs: None,
            })
            .map_err(|e| e.to_string())?;
        if let Some(store) = &self.store {
            store.upsert_connector(&connector)?;
        }
        let handle = |event: InboundEvent| self.handle_event(event);
        self.transport.start(&handle)
    }

    /// Go `Close`.
    pub fn close(&self) -> Result<(), String> {
        *self.started.lock() = false;
        self.transport.close()
    }

    /// Method form of the inbound normalization (Go `Runtime.NormalizeInboundEvent`):
    /// normalizes, applies tenant/connector/homeserver defaults, gates on
    /// hosted-setup readiness, routes, dedupes, records evidence, and maps the
    /// accepted event into an [InboundMessage].
    pub fn normalize_inbound_event(&self, event: InboundEvent) -> (InboundMessage, bool) {
        let mut event = crate::runtime::normalize_inbound_event(event);
        if event.tenant_id.trim().is_empty() {
            event.tenant_id = self.runtime_tenant_id();
        }
        if event.connector_id.trim().is_empty() {
            event.connector_id = self.cfg.connector_id.clone();
        }
        if event.homeserver_id.trim().is_empty() {
            event.homeserver_id = self.cfg.homeserver_id.clone();
        }
        if let Some(decision) = self.require_hosted_setup_ready(&event) {
            self.record_event_evidence(&event, &decision);
            self.record_route_outcome(&event, &decision);
            return (InboundMessage::default(), false);
        }
        let mut decision = decide_route(
            &event,
            self.policy.clone(),
            &self.cfg.homeserver_id,
            &self.cfg.bot_user_id,
        );
        if decision.outcome == RouteOutcome::Accepted {
            let key = matrix_event_identity_key(&[
                event.tenant_id.clone(),
                event.connector_id.clone(),
                event.homeserver_id.clone(),
                event.conversation_id.clone(),
                event.matrix_event_id.clone(),
            ]);
            if self.has_persisted_duplicate(&event) || self.mark_duplicate(&key) {
                decision = RouteDecision {
                    outcome: RouteOutcome::Duplicate,
                    reason_code: DiagnosticReasonCode::DuplicateInbound.as_str().to_string(),
                    surface: decision.surface.clone(),
                    ..RouteDecision::default()
                };
            }
        }
        self.record_event_evidence(&event, &decision);
        self.record_route_outcome(&event, &decision);
        if decision.outcome != RouteOutcome::Accepted {
            return (InboundMessage::default(), false);
        }
        let (kind, peer_id, direct) = if event.conversation_type == ConversationType::DirectMessage
        {
            (SessionKind::Direct, event.sender_id.clone(), true)
        } else {
            (SessionKind::Group, event.conversation_id.clone(), false)
        };
        (
            InboundMessage {
                connector_id: event.connector_id.clone(),
                connector_kind: CONNECTOR_KIND.to_string(),
                external_message_id: event.matrix_event_id.clone(),
                tenant_id: event.tenant_id.clone(),
                account_id: event.homeserver_id.clone(),
                connector_account_id: event.homeserver_id.clone(),
                channel_or_conversation_id: event.conversation_id.clone(),
                provider_message_id: event.matrix_event_id.clone(),
                equivalent_rule_id: MATRIX_DURABLE_IDENTITY_RULE_ID.to_string(),
                channel_id: event.conversation_id.clone(),
                peer_id,
                thread_id: event.conversation_id.clone(),
                author_id: event.sender_id.clone(),
                content: decision.normalized_text.clone(),
                kind,
                reply_to_message_id: event.matrix_event_id.clone(),
                direct,
                mentioned: event.conversation_type == ConversationType::Room,
                received_at: event.received_at,
                ..InboundMessage::default()
            },
            true,
        )
    }

    /// Go `requireHostedSetupReady`: gates inbound traffic on a persisted
    /// hosted setup in the ready terminal state. `None` means ready.
    fn require_hosted_setup_ready(&self, event: &InboundEvent) -> Option<RouteDecision> {
        let store = self.store.as_ref()?;
        let setup = match store.get_matrix_hosted_setup(&event.tenant_id, &event.connector_id) {
            Ok(Some(setup)) => setup,
            Ok(None) => {
                return Some(RouteDecision {
                    outcome: RouteOutcome::Blocked,
                    reason_code: DiagnosticReasonCode::AuthMissing.as_str().to_string(),
                    surface: event.conversation_type.as_str().to_string(),
                    ..RouteDecision::default()
                });
            }
            Err(_) => {
                return Some(RouteDecision {
                    outcome: RouteOutcome::Failed,
                    reason_code: DiagnosticReasonCode::UnknownConnectorFailure
                        .as_str()
                        .to_string(),
                    surface: event.conversation_type.as_str().to_string(),
                    ..RouteDecision::default()
                });
            }
        };
        if setup.terminal_state != TerminalState::Ready.as_str() || !setup.delivery_eligible {
            let mut reason = setup.reason_code.trim().to_string();
            if reason.is_empty() || reason == "healthy" {
                reason = DiagnosticReasonCode::AuthMissing.as_str().to_string();
            }
            return Some(RouteDecision {
                outcome: RouteOutcome::Blocked,
                reason_code: reason,
                surface: event.conversation_type.as_str().to_string(),
                ..RouteDecision::default()
            });
        }
        None
    }

    /// Go `handleEvent`: normalizes the event and drives one message-loop
    /// turn with a healthy connector projection.
    pub fn handle_event(&self, event: InboundEvent) {
        let (inbound, ok) = self.normalize_inbound_event(event);
        if !ok {
            return;
        }
        let connector = Connector {
            connector_id: self.cfg.connector_id.clone(),
            kind: CONNECTOR_KIND.to_string(),
            display_name: self.cfg.display_name.clone(),
            status: Status::Healthy,
            created_at: Utc::now(),
            updated_at: Utc::now(),
            ..Connector::default()
        };
        let cancel = kura_chat::CancellationToken::new();
        let sender = TransportReplySender(self.transport.as_ref());
        let _ = self
            .message_loop
            .process_single_turn(&connector, &inbound, &sender, &cancel);
    }

    /// Go `markDuplicate`: in-memory dedupe of the durable event identity.
    fn mark_duplicate(&self, key: &str) -> bool {
        let mut seen = self.seen.lock();
        if seen.contains(key) {
            return true;
        }
        seen.insert(key.to_string());
        false
    }

    /// Go `hasPersistedDuplicate`: durable dedupe against persisted event
    /// evidence with an accepted/duplicate outcome.
    fn has_persisted_duplicate(&self, event: &InboundEvent) -> bool {
        let Some(store) = &self.store else {
            return false;
        };
        let items = match store.list_matrix_event_evidence(
            &event.tenant_id,
            &event.connector_id,
            Utc::now(),
            100,
        ) {
            Ok(items) => items,
            Err(_) => return false,
        };
        items.iter().any(|item| {
            item.homeserver_id == event.homeserver_id
                && item.conversation_id == event.conversation_id
                && item.matrix_event_id == event.matrix_event_id
                && (item.route_outcome == RouteOutcome::Accepted.as_str()
                    || item.route_outcome == RouteOutcome::Duplicate.as_str())
        })
    }

    /// Go `recordEventEvidence`: persists the matrix event evidence row
    /// (Go `SaveMatrixEventEvidence`).
    fn record_event_evidence(&self, event: &InboundEvent, decision: &RouteDecision) {
        let Some(store) = &self.store else {
            return;
        };
        let received_at = if is_unset_time(&event.received_at) {
            Utc::now()
        } else {
            event.received_at
        };
        let _ = store.save_matrix_event_evidence(&MatrixEventEvidenceRecord {
            tenant_id: event.tenant_id.clone(),
            connector_id: event.connector_id.clone(),
            homeserver_id: event.homeserver_id.clone(),
            conversation_id: event.conversation_id.clone(),
            matrix_event_id: event.matrix_event_id.clone(),
            sync_batch_id: event.sync_batch_id.clone(),
            transaction_id: event.transaction_id.clone(),
            route_outcome: decision.outcome.as_str().to_string(),
            reason_code: decision.reason_code.clone(),
            received_at,
            retention_expires_at: received_at + Duration::days(90),
            redaction_status: RedactionStatus::Redacted.as_str().to_string(),
            safe_evidence: std::collections::HashMap::from([
                (
                    "identityRule".to_string(),
                    MATRIX_DURABLE_IDENTITY_RULE_ID.to_string(),
                ),
                ("surface".to_string(), decision.surface.clone()),
            ]),
        });
    }

    /// Go `recordRouteOutcome`: publishes the connector route-outcome event
    /// (Go `events.ConnectorMatrixRouteOutcomeRecorded`).
    fn record_route_outcome(&self, event: &InboundEvent, decision: &RouteDecision) {
        let Some(bus) = &self.event_bus else {
            return;
        };
        let mut payload = serde_json::Map::new();
        payload.insert(
            "tenantId".to_string(),
            serde_json::Value::String(event.tenant_id.clone()),
        );
        payload.insert(
            "connectorId".to_string(),
            serde_json::Value::String(event.connector_id.clone()),
        );
        payload.insert(
            "homeserverId".to_string(),
            serde_json::Value::String(event.homeserver_id.clone()),
        );
        payload.insert(
            "conversationId".to_string(),
            serde_json::Value::String(event.conversation_id.clone()),
        );
        payload.insert(
            "matrixEventId".to_string(),
            serde_json::Value::String(event.matrix_event_id.clone()),
        );
        payload.insert(
            "syncBatchId".to_string(),
            serde_json::Value::String(event.sync_batch_id.clone()),
        );
        payload.insert(
            "transactionId".to_string(),
            serde_json::Value::String(event.transaction_id.clone()),
        );
        payload.insert(
            "outcome".to_string(),
            serde_json::Value::String(decision.outcome.as_str().to_string()),
        );
        payload.insert(
            "reasonCode".to_string(),
            serde_json::Value::String(decision.reason_code.clone()),
        );
        payload.insert(
            "surface".to_string(),
            serde_json::Value::String(decision.surface.clone()),
        );
        payload.insert(
            "redactionStatus".to_string(),
            serde_json::Value::String("redacted".to_string()),
        );
        bus.publish(Event {
            category: "connector".to_string(),
            name: "connector.route_outcome_recorded".to_string(),
            scope: Scope {
                connector_id: event.connector_id.clone(),
                ..Scope::default()
            },
            resource: Resource {
                kind: "connector_route_outcome".to_string(),
                id: event.matrix_event_id.clone(),
            },
            payload,
            ..Event::default()
        });
    }

    /// Go `runtimeTenantID`: the explicitly passed tenant, falling back to
    /// the store's default personal tenant.
    fn runtime_tenant_id(&self) -> String {
        let stored = self.tenant_id.lock().clone();
        if !stored.trim().is_empty() {
            return stored.trim().to_string();
        }
        if let Some(store) = &self.store {
            if let Ok(tenant_id) = store.resolve_default_personal_tenant_id() {
                return tenant_id.trim().to_string();
            }
        }
        String::new()
    }
}

/// Go package-level `NormalizeInboundEvent`.
#[must_use]
pub fn normalize_inbound_event(mut event: InboundEvent) -> InboundEvent {
    event.tenant_id = event.tenant_id.trim().to_string();
    event.connector_id = event.connector_id.trim().to_string();
    event.homeserver_id = event.homeserver_id.trim().to_string();
    event.conversation_id = event.conversation_id.trim().to_string();
    event.matrix_event_id = event.matrix_event_id.trim().to_string();
    event.sync_batch_id = event.sync_batch_id.trim().to_string();
    event.transaction_id = event.transaction_id.trim().to_string();
    event.sender_id = event.sender_id.trim().to_string();
    event.text = event.text.trim().to_string();
    if event.message_kind == MessageKind::default() {
        event.message_kind = MessageKind::UnencryptedText;
    }
    if is_unset_time(&event.received_at) {
        event.received_at = Utc::now();
    }
    event
}

/// Go `routePolicyFromConfig`.
fn route_policy_from_config(cfg: &Config) -> RoutePolicy {
    let now = Utc::now();
    let mut rooms = Vec::new();
    for id in &cfg.selected_room_ids {
        if id.trim().is_empty() {
            continue;
        }
        rooms.push(ConversationRoute {
            conversation_id: id.trim().to_string(),
            conversation_type: ConversationType::Room,
            room_selection_state: RoomSelectionState::Selected,
            validation_state: RoutePolicyState::Valid,
            redaction_status: RedactionStatus::Redacted,
            ..ConversationRoute::default()
        });
    }
    let state = if !rooms.is_empty() || !cfg.allowed_direct_user_ids.is_empty() {
        RoutePolicyState::Valid
    } else {
        RoutePolicyState::Blocked
    };
    normalize_route_policy(
        RoutePolicy {
            connector_id: cfg.connector_id.clone(),
            homeserver_binding_id: format!("matrix_homeserver_{}", cfg.connector_id.trim()),
            selected_rooms: rooms,
            allowed_direct_users: cfg.allowed_direct_user_ids.clone(),
            room_invocation_gate: "bot_mention_or_command_required".to_string(),
            configured_commands: cfg.configured_commands.clone(),
            encrypted_room_policy: "unsupported".to_string(),
            validation_state: state,
            validated_at: now,
            redaction_status: RedactionStatus::Redacted,
            ..RoutePolicy::default()
        },
        now,
    )
}

/// Go `matrixEventIdentityKey`.
#[must_use]
pub fn matrix_event_identity_key(values: &[String]) -> String {
    values
        .iter()
        .map(|value| value.trim())
        .collect::<Vec<&str>>()
        .join("\u{0}")
}
