//! Telegram connector runtime (port of runtime.go).

use std::collections::HashMap;
use std::sync::Arc;

use chrono::{DateTime, Utc};
use kura_chat::CancellationToken;
use kura_connectors::{
    CapabilityProfile, ConformanceArea, ConformanceResultStatus, Connector,
    DiagnosticReasonCode, GroupRoomCapabilities, HandoffCapabilities, RegisterInput, Status,
    Supervisor, SurfaceSupport, core_invariant_areas,
};
use kura_events::{Bus, Event, Resource, Scope};
use kura_im::MessageLoop;
use kura_imtypes::InboundMessage;
use kura_router::SessionKind;
use kura_store::{
    ConnectorAccountBindingSummary, SQLiteStore, TelegramAllowmentRecord,
    TelegramHostedSetupRecord, TelegramUpdateEvidenceRecord,
};
use parking_lot::Mutex;

use crate::allowment::{
    AllowmentIndex, AllowmentValidation, ConversationType, InboundUpdate, RouteDecision,
    RouteOutcome, decide_route, new_allowment_index,
};
use crate::diagnostics::{build_diagnostic_state, diagnostic_reason_for_error};
use crate::readiness::{HostedSetup, HostedSetupInput, evaluate_hosted_setup};
use crate::transport::{FakeTransport, Transport, normalize_command_text};
use crate::{TelegramError, is_unset_time};

/// Telegram connector configuration (Go `Config`). The Go daemon reads the
/// tenant id from context (`tenantctx`); that is not ported, so the tenant
/// id is passed explicitly through [`Config::tenant_id`].
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Config {
    pub enabled: bool,
    pub connector_id: String,
    pub display_name: String,
    pub bot_token: String,
    pub bot_username: String,
    pub tenant_id: String,
    pub allowments: Vec<AllowmentValidation>,
}

impl Config {
    /// Go NewRuntime validation: connector id and display name are required.
    pub fn validate(&self) -> Result<(), TelegramError> {
        if self.connector_id.trim().is_empty() {
            return Err(TelegramError::ConnectorIdRequired);
        }
        if self.display_name.trim().is_empty() {
            return Err(TelegramError::DisplayNameRequired);
        }
        Ok(())
    }
}

/// The Telegram connector runtime (Go `Runtime`).
pub struct Runtime {
    inner: Arc<Mutex<RuntimeInner>>,
}

struct RuntimeInner {
    cfg: Config,
    supervisor: Arc<Supervisor>,
    loop_: Option<MessageLoop>,
    store: Option<Arc<SQLiteStore>>,
    event_bus: Option<Bus>,
    transport: Arc<dyn Transport>,
    allowments: AllowmentIndex,
    cancel: CancellationToken,
    started: bool,
}

impl Runtime {
    /// Go `NewRuntime`. Returns `None` when the connector is disabled; the
    /// transport defaults to a [`FakeTransport`] when absent.
    pub fn new(
        cfg: Config,
        supervisor: Arc<Supervisor>,
        loop_: Option<MessageLoop>,
        sqlite_store: Option<Arc<SQLiteStore>>,
        event_bus: Option<Bus>,
        transport: Option<Arc<dyn Transport>>,
    ) -> Result<Option<Runtime>, TelegramError> {
        if !cfg.enabled {
            return Ok(None);
        }
        cfg.validate()?;
        let transport = transport.unwrap_or_else(|| Arc::new(FakeTransport::new()));
        let allowments = new_allowment_index(cfg.allowments.clone());
        let inner = RuntimeInner {
            cfg,
            supervisor,
            loop_,
            store: sqlite_store,
            event_bus,
            transport,
            allowments,
            cancel: CancellationToken::new(),
            started: false,
        };
        Ok(Some(Runtime { inner: Arc::new(Mutex::new(inner)) }))
    }

    /// Go `Start`: registers the connector with the supervisor, persists it,
    /// and starts the transport long-poll.
    pub fn start(&self) -> Result<(), TelegramError> {
        let mut inner = self.inner.lock();
        if inner.started {
            return Ok(());
        }
        inner.started = true;
        let tenant_id = inner.runtime_tenant_id();
        let (connector, _created) = inner
            .supervisor
            .register(RegisterInput {
                tenant_id,
                connector_id: inner.cfg.connector_id.clone(),
                kind: "telegram".to_string(),
                display_name: inner.cfg.display_name.clone(),
                ..RegisterInput::default()
            })
            .map_err(TelegramError::from)?;
        if let Some(store) = &inner.store {
            store.upsert_connector(&connector).map_err(TelegramError::Store)?;
        }
        // The store and message loop are not Send in this workspace (the
        // SQLite connection is single-threaded), so the transport long-poll
        // cannot drive handle_update directly. The poll thread forwards
        // updates through a channel and the calling thread becomes the
        // connector's single processing loop (Go's per-connector goroutine
        // run inline). Close() drops the sender side and returns from the
        // loop once the transport disconnects.
        let (tx, rx) = std::sync::mpsc::channel::<InboundUpdate>();
        let handle: Arc<dyn Fn(InboundUpdate) + Send + Sync> = Arc::new(move |update| {
            let _ = tx.send(update);
        });
        inner.transport.start(handle).map_err(TelegramError::Transport)?;
        drop(inner);
        while let Ok(update) = rx.recv() {
            self.handle_update(update);
        }
        Ok(())
    }

    /// Go `Close`: stops the transport.
    pub fn close(&self) -> Result<(), String> {
        self.inner.lock().transport.close()
    }

    /// Go `RecordHostedSetupValidation`: evaluates the setup, persists the
    /// hosted-setup projection (setup + allowments) when a store is present,
    /// and publishes the `connector.telegram_setup_validated` event.
    pub fn record_hosted_setup_validation(
        &self,
        input: HostedSetupInput,
    ) -> Result<HostedSetup, TelegramError> {
        let inner = self.inner.lock();
        let mut input = input;
        if input.tenant_id.trim().is_empty() {
            input.tenant_id = inner.runtime_tenant_id();
        }
        if input.connector_id.trim().is_empty() {
            input.connector_id = inner.cfg.connector_id.clone();
        }
        if input.display_name.trim().is_empty() {
            input.display_name = inner.cfg.display_name.clone();
        }
        let setup = evaluate_hosted_setup(input);
        if let Some(store) = &inner.store {
            let record = TelegramHostedSetupRecord {
                tenant_id: setup.tenant_id.clone(),
                connector_id: setup.connector_id.clone(),
                connector_kind: setup.connector_kind.clone(),
                display_name: setup.display_name.clone(),
                status: setup.status.as_str().to_string(),
                terminal_state: setup.terminal_state.as_str().to_string(),
                hosted_ready: setup.hosted_ready,
                credential_state: setup.credential_state.as_str().to_string(),
                allowment_state: setup.allowment_state.as_str().to_string(),
                group_behavior: setup.group_behavior.as_str().to_string(),
                delivery_eligible: setup.delivery_eligible,
                reason_code: setup.reason_code.clone(),
                redaction_status: setup.redaction_status.as_str().to_string(),
                created_at: setup.created_at,
                updated_at: setup.updated_at,
                validated_at: Some(setup.validated_at),
                retention_expires_at: setup.retention_expires_at,
                account_binding: if setup.account_binding.connector_account_id.trim().is_empty() {
                    None
                } else {
                    Some(ConnectorAccountBindingSummary {
                        tenant_id: setup.account_binding.tenant_id.clone(),
                        connector_id: setup.account_binding.connector_id.clone(),
                        connector_account_id: setup.account_binding.connector_account_id.clone(),
                        display_name: setup.account_binding.provider_account_label.clone(),
                        provider_account_hint: setup.account_binding.provider_account_label.clone(),
                        redaction_status: setup.account_binding.redaction_status.as_str().to_string(),
                        updated_at: setup.account_binding.validated_at,
                    })
                },
                allowments: Vec::new(),
            };
            store.save_telegram_hosted_setup(&record).map_err(TelegramError::Store)?;
            for allowment in &setup.allowments {
                store
                    .save_telegram_allowment(&TelegramAllowmentRecord {
                        tenant_id: allowment.tenant_id.clone(),
                        connector_id: allowment.connector_id.clone(),
                        allowment_id: allowment.allowment_id.clone(),
                        scope_type: allowment.scope_type.as_str().to_string(),
                        scope_id: allowment.scope_id.clone(),
                        provider_label: allowment.provider_label.clone(),
                        enabled: allowment.enabled,
                        group_gate: allowment
                            .group_gate
                            .map(|gate| gate.as_str().to_string())
                            .unwrap_or_default(),
                        validation_state: allowment.validation_state.as_str().to_string(),
                        reason_code: allowment.reason_code.clone(),
                        validated_at: allowment.validated_at,
                        redaction_status: allowment.redaction_status.as_str().to_string(),
                        safe_evidence: allowment.safe_evidence.clone(),
                    })
                    .map_err(TelegramError::Store)?;
            }
        }
        if let Some(bus) = &inner.event_bus {
            bus.publish(telegram_setup_validated_event(TelegramSetupValidatedInput {
                tenant_id: setup.tenant_id.clone(),
                connector_id: setup.connector_id.clone(),
                terminal_state: setup.terminal_state.as_str().to_string(),
                hosted_ready: setup.hosted_ready,
                credential_state: setup.credential_state.as_str().to_string(),
                allowment_state: setup.allowment_state.as_str().to_string(),
                reason_code: setup.reason_code.clone(),
                redaction_status: setup.redaction_status.as_str().to_string(),
                validated_at: setup.validated_at,
            }));
        }
        Ok(setup)
    }

    /// Go `NormalizeInbound`: normalizes the raw update into an inbound
    /// message, recording route evidence for every decision and returning
    /// `None` for anything but an accepted route.
    pub fn normalize_inbound(&self, update: InboundUpdate) -> Option<InboundMessage> {
        self.inner.lock().normalize_inbound(update)
    }

    /// Dispatches one raw update through normalization and the message loop
    /// (Go `handleUpdate`).
    pub(crate) fn handle_update(&self, update: InboundUpdate) {
        self.inner.lock().handle_update(update);
    }
}
impl RuntimeInner {
    /// Go `handleUpdate`: normalize, run the message loop, and record
    /// duplicate/error evidence and diagnostics.
    fn handle_update(&self, update: InboundUpdate) {
        let Some(inbound) = self.normalize_inbound(update.clone()) else {
            return;
        };
        let Some(loop_) = &self.loop_ else {
            return;
        };
        let now = Utc::now();
        let connector = Connector {
            connector_id: self.cfg.connector_id.clone(),
            kind: "telegram".to_string(),
            display_name: self.cfg.display_name.clone(),
            status: Status::Healthy,
            created_at: now,
            updated_at: now,
            ..Connector::default()
        };
        match loop_.process_single_turn(&connector, &inbound, &*self.transport, &self.cancel) {
            Ok(process) => {
                if process.duplicate {
                    let surface = update.conversation_type.as_str().to_string();
                    self.record_update_evidence(
                        &update,
                        &RouteDecision {
                            outcome: RouteOutcome::Duplicate,
                            reason_code: DiagnosticReasonCode::DuplicateInbound.as_str().to_string(),
                            surface: surface.clone(),
                        },
                    );
                    self.record_diagnostic(
                        DiagnosticReasonCode::DuplicateInbound,
                        HashMap::from([
                            ("messageId".to_string(), update.message_id.clone()),
                            ("chatId".to_string(), update.chat_id.clone()),
                            ("surface".to_string(), surface),
                        ]),
                    );
                }
            }
            Err(err) => {
                self.record_diagnostic(
                    diagnostic_reason_for_error(&PlainError(err.clone())),
                    HashMap::from([
                        ("messageId".to_string(), update.message_id.clone()),
                        ("chatId".to_string(), update.chat_id.clone()),
                    ]),
                );
            }
        }
    }

    /// Go `NormalizeInbound`.
    fn normalize_inbound(&self, mut update: InboundUpdate) -> Option<InboundMessage> {
        // Go treats the empty conversation type as direct; the enum default is
        // already `Direct`, so no normalization is needed here.
        if !update.mentioned && !self.cfg.bot_username.trim().is_empty() {
            let (text, mentioned, command) = normalize_command_text(&update.text, &self.cfg.bot_username);
            update.text = text;
            update.mentioned = mentioned;
            update.command = update.command || command;
        }
        let decision = decide_route(&update, &self.allowments);
        if decision.outcome != RouteOutcome::Accepted {
            self.record_update_evidence(&update, &decision);
            self.record_route_outcome(decision.clone());
            if let Some(reason) = Self::diagnostic_reason_for_route_decision(&decision) {
                self.record_diagnostic(
                    reason,
                    HashMap::from([
                        ("messageId".to_string(), update.message_id.clone()),
                        ("chatId".to_string(), update.chat_id.clone()),
                        ("surface".to_string(), decision.surface.clone()),
                    ]),
                );
            }
            return None;
        }
        self.record_update_evidence(&update, &decision);
        let now = if is_unset_time(&update.received_at) {
            Utc::now()
        } else {
            update.received_at
        };
        let group = update.conversation_type == ConversationType::Group;
        let kind = if group { SessionKind::Group } else { SessionKind::Direct };
        let peer_id = if group { update.chat_id.clone() } else { update.sender_id.clone() };
        let account_id = self.connector_account_id();
        Some(InboundMessage {
            connector_id: self.cfg.connector_id.clone(),
            connector_kind: "telegram".to_string(),
            external_message_id: update.message_id.clone(),
            tenant_id: self.runtime_tenant_id(),
            account_id: account_id.clone(),
            connector_account_id: account_id,
            channel_or_conversation_id: update.chat_id.clone(),
            provider_message_id: update.message_id.clone(),
            equivalent_rule_id: "telegram_chat_message_id".to_string(),
            channel_id: update.chat_id.clone(),
            peer_id,
            author_id: update.sender_id.clone(),
            content: update.text.trim().to_string(),
            kind,
            direct: !group,
            mentioned: update.mentioned || update.command,
            received_at: now,
            ..InboundMessage::default()
        })
    }

    /// Go `recordRouteOutcome`: forwards the decision to the transport's
    /// optional recorder.
    fn record_route_outcome(&self, decision: RouteDecision) {
        self.transport.record_route_outcome(decision);
    }

    /// Go `recordUpdateEvidence`: persists Telegram update evidence.
    fn record_update_evidence(&self, update: &InboundUpdate, decision: &RouteDecision) {
        let Some(store) = &self.store else {
            return;
        };
        if update.chat_id.trim().is_empty()
            || update.message_id.trim().is_empty()
            || update.update_id.trim().is_empty()
        {
            return;
        }
        let received_at = if is_unset_time(&update.received_at) {
            Utc::now()
        } else {
            update.received_at
        };
        let _ = store.save_telegram_update_evidence(&TelegramUpdateEvidenceRecord {
            tenant_id: self.runtime_tenant_id(),
            connector_id: self.cfg.connector_id.clone(),
            chat_id: update.chat_id.clone(),
            message_id: update.message_id.clone(),
            update_id: update.update_id.clone(),
            route_outcome: decision.outcome.as_str().to_string(),
            reason_code: decision.reason_code.clone(),
            received_at,
            retention_expires_at: received_at + chrono::Duration::days(90),
            redaction_status: "redacted".to_string(),
            safe_evidence: HashMap::from([
                ("identityRule".to_string(), "telegram_chat_message_id".to_string()),
                ("surface".to_string(), decision.surface.clone()),
            ]),
        });
    }

    /// Go `recordDiagnostic`: builds (redacting evidence) and persists a
    /// connector diagnostic state.
    fn record_diagnostic(&self, reason: DiagnosticReasonCode, evidence: HashMap<String, String>) {
        let Some(store) = &self.store else {
            return;
        };
        let state = match build_diagnostic_state(
            &self.runtime_tenant_id(),
            &self.cfg.connector_id,
            &self.connector_account_id(),
            reason,
            evidence,
            Utc::now(),
        ) {
            Ok(state) => state,
            Err(_) => return,
        };
        let _ = store.save_connector_diagnostic_state(&state);
    }

    /// Go `diagnosticReasonForRouteDecision`.
    fn diagnostic_reason_for_route_decision(decision: &RouteDecision) -> Option<DiagnosticReasonCode> {
        match decision.outcome {
            RouteOutcome::Blocked => Some(DiagnosticReasonCode::BlockedRoute),
            RouteOutcome::Unsupported => Some(DiagnosticReasonCode::UnsupportedCapability),
            RouteOutcome::Failed => Some(DiagnosticReasonCode::UnknownConnectorFailure),
            _ => None,
        }
    }

    /// Go `connectorAccountID`.
    fn connector_account_id(&self) -> String {
        let username = self.cfg.bot_username.trim();
        if !username.is_empty() {
            format!("bot_{}", username.trim_start_matches('@'))
        } else {
            self.cfg.connector_id.clone()
        }
    }

    /// Go `runtimeTenantID`: the explicit [`Config::tenant_id`] source,
    /// falling back to the store's default personal tenant id.
    fn runtime_tenant_id(&self) -> String {
        let configured = self.cfg.tenant_id.trim();
        if !configured.is_empty() {
            return configured.to_string();
        }
        if let Some(store) = &self.store {
            if let Ok(tenant_id) = store.resolve_default_personal_tenant_id() {
                return tenant_id.trim().to_string();
            }
        }
        String::new()
    }
}
/// Go `ConformanceProfile`: the Telegram capability profile with exact
/// surface declarations and the equivalent durable identity rule.
#[must_use]
pub fn conformance_profile(cfg: &Config, declared_at: DateTime<Utc>) -> CapabilityProfile {
    let declared_at = if is_unset_time(&declared_at) { Utc::now() } else { declared_at };
    let core = core_invariant_areas()
        .into_iter()
        .map(|area| (area, ConformanceResultStatus::Pass))
        .collect::<HashMap<ConformanceArea, ConformanceResultStatus>>();
    CapabilityProfile {
        profile_id: format!("profile_telegram_{}", cfg.connector_id),
        tenant_id: String::new(),
        connector_id: cfg.connector_id.clone(),
        connector_kind: "telegram".to_string(),
        core_invariant_results: core,
        provider_surface_results: HashMap::from([
            ("direct_message".to_string(), SurfaceSupport::Supported),
            ("group_message".to_string(), SurfaceSupport::Supported),
            ("mention_gating".to_string(), SurfaceSupport::Supported),
            ("command_gating".to_string(), SurfaceSupport::Supported),
            ("final_only_foreground_reply".to_string(), SurfaceSupport::Supported),
            ("connector_backed_delivery".to_string(), SurfaceSupport::Supported),
            ("attachments".to_string(), SurfaceSupport::Unsupported),
            ("voice".to_string(), SurfaceSupport::Unsupported),
            ("payments".to_string(), SurfaceSupport::Unsupported),
            ("mini_apps".to_string(), SurfaceSupport::Unsupported),
            ("media_transfer".to_string(), SurfaceSupport::Unsupported),
            ("thinking_visibility".to_string(), SurfaceSupport::Unsupported),
            ("incremental_visible_updates".to_string(), SurfaceSupport::Unsupported),
            ("standard_durable_identity".to_string(), SurfaceSupport::Supported),
            ("blocked_route_classification".to_string(), SurfaceSupport::Supported),
        ]),
        group_room_capabilities: GroupRoomCapabilities {
            mention_evidence: Some(SurfaceSupport::Supported),
            allowlist_evidence: Some(SurfaceSupport::Supported),
            unsupported_source_evidence: Some(SurfaceSupport::Limited),
            duplicate_message_evidence: Some(SurfaceSupport::Supported),
            edited_message_evidence: Some(SurfaceSupport::Unsupported),
            deleted_message_evidence: Some(SurfaceSupport::Unsupported),
        },
        handoff_capabilities: HandoffCapabilities {
            source_support: Some(SurfaceSupport::Supported),
            destination_support: Some(SurfaceSupport::Supported),
            first_response_source_references: Some(SurfaceSupport::Supported),
        },
        equivalent_durable_identity_rule_id: "telegram_chat_message_id".to_string(),
        equivalent_durable_identity_rule: "tenant_id + connector_account_id + telegram_chat_id + telegram_message_id".to_string(),
        declared_at,
    }
}
/// Input to [`telegram_setup_validated_event`] (Go
/// `events.ConnectorTelegramSetupValidatedInput`).
#[derive(Debug, Clone, PartialEq)]
pub struct TelegramSetupValidatedInput {
    pub tenant_id: String,
    pub connector_id: String,
    pub terminal_state: String,
    pub hosted_ready: bool,
    pub credential_state: String,
    pub allowment_state: String,
    pub reason_code: String,
    pub redaction_status: String,
    pub validated_at: DateTime<Utc>,
}

/// Go `events.ConnectorTelegramSetupValidated`: builds the redacted setup
/// validation event. The events crate does not (yet) own this emitter, so
/// it is built here from the shared [`Event`] wire type.
#[must_use]
pub fn telegram_setup_validated_event(input: TelegramSetupValidatedInput) -> Event {
    Event {
        category: "connector".to_string(),
        name: "connector.telegram_setup_validated".to_string(),
        scope: Scope { connector_id: input.connector_id.clone(), ..Scope::default() },
        resource: Resource { kind: "telegram_hosted_setup".to_string(), id: input.connector_id.clone() },
        payload: serde_json::json!({
            "tenantId": input.tenant_id,
            "connectorId": input.connector_id,
            "terminalState": input.terminal_state,
            "hostedReady": input.hosted_ready,
            "credentialState": input.credential_state,
            "allowmentState": input.allowment_state,
            "reasonCode": input.reason_code,
            "redactionStatus": input.redaction_status,
            "validatedAt": serde_json::to_value(input.validated_at).unwrap_or_default(),
        })
        .as_object()
        .cloned()
        .unwrap_or_default(),
        ..Event::default()
    }
}

/// String error wrapper used to classify loop errors (Go plain `error`
/// values that never implement the classified interface).
#[derive(Debug, Clone)]
pub(crate) struct PlainError(pub(crate) String);

impl std::fmt::Display for PlainError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for PlainError {}
#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use kura_chat::Service as ChatService;
    use kura_checkpoints::Manager as CheckpointManager;
    use kura_events::Filter;
    use kura_im::ReplySender;
    use kura_imtypes::OutboundReply;
    use kura_llm::{Dispatcher, Provider, ProviderError, ProviderRequest, ProviderResponse, StreamEmitter, Usage};
    use kura_router::SessionRouter;
    use kura_runtime::Manager as RuntimeManager;
    use kura_store::SQLiteStore;
    use futures::future::BoxFuture;
    use tempfile::tempdir;

    use crate::allowment::{AllowmentValidationState, GroupGate, ScopeType};
    use crate::readiness::{AccountBinding, CredentialState, PermissionState, TerminalState};
    use crate::transport::{FakeTransport, Transport};

    fn ts(y: i32, mo: u32, d: u32, h: u32, mi: u32, s: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(y, mo, d, h, mi, s).single().expect("valid timestamp")
    }

    fn allow_dm() -> AllowmentValidation {
        AllowmentValidation {
            scope_type: ScopeType::DirectChat,
            scope_id: "chat_1".to_string(),
            enabled: true,
            validation_state: AllowmentValidationState::Valid,
            ..AllowmentValidation::default()
        }
    }

    fn base_config() -> Config {
        Config {
            enabled: true,
            connector_id: "telegram-main".to_string(),
            display_name: "Telegram Main".to_string(),
            allowments: vec![allow_dm()],
            ..Config::default()
        }
    }

    /// Go telegramTestProvider: echoes the first message content with a
    /// reply: prefix (stream and complete agree).
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

    /// Builds a store-backed message loop with the echo provider registered.
    fn build_loop(store: Arc<SQLiteStore>, bus: Bus) -> MessageLoop {
        let dispatcher = Arc::new(Dispatcher::new());
        dispatcher.register_provider(Arc::new(EchoTestProvider));
        dispatcher.set_default_provider("echo").expect("default provider");
        dispatcher.set_default_model("echo-v1");
        let chat = ChatService::new_service(dispatcher, None, None, Some(bus.clone()), None);
        let runtime = Arc::new(RuntimeManager::new());
        let checkpoints = CheckpointManager::new(
            Arc::new(parking_lot::Mutex::new(
                SQLiteStore::new(store.data_dir()).expect("store"),
            )),
            runtime.clone(),
        );
        MessageLoop::new(
            SessionRouter::new(),
            runtime,
            Some(checkpoints),
            Some(bus),
            store,
            chat,
        )
    }

    fn direct_update(update_id: &str, message_id: &str, chat_id: &str, sender_id: &str, text: &str) -> InboundUpdate {
        InboundUpdate {
            update_id: update_id.to_string(),
            message_id: message_id.to_string(),
            chat_id: chat_id.to_string(),
            sender_id: sender_id.to_string(),
            text: text.to_string(),
            conversation_type: ConversationType::Direct,
            ..InboundUpdate::default()
        }
    }

    // Go TestRuntimeNormalizesTelegramIdentityAndBlocksUnallowedRoutes.
    #[test]
    fn runtime_normalizes_telegram_identity_and_blocks_unallowed_routes() {
        let transport = Arc::new(FakeTransport::new());
        let runtime = Runtime::new(
            base_config(),
            Arc::new(Supervisor::new()),
            None,
            None,
            None,
            Some(transport.clone()),
        )
        .expect("new runtime")
        .expect("enabled runtime");

        let mut inbound = direct_update("update_1", "message_1", "chat_1", "user_1", "hello");
        inbound.received_at = ts(2026, 5, 8, 10, 0, 0);
        let normalized = runtime.normalize_inbound(inbound).expect("normalized inbound");
        assert_eq!(normalized.connector_kind, "telegram");
        assert_eq!(normalized.equivalent_rule_id, "telegram_chat_message_id");
        assert_eq!(normalized.provider_message_id, "message_1");

        assert!(
            runtime
                .normalize_inbound(direct_update("update_2", "message_2", "chat_2", "user_2", "hello"))
                .is_none(),
            "unallowed sender/chat must not normalize into accepted inbound"
        );
        assert_eq!(transport.last_route_outcome().outcome, RouteOutcome::Blocked);
    }

    // Go TestFakeTransportSendsFinalOnlyReplies.
    #[test]
    fn fake_transport_sends_final_only_replies() {
        let transport = FakeTransport::new();
        let sent = transport
            .send_reply(OutboundReply {
                connector_id: "telegram-main".to_string(),
                channel_id: "chat_1".to_string(),
                content: "final".to_string(),
                ..OutboundReply::default()
            })
            .expect("send reply");
        assert!(!sent.external_message_id.is_empty());
        assert!(!transport.reply_capabilities().supports_streaming);
    }

    // Go TestRuntimeEnforcesTelegramGroupMentionAndCommandGate.
    #[test]
    fn runtime_enforces_telegram_group_mention_and_command_gate() {
        let transport = Arc::new(FakeTransport::new());
        let mut cfg = base_config();
        cfg.bot_username = "kura_test_bot".to_string();
        cfg.allowments = vec![AllowmentValidation {
            scope_type: ScopeType::Group,
            scope_id: "group_1".to_string(),
            enabled: true,
            group_gate: Some(GroupGate::MentionOrCommandRequired),
            validation_state: AllowmentValidationState::Valid,
            ..AllowmentValidation::default()
        }];
        let runtime = Runtime::new(
            cfg,
            Arc::new(Supervisor::new()),
            None,
            None,
            None,
            Some(transport.clone()),
        )
        .expect("new runtime")
        .expect("enabled runtime");

        assert!(
            runtime
                .normalize_inbound(InboundUpdate {
                    update_id: "update_group_ignored".to_string(),
                    message_id: "message_group_ignored".to_string(),
                    chat_id: "group_1".to_string(),
                    sender_id: "user_1".to_string(),
                    text: "hello group".to_string(),
                    conversation_type: ConversationType::Group,
                    ..InboundUpdate::default()
                })
                .is_none(),
            "allowed group without mention or command should be ignored"
        );
        let outcome = transport.last_route_outcome();
        assert_eq!(outcome.outcome, RouteOutcome::Ignored);
        assert_eq!(outcome.reason_code, "mention_required");

        let mentioned = runtime
            .normalize_inbound(InboundUpdate {
                update_id: "update_group_mentioned".to_string(),
                message_id: "message_group_mentioned".to_string(),
                chat_id: "group_1".to_string(),
                sender_id: "user_1".to_string(),
                text: "@kura_test_bot summarize this".to_string(),
                conversation_type: ConversationType::Group,
                ..InboundUpdate::default()
            })
            .expect("mentioned group message normalized");
        assert!(mentioned.mentioned);
        assert_eq!(mentioned.content, "summarize this");

        let command = runtime
            .normalize_inbound(InboundUpdate {
                update_id: "update_group_command".to_string(),
                message_id: "message_group_command".to_string(),
                chat_id: "group_1".to_string(),
                sender_id: "user_1".to_string(),
                text: "/kura summarize this".to_string(),
                conversation_type: ConversationType::Group,
                ..InboundUpdate::default()
            })
            .expect("command group message normalized");
        assert!(command.mentioned);
        assert_eq!(command.content, "/kura summarize this");
    }

    // Go TestRuntimeRejectsUnsupportedTelegramSurfaces.
    #[test]
    fn runtime_rejects_unsupported_telegram_surfaces() {
        let transport = Arc::new(FakeTransport::new());
        let runtime = Runtime::new(
            base_config(),
            Arc::new(Supervisor::new()),
            None,
            None,
            None,
            Some(transport.clone()),
        )
        .expect("new runtime")
        .expect("enabled runtime");

        for surface in ["attachment", "media_transfer", "voice", "payment", "mini_app"] {
            assert!(
                runtime
                    .normalize_inbound(InboundUpdate {
                        update_id: format!("update_{surface}"),
                        message_id: format!("message_{surface}"),
                        chat_id: "chat_1".to_string(),
                        sender_id: "user_1".to_string(),
                        text: "unsupported".to_string(),
                        conversation_type: ConversationType::Direct,
                        unsupported_surface: surface.to_string(),
                        ..InboundUpdate::default()
                    })
                    .is_none(),
                "{surface} should not normalize into accepted inbound"
            );
            let outcome = transport.last_route_outcome();
            assert_eq!(outcome.outcome, RouteOutcome::Unsupported, "{surface}");
            assert_eq!(outcome.surface, surface, "{surface}");
        }
    }

    // Go TestRuntimePersistsRedactedTelegramUpdateEvidence.
    #[test]
    fn runtime_persists_redacted_telegram_update_evidence() {
        let dir = tempdir().expect("tempdir");
        let store = Arc::new(SQLiteStore::new(dir.path().to_str().expect("path")).expect("store"));
        let mut cfg = base_config();
        cfg.tenant_id = "ten_telegram".to_string();
        let runtime = Runtime::new(
            cfg,
            Arc::new(Supervisor::new()),
            None,
            Some(store.clone()),
            None,
            Some(Arc::new(FakeTransport::new())),
        )
        .expect("new runtime")
        .expect("enabled runtime");

        let mut inbound = direct_update("update_1", "message_1", "chat_1", "user_1", "hello");
        inbound.received_at = ts(2026, 5, 8, 10, 0, 0);
        assert!(runtime.normalize_inbound(inbound).is_some());

        // The Go test lists with the same fixed timestamp as the update, so
        // the 90-day retention window still covers the row.
        let evidence = store
            .list_telegram_update_evidence("ten_telegram", "telegram-main", ts(2026, 5, 8, 10, 0, 0), 10)
            .expect("list update evidence");
        assert_eq!(evidence.len(), 1);
        assert_eq!(evidence[0].route_outcome, "accepted");
        assert_eq!(
            evidence[0].safe_evidence.get("identityRule").map(String::as_str),
            Some("telegram_chat_message_id")
        );
    }

    // Go TestRuntimeRecordsTelegramSetupValidationEventAndStoreProjection.
    #[test]
    fn runtime_records_telegram_setup_validation_event_and_store_projection() {
        let dir = tempdir().expect("tempdir");
        let store = Arc::new(SQLiteStore::new(dir.path().to_str().expect("path")).expect("store"));
        let event_bus = Bus::new();
        let runtime = Runtime::new(
            Config {
                enabled: true,
                connector_id: "telegram-main".to_string(),
                display_name: "Telegram Main".to_string(),
                ..Config::default()
            },
            Arc::new(Supervisor::new()),
            None,
            Some(store.clone()),
            Some(event_bus.clone()),
            Some(Arc::new(FakeTransport::new())),
        )
        .expect("new runtime")
        .expect("enabled runtime");

        let now = ts(2026, 5, 8, 10, 1, 0);
        let setup = runtime
            .record_hosted_setup_validation(HostedSetupInput {
                tenant_id: "ten_telegram".to_string(),
                credential: CredentialState::Valid,
                account_binding: AccountBinding {
                    connector_account_id: "bot_redacted".to_string(),
                    provider_account_label: "telegram:bot_redacted".to_string(),
                    permission_state: PermissionState::Valid,
                    ..AccountBinding::default()
                },
                allowments: vec![AllowmentValidation {
                    allowment_id: "allow_dm".to_string(),
                    scope_type: ScopeType::DirectChat,
                    scope_id: "chat_redacted".to_string(),
                    enabled: true,
                    validation_state: AllowmentValidationState::Valid,
                    ..AllowmentValidation::default()
                }],
                started_at: now,
                validated_at: now,
                ..HostedSetupInput::default()
            })
            .expect("record hosted setup validation");
        assert_eq!(setup.terminal_state, TerminalState::Ready);

        let stored = store
            .get_telegram_hosted_setup("ten_telegram", "telegram-main")
            .expect("get hosted setup")
            .expect("stored setup");
        assert_eq!(stored.terminal_state, "ready");
        assert_eq!(stored.allowments.len(), 1);
        let binding = stored.account_binding.as_ref().expect("account binding retained");
        assert_eq!(binding.connector_account_id, "bot_redacted");
        assert_eq!(binding.provider_account_hint, "telegram:bot_redacted");

        let published = event_bus.list(&Filter { category: "connector".to_string(), ..Filter::default() });
        assert_eq!(published.len(), 1);
        assert_eq!(published[0].name, "connector.telegram_setup_validated");
        assert_eq!(
            published[0].payload.get("redactionStatus").and_then(|v| v.as_str()),
            Some("redacted")
        );
        assert_eq!(
            published[0].payload.get("credentialState").and_then(|v| v.as_str()),
            Some("valid")
        );
    }

    // Go TestRuntimeUpdatesTelegramEvidenceWhenMessageLoopDetectsDuplicate.
    #[test]
    fn runtime_updates_telegram_evidence_when_message_loop_detects_duplicate() {
        let dir = tempdir().expect("tempdir");
        let store = Arc::new(SQLiteStore::new(dir.path().to_str().expect("path")).expect("store"));
        let event_bus = Bus::new();
        let loop_ = build_loop(store.clone(), event_bus.clone());
        let transport = Arc::new(FakeTransport::new());
        let mut cfg = base_config();
        cfg.tenant_id = "ten_telegram".to_string();
        let runtime = Runtime::new(
            cfg,
            Arc::new(Supervisor::new()),
            Some(loop_),
            Some(store.clone()),
            Some(event_bus),
            Some(transport.clone()),
        )
        .expect("new runtime")
        .expect("enabled runtime");

        let mut update = direct_update("update_1", "message_1", "chat_1", "user_1", "hello");
        update.received_at = ts(2026, 5, 8, 10, 0, 0);

        runtime.handle_update(update.clone());
        runtime.handle_update(update);

        let evidence = store
            .list_telegram_update_evidence("ten_telegram", "telegram-main", ts(2026, 5, 8, 10, 0, 0), 10)
            .expect("list update evidence");
        assert_eq!(evidence.len(), 1, "duplicate must upsert the same evidence row");
        assert_eq!(evidence[0].route_outcome, "duplicate");
        assert_eq!(evidence[0].reason_code, "duplicate_inbound");
    }

    // Go TestTelegramConformanceProfileDeclaresExplicitSurfaces.
    #[test]
    fn telegram_conformance_profile_declares_explicit_surfaces() {
        let profile = conformance_profile(
            &Config { connector_id: "telegram-main".to_string(), ..Config::default() },
            ts(2026, 5, 8, 10, 0, 0),
        );
        assert_eq!(profile.connector_kind, "telegram");
        assert_eq!(profile.equivalent_durable_identity_rule_id, "telegram_chat_message_id");
        for surface in [
            "direct_message",
            "group_message",
            "mention_gating",
            "command_gating",
            "final_only_foreground_reply",
            "connector_backed_delivery",
        ] {
            assert_eq!(
                profile.provider_surface_results.get(surface).copied(),
                Some(SurfaceSupport::Supported),
                "{surface} must be supported"
            );
        }
        for surface in [
            "attachments",
            "voice",
            "payments",
            "mini_apps",
            "media_transfer",
            "thinking_visibility",
            "incremental_visible_updates",
        ] {
            assert_eq!(
                profile.provider_surface_results.get(surface).copied(),
                Some(SurfaceSupport::Unsupported),
                "{surface} must be unsupported"
            );
        }
    }
}
