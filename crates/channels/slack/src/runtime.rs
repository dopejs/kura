//! Slack connector runtime (port of runtime.go): configuration, supervisor
//! registration, inbound normalization + routing + evidence, hosted-setup
//! validation persistence, and the conformance capability profile.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use chrono::{DateTime, Duration, Utc};

use kura_connectors::{
    CapabilityProfile, ConformanceResultStatus, Connector, DiagnosticReasonCode,
    GroupRoomCapabilities, HandoffCapabilities, RedactionStatus, RegisterInput, Status, Supervisor,
    SurfaceSupport, core_invariant_areas,
};
use kura_events::{Bus, Event, Resource, Scope};
use kura_im::{MessageLoop, ReplySender};
use kura_imtypes::{InboundMessage, OutboundReply, SentReply};
use kura_router::SessionKind;
use kura_store::SQLiteStore;
use kura_store::slack_setup::{
    SlackConversationRouteRecord, SlackEventEvidenceRecord, SlackHostedSetupRecord,
    SlackRoutePolicyRecord, SlackWorkspaceBinding,
};
use parking_lot::Mutex;

use crate::destinations::{
    ConversationRoute, ConversationType, RoutePolicy, RouteValidationState, SelectedChannelState,
    normalize_route_policy,
};
use crate::diagnostics::{build_diagnostic_state, diagnostic_reason_for_message};
use crate::error::SlackError;
use crate::readiness::{HostedSetup, HostedSetupInput, TerminalState, evaluate_hosted_setup};
use crate::route::{InboundEvent, RouteDecision, RouteOutcome, decide_route};
use crate::transport::{FakeTransport, Transport};
use crate::util::{first_non_empty, is_unset_time, json_object};

/// Slack connector configuration (Go Config). tenant_id stands in for Go's
/// tenantctx: the runtime sources the tenant id from this field (falling back
/// to the store's default personal tenant), matching where the Go code reads
/// it from the context.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Config {
    pub enabled: bool,
    pub connector_id: String,
    pub display_name: String,
    pub workspace_binding_id: String,
    pub workspace_id: String,
    pub bot_user_id: String,
    pub allowed_channel_ids: Vec<String>,
    pub allowed_dm_user_ids: Vec<String>,
    pub allowed_dm_user_groups: Vec<String>,
    pub tenant_id: String,
}

/// Slack connector runtime (Go Runtime).
pub struct Runtime {
    cfg: Config,
    supervisor: Arc<Supervisor>,
    loop_: MessageLoop,
    store: Option<Arc<SQLiteStore>>,
    event_bus: Option<Bus>,
    transport: Arc<dyn Transport>,
    policy: RoutePolicy,
    seen: Mutex<HashSet<String>>,
    started: Mutex<bool>,
}

/// Go NewRuntime: returns None when the connector is disabled.
pub fn new_runtime(
    cfg: Config,
    supervisor: Arc<Supervisor>,
    loop_: MessageLoop,
    store: Option<Arc<SQLiteStore>>,
    event_bus: Option<Bus>,
    transport: Option<Arc<dyn Transport>>,
) -> Result<Option<Runtime>, SlackError> {
    if !cfg.enabled {
        return Ok(None);
    }
    if cfg.connector_id.trim().is_empty() {
        return Err(SlackError::ConnectorIdRequired);
    }
    if cfg.display_name.trim().is_empty() {
        return Err(SlackError::DisplayNameRequired);
    }
    // Go also rejects a nil supervisor/loop; the supervisor is an Arc (never
    // nil) and the loop is required by type, so that check is unreachable.
    let transport = transport.unwrap_or_else(|| Arc::new(FakeTransport::new(Vec::new())));
    Ok(Some(Runtime {
        policy: route_policy_from_config(&cfg),
        cfg,
        supervisor,
        loop_,
        store,
        event_bus,
        transport,
        seen: Mutex::new(HashSet::new()),
        started: Mutex::new(false),
    }))
}

impl Runtime {
    /// Go Start: registers with the supervisor, persists the connector, and
    /// starts the transport (which synchronously delivers the seeded inbound
    /// events).
    pub fn start(&self) -> Result<(), SlackError> {
        {
            let mut started = self.started.lock();
            if *started {
                return Ok(());
            }
            *started = true;
        }
        let tenant_id = self.runtime_tenant_id();
        let (connector, _created) = self
            .supervisor
            .register(RegisterInput {
                tenant_id,
                connector_id: self.cfg.connector_id.clone(),
                kind: "slack".to_string(),
                display_name: self.cfg.display_name.clone(),
                secret_refs: None,
            })
            .map_err(|e| SlackError::Message(e.to_string()))?;
        if let Some(store) = &self.store {
            store
                .upsert_connector(&connector)
                .map_err(SlackError::Message)?;
        }
        self.transport
            .start(Box::new(|event| self.handle_event(event)))
    }

    /// Go Close.
    pub fn close(&self) -> Result<(), SlackError> {
        *self.started.lock() = false;
        self.transport.close()
    }

    /// The connector id this runtime receives and replies as.
    #[must_use]
    pub fn connector_id(&self) -> String {
        self.cfg.connector_id.clone()
    }

    /// An owned sender over this runtime's transport, for replies sent
    /// outside an inbound message's handling (the work-item outbox's
    /// retries, work-items.md §8).
    #[must_use]
    pub fn reply_sender(&self) -> Arc<dyn ReplySender> {
        Arc::new(OwnedReplySender(self.transport.clone()))
    }

    /// Go NormalizeInboundEvent: fills identity defaults, requires the hosted
    /// setup, applies route policy + duplicate suppression, and normalizes an
    /// accepted event into an inbound message. Returns None for every
    /// non-accepted outcome.
    pub fn normalize_inbound_event(&self, event: InboundEvent) -> Option<InboundMessage> {
        let mut event = event;
        if event.tenant_id.trim().is_empty() {
            event.tenant_id = self.runtime_tenant_id();
        }
        if event.connector_id.trim().is_empty() {
            event.connector_id = self.cfg.connector_id.clone();
        }
        let (decision, ok) = self.require_hosted_setup_ready(&event);
        if !ok {
            self.record_event_evidence(&event, &decision);
            self.record_route_outcome(&event, &decision);
            return None;
        }
        let mut decision = decide_route(
            &event,
            &self.policy,
            &self.cfg.workspace_id,
            &self.cfg.bot_user_id,
        );
        if decision.outcome == RouteOutcome::Accepted {
            let key = slack_message_identity_key(&[
                &event.tenant_id,
                &event.connector_id,
                &event.workspace_id,
                &event.conversation_id,
                &event.message_id,
            ]);
            if self.mark_duplicate(&key) {
                decision = RouteDecision {
                    outcome: RouteOutcome::Duplicate,
                    reason_code: DiagnosticReasonCode::DuplicateInbound.as_str().to_string(),
                    surface: decision.surface,
                    normalized_text: String::new(),
                };
            }
        }
        self.record_event_evidence(&event, &decision);
        self.record_route_outcome(&event, &decision);
        if decision.outcome != RouteOutcome::Accepted {
            return None;
        }
        let received_at = if is_unset_time(&event.received_at) {
            Utc::now()
        } else {
            event.received_at
        };
        let mut kind = SessionKind::Group;
        let mut peer_id = event.conversation_id.clone();
        let mut reply_to_message_id = event.thread_root_message_id.clone();
        if reply_to_message_id.trim().is_empty()
            && event.conversation_type == ConversationType::Channel
        {
            reply_to_message_id = event.message_id.clone();
        }
        if event.conversation_type == ConversationType::DirectMessage {
            kind = SessionKind::Direct;
            peer_id = event.sender_id.clone();
            reply_to_message_id = event.message_id.clone();
        }
        Some(InboundMessage {
            connector_id: event.connector_id.clone(),
            connector_kind: "slack".to_string(),
            external_message_id: event.message_id.clone(),
            tenant_id: event.tenant_id.clone(),
            account_id: event.workspace_id.clone(),
            connector_account_id: event.workspace_id.clone(),
            channel_or_conversation_id: event.conversation_id.clone(),
            provider_message_id: event.message_id.clone(),
            equivalent_rule_id: "slack_workspace_conversation_message_id".to_string(),
            channel_id: event.conversation_id.clone(),
            peer_id,
            thread_id: first_non_empty(&[&event.thread_root_message_id, &event.message_id]),
            author_id: event.sender_id.clone(),
            content: decision.normalized_text,
            kind,
            reply_to_message_id,
            received_at,
            direct: event.conversation_type == ConversationType::DirectMessage,
            mentioned: event.conversation_type == ConversationType::Channel,
            ..InboundMessage::default()
        })
    }

    /// Go RecordHostedSetupValidation: evaluates the setup, persists the
    /// hosted setup + route policy projections, and publishes the validation
    /// event.
    pub fn record_hosted_setup_validation(
        &self,
        mut input: HostedSetupInput,
    ) -> Result<HostedSetup, SlackError> {
        if input.tenant_id.trim().is_empty() {
            input.tenant_id = self.runtime_tenant_id();
        }
        if input.connector_id.trim().is_empty() {
            input.connector_id = self.cfg.connector_id.clone();
        }
        if input.display_name.trim().is_empty() {
            input.display_name = self.cfg.display_name.clone();
        }
        let setup = evaluate_hosted_setup(input);
        if let Some(store) = &self.store {
            store
                .save_slack_hosted_setup(&slack_hosted_setup_record(&setup))
                .map_err(SlackError::Message)?;
            // Go gates on a non-empty route-policy validation state; policy
            // normalization guarantees one, so the policy is always persisted.
            store
                .save_slack_route_policy(&slack_route_policy_record(&setup))
                .map_err(SlackError::Message)?;
        }
        if let Some(bus) = &self.event_bus {
            bus.publish(slack_setup_validated_event(&setup));
        }
        Ok(setup)
    }
}

impl Runtime {
    /// Go handleEvent: normalizes the inbound event and drives one message
    /// loop turn, recording reply/duplicate diagnostics.
    fn handle_event(&self, event: InboundEvent) {
        let Some(inbound) = self.normalize_inbound_event(event.clone()) else {
            return;
        };
        let connector = Connector {
            connector_id: self.cfg.connector_id.clone(),
            kind: "slack".to_string(),
            display_name: self.cfg.display_name.clone(),
            status: Status::Healthy,
            created_at: Utc::now(),
            updated_at: Utc::now(),
            ..Connector::default()
        };
        let cancel = kura_chat::CancellationToken::new();
        let sender = TransportReplySender {
            transport: self.transport.as_ref(),
        };
        match self
            .loop_
            .process_single_turn(&connector, &inbound, &sender, &cancel)
        {
            Ok(result) => {
                if result.duplicate {
                    self.record_diagnostic(
                        DiagnosticReasonCode::DuplicateInbound,
                        HashMap::from([
                            ("workspaceId".to_string(), event.workspace_id.clone()),
                            ("conversationId".to_string(), event.conversation_id.clone()),
                            (
                                "surface".to_string(),
                                event.conversation_type.as_str().to_string(),
                            ),
                            ("stage".to_string(), "durable_dedupe".to_string()),
                        ]),
                    );
                }
            }
            Err(err) => {
                let reason = diagnostic_reason_for_message(&err);
                self.record_diagnostic(
                    reason,
                    HashMap::from([
                        ("workspaceId".to_string(), event.workspace_id.clone()),
                        ("conversationId".to_string(), event.conversation_id.clone()),
                        (
                            "surface".to_string(),
                            event.conversation_type.as_str().to_string(),
                        ),
                        ("stage".to_string(), "message_loop".to_string()),
                    ]),
                );
            }
        }
    }

    /// Go recordDiagnostic.
    fn record_diagnostic(&self, reason: DiagnosticReasonCode, evidence: HashMap<String, String>) {
        let Some(store) = &self.store else {
            return;
        };
        let tenant_id = self.runtime_tenant_id();
        let Ok(state) = build_diagnostic_state(
            &tenant_id,
            &self.cfg.connector_id,
            &self.cfg.workspace_binding_id,
            reason,
            &evidence,
            Utc::now(),
        ) else {
            return;
        };
        let _ = store.save_connector_diagnostic_state(&state);
    }

    /// Go markDuplicate.
    fn mark_duplicate(&self, key: &str) -> bool {
        let mut seen = self.seen.lock();
        if seen.contains(key) {
            return true;
        }
        seen.insert(key.to_string());
        false
    }

    /// Go requireHostedSetupReady.
    fn require_hosted_setup_ready(&self, event: &InboundEvent) -> (RouteDecision, bool) {
        let Some(store) = &self.store else {
            return (RouteDecision::default(), true);
        };
        let surface = first_non_empty(&[&event.surface, event.conversation_type.as_str()]);
        let setup = match store.get_slack_hosted_setup(&event.tenant_id, &event.connector_id) {
            Ok(Some(setup)) => setup,
            Ok(None) => {
                return (
                    RouteDecision {
                        outcome: RouteOutcome::Blocked,
                        reason_code: DiagnosticReasonCode::AuthMissing.as_str().to_string(),
                        surface,
                        normalized_text: String::new(),
                    },
                    false,
                );
            }
            Err(_) => {
                return (
                    RouteDecision {
                        outcome: RouteOutcome::Failed,
                        reason_code: DiagnosticReasonCode::UnknownConnectorFailure
                            .as_str()
                            .to_string(),
                        surface,
                        normalized_text: String::new(),
                    },
                    false,
                );
            }
        };
        if setup.terminal_state != TerminalState::Ready.as_str() || !setup.delivery_eligible {
            let mut reason = setup.reason_code.trim().to_string();
            if reason.is_empty() || reason == "healthy" {
                reason = DiagnosticReasonCode::AuthMissing.as_str().to_string();
            }
            return (
                RouteDecision {
                    outcome: RouteOutcome::Blocked,
                    reason_code: reason,
                    surface,
                    normalized_text: String::new(),
                },
                false,
            );
        }
        if !setup.workspace_binding_id.trim().is_empty()
            && !self.cfg.workspace_binding_id.trim().is_empty()
            && setup.workspace_binding_id.trim() != self.cfg.workspace_binding_id.trim()
        {
            return (
                RouteDecision {
                    outcome: RouteOutcome::Blocked,
                    reason_code: "workspace_mismatch".to_string(),
                    surface,
                    normalized_text: String::new(),
                },
                false,
            );
        }
        (RouteDecision::default(), true)
    }

    /// Go recordEventEvidence.
    fn record_event_evidence(&self, event: &InboundEvent, decision: &RouteDecision) {
        let Some(store) = &self.store else {
            return;
        };
        let received_at = if is_unset_time(&event.received_at) {
            Utc::now()
        } else {
            event.received_at
        };
        let _ = store.save_slack_event_evidence(&SlackEventEvidenceRecord {
            tenant_id: event.tenant_id.clone(),
            connector_id: event.connector_id.clone(),
            workspace_id: event.workspace_id.clone(),
            conversation_id: event.conversation_id.clone(),
            message_id: event.message_id.clone(),
            event_id: event.event_id.clone(),
            route_outcome: decision.outcome.as_str().to_string(),
            reason_code: decision.reason_code.clone(),
            received_at,
            retention_expires_at: received_at + Duration::days(90),
            redaction_status: RedactionStatus::Redacted.as_str().to_string(),
            safe_evidence: HashMap::from([
                (
                    "identityRule".to_string(),
                    "slack_workspace_conversation_message_id".to_string(),
                ),
                ("surface".to_string(), decision.surface.clone()),
            ]),
        });
    }

    /// Go recordRouteOutcome.
    fn record_route_outcome(&self, event: &InboundEvent, decision: &RouteDecision) {
        let Some(bus) = &self.event_bus else {
            return;
        };
        bus.publish(Event {
            category: "connector".to_string(),
            name: "connector.route_outcome_recorded".to_string(),
            tenant_id: event.tenant_id.clone(),
            scope: Scope {
                connector_id: event.connector_id.clone(),
                ..Scope::default()
            },
            resource: Resource {
                kind: "connector_route_outcome".to_string(),
                id: event.message_id.clone(),
            },
            payload: json_object(serde_json::json!({
                "tenantId": event.tenant_id,
                "connectorId": event.connector_id,
                "workspaceId": event.workspace_id,
                "conversationId": event.conversation_id,
                "messageId": event.message_id,
                "eventId": event.event_id,
                "outcome": decision.outcome.as_str(),
                "reasonCode": decision.reason_code,
                "surface": decision.surface,
                "redactionStatus": "redacted",
            })),
            ..Event::default()
        });
    }

    /// Go runtimeTenantID.
    fn runtime_tenant_id(&self) -> String {
        if !self.cfg.tenant_id.trim().is_empty() {
            return self.cfg.tenant_id.trim().to_string();
        }
        if let Some(store) = &self.store {
            if let Ok(tenant_id) = store.resolve_default_personal_tenant_id() {
                return tenant_id.trim().to_string();
            }
        }
        String::new()
    }
}

/// Adapter exposing a Transport as the loop's ReplySender (Go passes the
/// transport directly because it satisfies the imtypes reply methods).
struct TransportReplySender<'a> {
    transport: &'a dyn Transport,
}

impl ReplySender for TransportReplySender<'_> {
    fn send_reply(&self, reply: OutboundReply) -> Result<SentReply, String> {
        self.transport.send_reply(&reply).map_err(|e| e.to_string())
    }
}

/// [`Runtime::reply_sender`]: the transport, owned.
struct OwnedReplySender(Arc<dyn Transport>);

impl ReplySender for OwnedReplySender {
    fn send_reply(&self, reply: OutboundReply) -> Result<SentReply, String> {
        self.0.send_reply(&reply).map_err(|e| e.to_string())
    }
}

/// Go ConformanceProfile: the default capability profile for a config.
#[must_use]
pub fn conformance_profile(cfg: &Config, declared_at: DateTime<Utc>) -> CapabilityProfile {
    conformance_profile_for_setup(cfg, &HostedSetup::default(), declared_at)
}

/// Go ConformanceProfileForSetup.
#[must_use]
pub fn conformance_profile_for_setup(
    cfg: &Config,
    setup: &HostedSetup,
    declared_at: DateTime<Utc>,
) -> CapabilityProfile {
    let ready = setup.terminal_state == TerminalState::Ready;
    conformance_profile_inner(cfg, setup, ready, declared_at)
}

/// Go conformanceProfileForSetup.
fn conformance_profile_inner(
    cfg: &Config,
    setup: &HostedSetup,
    ready: bool,
    declared_at: DateTime<Utc>,
) -> CapabilityProfile {
    let declared_at = if is_unset_time(&declared_at) {
        Utc::now()
    } else {
        declared_at
    };
    let core_status = if ready {
        ConformanceResultStatus::Pass
    } else {
        ConformanceResultStatus::Fail
    };
    let mut core = HashMap::with_capacity(core_invariant_areas().len());
    for area in core_invariant_areas() {
        core.insert(area, core_status);
    }
    let surfaces: HashMap<String, SurfaceSupport> = HashMap::from([
        ("hosted_oauth_setup".to_string(), SurfaceSupport::Supported),
        (
            "submitted_token_setup".to_string(),
            SurfaceSupport::Unsupported,
        ),
        ("workspace_binding".to_string(), SurfaceSupport::Supported),
        (
            "multiple_connectors_per_tenant".to_string(),
            SurfaceSupport::Supported,
        ),
        (
            "direct_message".to_string(),
            support_flag(
                !cfg.allowed_dm_user_ids.is_empty() || !cfg.allowed_dm_user_groups.is_empty(),
            ),
        ),
        (
            "selected_channel_mention".to_string(),
            support_flag(!cfg.allowed_channel_ids.is_empty()),
        ),
        (
            "channel_thread_reply".to_string(),
            SurfaceSupport::Supported,
        ),
        (
            "final_only_foreground_reply".to_string(),
            SurfaceSupport::Supported,
        ),
        (
            "connector_backed_delivery".to_string(),
            SurfaceSupport::Supported,
        ),
        (
            "marketplace_publication".to_string(),
            SurfaceSupport::Unsupported,
        ),
        (
            "enterprise_grid_administration".to_string(),
            SurfaceSupport::Unsupported,
        ),
        (
            "memory_based_team_context".to_string(),
            SurfaceSupport::Unsupported,
        ),
        ("files".to_string(), SurfaceSupport::Unsupported),
        ("voice_huddles".to_string(), SurfaceSupport::Unsupported),
        ("canvases".to_string(), SurfaceSupport::Unsupported),
        ("workflow_buttons".to_string(), SurfaceSupport::Unsupported),
        (
            "interactive_blocks".to_string(),
            SurfaceSupport::Unsupported,
        ),
        ("rich_media".to_string(), SurfaceSupport::Unsupported),
        (
            "thinking_visibility".to_string(),
            SurfaceSupport::Unsupported,
        ),
        (
            "incremental_visible_updates".to_string(),
            SurfaceSupport::Unsupported,
        ),
        (
            "standard_durable_identity".to_string(),
            SurfaceSupport::Supported,
        ),
        (
            "blocked_route_classification".to_string(),
            SurfaceSupport::Supported,
        ),
    ]);
    let mention = support_flag(!cfg.allowed_channel_ids.is_empty());
    CapabilityProfile {
        profile_id: format!("profile_slack_{}", cfg.connector_id),
        tenant_id: setup.tenant_id.clone(),
        connector_id: cfg.connector_id.clone(),
        connector_kind: "slack".to_string(),
        core_invariant_results: core,
        provider_surface_results: surfaces,
        group_room_capabilities: GroupRoomCapabilities {
            mention_evidence: Some(mention),
            allowlist_evidence: Some(mention),
            unsupported_source_evidence: Some(SurfaceSupport::Limited),
            duplicate_message_evidence: Some(SurfaceSupport::Supported),
            edited_message_evidence: Some(SurfaceSupport::Unsupported),
            deleted_message_evidence: Some(SurfaceSupport::Unsupported),
        },
        handoff_capabilities: HandoffCapabilities {
            source_support: Some(mention),
            destination_support: Some(mention),
            first_response_source_references: Some(SurfaceSupport::Supported),
        },
        equivalent_durable_identity_rule_id: "slack_workspace_conversation_message_id".to_string(),
        equivalent_durable_identity_rule:
            "tenant_id + connector_id + workspace_id + conversation_id + slack_message_id"
                .to_string(),
        declared_at,
    }
}

/// Go supportFlag.
#[must_use]
pub fn support_flag(enabled: bool) -> SurfaceSupport {
    if enabled {
        SurfaceSupport::Supported
    } else {
        SurfaceSupport::Unsupported
    }
}

/// Go routePolicyFromConfig.
fn route_policy_from_config(cfg: &Config) -> RoutePolicy {
    let channels = cfg
        .allowed_channel_ids
        .iter()
        .filter(|id| !id.trim().is_empty())
        .map(|id| ConversationRoute {
            conversation_id: id.trim().to_string(),
            conversation_type: ConversationType::Channel,
            selected_channel_state: SelectedChannelState::Selected,
            validation_state: RouteValidationState::Valid,
            redaction_status: Some(RedactionStatus::Redacted),
            ..ConversationRoute::default()
        })
        .collect();
    normalize_route_policy(
        RoutePolicy {
            connector_id: cfg.connector_id.clone(),
            workspace_binding_id: cfg.workspace_binding_id.clone(),
            selected_channels: channels,
            allowed_dm_users: cfg.allowed_dm_user_ids.clone(),
            allowed_dm_user_groups: cfg.allowed_dm_user_groups.clone(),
            validation_state: RouteValidationState::Valid,
            redaction_status: RedactionStatus::Redacted,
            ..RoutePolicy::default()
        },
        Utc::now(),
    )
}

/// Go slackMessageIdentityKey: NUL-joined trimmed identity components.
#[must_use]
pub fn slack_message_identity_key(values: &[&str]) -> String {
    let cleaned: Vec<String> = values
        .iter()
        .map(|value| value.trim().to_string())
        .collect();
    cleaned.join("\0")
}

/// Go slackRoutePolicyRecord (runtime.go).
fn slack_route_policy_record(setup: &HostedSetup) -> SlackRoutePolicyRecord {
    let policy = setup.route_policy.clone().unwrap_or_default();
    let selected = policy
        .selected_channels
        .iter()
        .map(|route| SlackConversationRouteRecord {
            conversation_id: route.conversation_id.clone(),
            conversation_type: route.conversation_type.as_str().to_string(),
            selected_channel_state: route.selected_channel_state.as_str().to_string(),
            validation_state: route.validation_state.as_str().to_string(),
            reason_code: route.reason_code.clone(),
            redaction_status: route
                .redaction_status
                .map(|status| status.as_str().to_string())
                .unwrap_or_default(),
            safe_evidence: route.safe_evidence.clone(),
        })
        .collect();
    SlackRoutePolicyRecord {
        tenant_id: setup.tenant_id.clone(),
        connector_id: setup.connector_id.clone(),
        workspace_binding_id: setup.workspace_binding_id.clone(),
        selected_channels: selected,
        allowed_dm_users: policy.allowed_dm_users.clone(),
        allowed_dm_user_groups: policy.allowed_dm_user_groups.clone(),
        mention_gate: policy.mention_gate.clone(),
        thread_reply_mode: policy.thread_reply_mode.clone(),
        validation_state: policy.validation_state.as_str().to_string(),
        reason_code: policy.reason_code.clone(),
        validated_at: policy.validated_at.unwrap_or_else(Utc::now),
        redaction_status: policy.redaction_status.as_str().to_string(),
        safe_evidence: policy.safe_evidence.clone(),
    }
}

/// Go's inline hosted-setup -> store record conversion (runtime.go).
fn slack_hosted_setup_record(setup: &HostedSetup) -> SlackHostedSetupRecord {
    let mut record = SlackHostedSetupRecord {
        tenant_id: setup.tenant_id.clone(),
        connector_id: setup.connector_id.clone(),
        connector_kind: setup.connector_kind.clone(),
        display_name: setup.display_name.clone(),
        status: setup.status.as_str().to_string(),
        terminal_state: setup.terminal_state.as_str().to_string(),
        oauth_state: setup.oauth_state.as_str().to_string(),
        route_policy_state: setup.route_policy_state.as_str().to_string(),
        delivery_eligible: setup.delivery_eligible,
        workspace_binding_id: setup.workspace_binding_id.clone(),
        reason_code: setup.reason_code.clone(),
        redaction_status: setup.redaction_status.as_str().to_string(),
        created_at: setup.created_at.unwrap_or_else(Utc::now),
        updated_at: setup.updated_at.unwrap_or_else(Utc::now),
        validated_at: setup.validated_at,
        retention_expires_at: setup
            .retention_expires_at
            .unwrap_or_else(|| Utc::now() + Duration::days(90)),
        workspace_binding: None,
        route_policy: None,
    };
    if let Some(binding) = &setup.workspace_binding {
        if !binding.workspace_id.trim().is_empty() || !binding.installation_id.trim().is_empty() {
            record.workspace_binding = Some(SlackWorkspaceBinding {
                tenant_id: binding.tenant_id.clone(),
                connector_id: binding.connector_id.clone(),
                workspace_binding_id: binding.workspace_binding_id.clone(),
                workspace_id: binding.workspace_id.clone(),
                workspace_label: binding.workspace_label.clone(),
                installation_id: binding.installation_id.clone(),
                oauth_grant_state: binding.oauth_grant_state.clone(),
                required_scope_state: binding.required_scope_state.clone(),
                validated_at: binding.validated_at,
                redaction_status: binding.redaction_status.as_str().to_string(),
                safe_evidence: binding.safe_evidence.clone(),
            });
        }
    }
    record
}

/// Go slackConditionForSetup.
fn slack_condition_for_setup(setup: &HostedSetup) -> String {
    if setup.reason_code.is_empty() || setup.reason_code == "healthy" {
        "healthy".to_string()
    } else {
        setup.reason_code.clone()
    }
}

/// Go events.ConnectorSlackSetupValidated.
fn slack_setup_validated_event(setup: &HostedSetup) -> Event {
    Event {
        category: "connector".to_string(),
        name: "connector.slack_setup_validated".to_string(),
        tenant_id: setup.tenant_id.clone(),
        scope: Scope {
            connector_id: setup.connector_id.clone(),
            ..Scope::default()
        },
        resource: Resource {
            kind: "slack_hosted_setup".to_string(),
            id: setup.connector_id.clone(),
        },
        payload: json_object(serde_json::json!({
            "tenantId": setup.tenant_id,
            "connectorId": setup.connector_id,
            "workspaceBindingId": setup.workspace_binding_id,
            "terminalState": setup.terminal_state.as_str(),
            "oauthState": setup.oauth_state.as_str(),
            "routePolicyState": setup.route_policy_state.as_str(),
            "deliveryEligible": setup.delivery_eligible,
            "reasonCode": setup.reason_code,
            "slackCondition": slack_condition_for_setup(setup),
            "redactionStatus": setup.redaction_status.as_str(),
            "validatedAt": setup.validated_at.unwrap_or_default().to_rfc3339(),
        })),
        ..Event::default()
    }
}
