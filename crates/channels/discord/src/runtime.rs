//! Connector runtime (port of runtime.go): wires a transport into the
//! supervisor, the kura-im message loop, the SQLite store, and the event bus,
//! and drives the inbound/route/diagnostic/conformance persistence.

use parking_lot::Mutex;
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver};

use chrono::{DateTime, Utc};
use kura_chat::CancellationToken;
use kura_connectors::{
    Connector, DiagnosticReasonCode, RegisterInput, ReportFailureInput, ReportHealthInput, Status,
    Supervisor,
};
use kura_events::{Bus, Event, Resource, Scope};
use kura_im::MessageLoop;
use kura_imtypes::InboundMessage;
use kura_store::SQLiteStore;
use kura_store::discord_setup::{DiscordDestinationValidationRecord, DiscordHostedSetupRecord};
use kura_telemetry::Logger;
use serde_json::{Map, Value};

use crate::config::{Config, credential_state_for_config, destination_evidence_for_config};
use crate::conformance::conformance_profile_for_setup;
use crate::destinations::{DestinationValidation, DestinationValidationState};
use crate::diagnostics::{
    build_diagnostic_state, classify_discord_error, classify_discord_error_message,
    diagnostic_reason_for_error,
};
use crate::readiness::{HostedSetup, evaluate_hosted_setup};
use crate::transport::{Transport, TransportLifecycleEvent};

/// One queued event for the runtime's inbound drain loop. The transport's
/// handler and lifecycle observer only send over a channel; all store/loop
/// access happens on the thread that owns the runtime (the SQLite connection
/// and message loop are not thread-safe).
enum RuntimeEvent {
    Inbound(InboundMessage),
    Lifecycle(TransportLifecycleEvent),
}

struct RuntimeInner {
    cfg: Config,
    logger: Option<Logger>,
    supervisor: Arc<Supervisor>,
    message_loop: Arc<MessageLoop>,
    store: Option<Arc<SQLiteStore>>,
    event_bus: Option<Bus>,
    transport: Box<dyn Transport>,
    started: Mutex<bool>,
    inbound_rx: Mutex<Option<Receiver<RuntimeEvent>>>,
}

/// Go `Runtime`. Owns the transport and persists connector/setup/diagnostic
/// evidence. The inbound drain loop (`drain_pending` / `run_inbound`) must
/// run on the thread that owns the runtime.
pub struct Runtime {
    inner: Arc<RuntimeInner>,
}

/// Go `NewRuntime`: returns None when the connector is disabled; validates
/// the config and builds the default `GatewayTransport` when none is given.
pub fn new_runtime(
    cfg: Config,
    logger: Option<Logger>,
    supervisor: Arc<Supervisor>,
    message_loop: Arc<MessageLoop>,
    sqlite_store: Option<Arc<SQLiteStore>>,
    event_bus: Option<Bus>,
    transport: Option<Box<dyn Transport>>,
) -> Result<Option<Runtime>, crate::DiscordError> {
    use crate::DiscordError;
    if !cfg.enabled {
        return Ok(None);
    }
    let mut cfg = cfg;
    if cfg.connector_id.trim().is_empty() {
        return Err(DiscordError::ConnectorIdRequired);
    }
    if cfg.display_name.trim().is_empty() {
        return Err(DiscordError::DisplayNameRequired);
    }
    let mode = cfg.delivery_mode.trim().to_string();
    if mode.is_empty() {
        cfg.delivery_mode = "gateway".to_string();
    } else if mode != "gateway" {
        return Err(DiscordError::UnsupportedDeliveryMode(mode));
    }
    if cfg.bot_token.trim().is_empty() {
        return Err(DiscordError::BotTokenRequired);
    }
    let transport = match transport {
        Some(transport) => transport,
        None => Box::new(crate::GatewayTransport::new(cfg.clone())?),
    };
    Ok(Some(Runtime {
        inner: Arc::new(RuntimeInner {
            cfg,
            logger,
            supervisor,
            message_loop,
            store: sqlite_store,
            event_bus,
            transport,
            started: Mutex::new(false),
            inbound_rx: Mutex::new(None),
        }),
    }))
}

impl Runtime {
    /// Go `Start(ctx)`: registers the connector, wires the lifecycle
    /// observer, starts the transport, persists the hosted-setup projection
    /// and conformance evidence, and reports health. Non-blocking: inbound
    /// messages queue until `drain_pending`/`run_inbound` processes them.
    pub fn start(&self) -> Result<(), crate::DiscordError> {
        use crate::DiscordError;
        let mut started = self.inner.started.lock();
        if *started {
            return Ok(());
        }
        *started = true;
        drop(started);

        let (tx, rx) = mpsc::channel::<RuntimeEvent>();
        *self.inner.inbound_rx.lock() = Some(rx);

        let connector = self
            .inner
            .supervisor
            .register(RegisterInput {
                tenant_id: self.runtime_tenant_id(),
                connector_id: self.inner.cfg.connector_id.clone(),
                kind: "discord".to_string(),
                display_name: self.inner.cfg.display_name.clone(),
                secret_refs: None,
            })
            .map_err(|err| DiscordError::Other(err.to_string()))?;
        self.persist_connector(&connector.0)?;

        if let Some(observable) = self.inner.transport.lifecycle_observable() {
            let event_tx = tx.clone();
            let observer: Arc<dyn Fn(TransportLifecycleEvent) + Send + Sync> =
                Arc::new(move |event: TransportLifecycleEvent| {
                    let _ = event_tx.send(RuntimeEvent::Lifecycle(event));
                });
            observable.set_lifecycle_observer(Some(observer));
        }

        let handler: Arc<dyn Fn(InboundMessage) + Send + Sync> =
            Arc::new(move |inbound: InboundMessage| {
                let _ = tx.send(RuntimeEvent::Inbound(inbound));
            });

        if let Err(err) = self.inner.transport.start(handler) {
            *self.inner.started.lock() = false;
            let _ = self.persist_diagnostic(
                diagnostic_reason_for_error(&err),
                HashMap::from([("stage".to_string(), "transport_start".to_string())]),
            );
            let _ = self.persist_hosted_setup_projection();
            if let Ok(failed) = self.inner.supervisor.report_failure(
                &self.inner.cfg.connector_id,
                ReportFailureInput {
                    reason: err.to_string(),
                },
            ) {
                let _ = self.persist_connector(&failed);
                let mut payload = Map::new();
                payload.insert("kind".to_string(), Value::String(failed.kind.clone()));
                payload.insert(
                    "status".to_string(),
                    Value::String(failed.status.as_str().to_string()),
                );
                payload.insert(
                    "deliveryMode".to_string(),
                    Value::String(self.inner.cfg.delivery_mode.clone()),
                );
                payload.insert("error".to_string(), Value::String(err.to_string()));
                payload.insert(
                    "errorClass".to_string(),
                    Value::String(classify_discord_error(&err)),
                );
                let _ = self.publish_event("connector.failed", payload);
            }
            return Err(err);
        }

        self.persist_hosted_setup_projection()?;

        let connector = self
            .inner
            .supervisor
            .report_health(
                &self.inner.cfg.connector_id,
                ReportHealthInput {
                    status: Status::Healthy,
                },
            )
            .map_err(|err| DiscordError::Other(err.to_string()))?;
        self.persist_connector(&connector)?;
        let mut payload = Map::new();
        payload.insert("kind".to_string(), Value::String(connector.kind.clone()));
        payload.insert(
            "status".to_string(),
            Value::String(connector.status.as_str().to_string()),
        );
        payload.insert(
            "deliveryMode".to_string(),
            Value::String(self.inner.cfg.delivery_mode.clone()),
        );
        let _ = self.publish_event("connector.healthy", payload);
        Ok(())
    }

    /// Go `Close(ctx)`.
    pub fn close(&self) -> Result<(), crate::DiscordError> {
        let mut started = self.inner.started.lock();
        if !*started {
            return Ok(());
        }
        *started = false;
        drop(started);
        self.inner.transport.close()
    }

    /// Drains every queued inbound/lifecycle event without blocking. Test and
    /// tick-friendly counterpart of `run_inbound`.
    pub fn drain_pending(&self) {
        let events: Vec<RuntimeEvent> = {
            let mut guard = self.inner.inbound_rx.lock();
            let mut out = Vec::new();
            if let Some(rx) = guard.as_mut() {
                while let Ok(event) = rx.try_recv() {
                    out.push(event);
                }
            }
            out
        };
        for event in events {
            self.handle_event(event);
        }
    }

    /// Blocks processing inbound and lifecycle events until the channel closes
    /// (all senders dropped — the transport closed). Must run on the thread
    /// that owns the runtime.
    pub fn run_inbound(&self) {
        loop {
            // The guard is a temporary: it is dropped before handle_event runs.
            let event = self
                .inner
                .inbound_rx
                .lock()
                .as_ref()
                .and_then(|rx| rx.recv().ok());
            match event {
                Some(event) => self.handle_event(event),
                None => return,
            }
        }
    }

    fn handle_event(&self, event: RuntimeEvent) {
        match event {
            RuntimeEvent::Inbound(inbound) => self.handle_inbound(inbound),
            RuntimeEvent::Lifecycle(event) => self.handle_lifecycle_event(event),
        }
    }

    /// Go's lifecycle observer body: persist the diagnostic and report the
    /// degraded health state. Runs on the runtime's owning thread.
    fn handle_lifecycle_event(&self, event: TransportLifecycleEvent) {
        if let Some(reason) = event.reason_code {
            let _ = self.persist_diagnostic(reason, event.evidence.clone());
        }
        if event.degraded {
            if let Ok(degraded) = self.inner.supervisor.report_health(
                &self.inner.cfg.connector_id,
                ReportHealthInput {
                    status: Status::Degraded,
                },
            ) {
                let _ = self.persist_connector(&degraded);
            }
        }
    }

    fn handle_inbound(&self, mut inbound: InboundMessage) {
        let reason = self.normalize_inbound_identity(&mut inbound);
        if !reason.is_empty() {
            self.publish_route_outcome(&inbound, "blocked", &reason);
            let _ = self.persist_diagnostic_for_inbound(
                &inbound,
                DiagnosticReasonCode::BlockedRoute,
                HashMap::from([
                    ("reason".to_string(), reason.clone()),
                    ("stage".to_string(), "identity_binding".to_string()),
                ]),
            );
            return;
        }
        if !self.should_handle(&inbound) {
            let reason = discord_route_reason(&self.inner.cfg, &inbound);
            let outcome = discord_route_outcome(&self.inner.cfg, &inbound);
            self.publish_route_outcome(&inbound, outcome, &reason);
            if outcome == "blocked" {
                let _ = self.persist_diagnostic_for_inbound(
                    &inbound,
                    DiagnosticReasonCode::BlockedRoute,
                    HashMap::from([
                        ("reason".to_string(), reason),
                        ("stage".to_string(), "route_gating".to_string()),
                    ]),
                );
            }
            return;
        }
        if let Ok(connector) = self.inner.supervisor.report_health(
            &self.inner.cfg.connector_id,
            ReportHealthInput {
                status: Status::Healthy,
            },
        ) {
            let _ = self.persist_connector(&connector);
        }

        if let Some(logger) = &self.inner.logger {
            logger.info(&format!(
                "discord inbound message accepted connector_id={} message_id={}",
                self.inner.cfg.connector_id, inbound.external_message_id
            ));
        }

        let now = Utc::now();
        let connector = Connector {
            connector_id: self.inner.cfg.connector_id.clone(),
            kind: "discord".to_string(),
            display_name: self.inner.cfg.display_name.clone(),
            status: Status::Healthy,
            created_at: now,
            updated_at: now,
            ..Connector::default()
        };
        let cancel = CancellationToken::new();
        match self.inner.message_loop.process_single_turn(
            &connector,
            &inbound,
            self.inner.transport.as_ref(),
            &cancel,
        ) {
            Err(err) => {
                if let Some(logger) = &self.inner.logger {
                    logger.error(&format!(
                        "discord message loop failed connector_id={} message_id={} error={}",
                        self.inner.cfg.connector_id, inbound.external_message_id, err
                    ));
                }
                let _ = self.persist_diagnostic_for_inbound(
                    &inbound,
                    DiagnosticReasonCode::ReplyFailed,
                    HashMap::from([
                        (
                            "errorClass".to_string(),
                            classify_discord_error_message(&err),
                        ),
                        ("stage".to_string(), "message_loop".to_string()),
                    ]),
                );
            }
            Ok(result) => {
                if result.duplicate {
                    let _ = self.persist_diagnostic_for_inbound(
                        &inbound,
                        DiagnosticReasonCode::DuplicateInbound,
                        HashMap::from([("stage".to_string(), "durable_dedupe".to_string())]),
                    );
                    if let Some(logger) = &self.inner.logger {
                        logger.info(&format!(
                            "discord duplicate message ignored connector_id={} message_id={}",
                            self.inner.cfg.connector_id, inbound.external_message_id
                        ));
                    }
                }
            }
        }
    }

    /// Go `normalizeInboundIdentity`: binds tenant/connector/identity fields,
    /// returning the blocking reason when durable identity is missing.
    fn normalize_inbound_identity(&self, inbound: &mut InboundMessage) -> String {
        inbound.tenant_id = first_non_empty(&[
            inbound.tenant_id.as_str(),
            self.runtime_tenant_id().as_str(),
        ]);
        inbound.connector_id = first_non_empty(&[
            inbound.connector_id.as_str(),
            self.inner.cfg.connector_id.as_str(),
        ]);
        inbound.connector_kind = first_non_empty(&[inbound.connector_kind.as_str(), "discord"]);
        inbound.connector_account_id = inbound_connector_account_id(inbound);
        inbound.account_id = first_non_empty(&[
            inbound.account_id.as_str(),
            inbound.connector_account_id.as_str(),
        ]);
        inbound.channel_or_conversation_id = inbound_channel_or_conversation_id(inbound);
        inbound.provider_message_id = inbound_provider_message_id(inbound);
        if inbound.equivalent_rule_id.is_empty() {
            inbound.equivalent_rule_id = "discord_message_id".to_string();
        }
        if inbound.tenant_id.trim().is_empty() {
            return "tenant_binding_missing".to_string();
        }
        if inbound.connector_account_id.trim().is_empty()
            || inbound.channel_or_conversation_id.trim().is_empty()
            || inbound.provider_message_id.trim().is_empty()
        {
            return "missing_durable_identity".to_string();
        }
        String::new()
    }

    fn publish_route_outcome(&self, inbound: &InboundMessage, outcome: &str, reason: &str) {
        let mut payload = Map::new();
        payload.insert(
            "tenantId".to_string(),
            Value::String(inbound.tenant_id.clone()),
        );
        payload.insert(
            "connectorId".to_string(),
            Value::String(self.inner.cfg.connector_id.clone()),
        );
        payload.insert("outcome".to_string(), Value::String(outcome.to_string()));
        payload.insert("reasonCode".to_string(), Value::String(reason.to_string()));
        payload.insert(
            "surface".to_string(),
            Value::String(discord_route_surface(inbound).to_string()),
        );
        payload.insert(
            "connectorAccountId".to_string(),
            Value::String(inbound_connector_account_id(inbound)),
        );
        payload.insert(
            "channelOrConversationId".to_string(),
            Value::String(inbound_channel_or_conversation_id(inbound)),
        );
        payload.insert(
            "providerMessageId".to_string(),
            Value::String(inbound_provider_message_id(inbound)),
        );
        payload.insert(
            "equivalentRuleId".to_string(),
            Value::String(inbound.equivalent_rule_id.clone()),
        );
        payload.insert(
            "redactionStatus".to_string(),
            Value::String("redacted".to_string()),
        );
        let _ = self.publish_event("connector.route_outcome_recorded", payload);
    }

    fn should_handle(&self, inbound: &InboundMessage) -> bool {
        let cfg = &self.inner.cfg;
        if inbound.direct {
            return cfg.respond_in_dm;
        }
        if !cfg.allowed_guild_ids.is_empty() && !contains(&cfg.allowed_guild_ids, &inbound.guild_id)
        {
            return false;
        }
        if !cfg.allowed_channel_ids.is_empty()
            && !contains(&cfg.allowed_channel_ids, &inbound.channel_id)
        {
            return false;
        }
        if cfg.require_mention && !inbound.mentioned {
            return false;
        }
        true
    }

    fn persist_connector(&self, connector: &Connector) -> Result<(), crate::DiscordError> {
        if let Some(store) = &self.inner.store {
            store
                .upsert_connector(connector)
                .map_err(|err| crate::DiscordError::Other(err))?;
        }
        Ok(())
    }

    fn publish_event(
        &self,
        name: &str,
        payload: Map<String, Value>,
    ) -> Result<Event, crate::DiscordError> {
        let Some(bus) = &self.inner.event_bus else {
            return Ok(Event::default());
        };
        let mut event = Event {
            category: "connector".to_string(),
            name: name.to_string(),
            scope: Scope {
                connector_id: self.inner.cfg.connector_id.clone(),
                ..Scope::default()
            },
            resource: Resource {
                kind: "connector".to_string(),
                id: self.inner.cfg.connector_id.clone(),
            },
            payload,
            ..Event::default()
        };
        if let Some(store) = &self.inner.store {
            event = store
                .append_event(&event)
                .map_err(|err| crate::DiscordError::Other(err))?;
        }
        Ok(bus.publish(event))
    }

    fn persist_diagnostic(
        &self,
        reason: DiagnosticReasonCode,
        evidence: HashMap<String, String>,
    ) -> Result<(), crate::DiscordError> {
        self.persist_diagnostic_for_inbound(&InboundMessage::default(), reason, evidence)
    }

    fn persist_diagnostic_for_inbound(
        &self,
        inbound: &InboundMessage,
        reason: DiagnosticReasonCode,
        evidence: HashMap<String, String>,
    ) -> Result<(), crate::DiscordError> {
        let Some(store) = &self.inner.store else {
            return Ok(());
        };
        let state = build_diagnostic_state(
            &first_non_empty(&[
                inbound.tenant_id.as_str(),
                self.runtime_tenant_id().as_str(),
            ]),
            &self.inner.cfg.connector_id,
            &inbound_connector_account_id(inbound),
            reason,
            evidence,
            Utc::now(),
        )?;
        store
            .save_connector_diagnostic_state(&state)
            .map_err(|err| crate::DiscordError::Other(err))?;
        Ok(())
    }

    fn persist_hosted_setup_projection(&self) -> Result<(), crate::DiscordError> {
        let Some(store) = &self.inner.store else {
            return Ok(());
        };
        let now = Utc::now();
        let setup = evaluate_hosted_setup(crate::readiness::HostedSetupInput {
            tenant_id: self.runtime_tenant_id(),
            connector_id: self.inner.cfg.connector_id.clone(),
            display_name: self.inner.cfg.display_name.clone(),
            credential: credential_state_for_config(&self.inner.cfg),
            respond_in_dm: self.inner.cfg.respond_in_dm,
            require_mention: self.inner.cfg.require_mention,
            delivery_mode: self.inner.cfg.delivery_mode.clone(),
            destinations: self.destination_validation_evidence(now),
            validated_at: now,
        });
        let record = DiscordHostedSetupRecord {
            tenant_id: setup.tenant_id.clone(),
            connector_id: setup.connector_id.clone(),
            connector_kind: setup.connector_kind.clone(),
            display_name: setup.display_name.clone(),
            status: setup.status.as_str().to_string(),
            readiness_state: setup.readiness_state.as_str().to_string(),
            hosted_ready: setup.hosted_ready,
            credential_state: setup.credential_state.as_str().to_string(),
            respond_in_dm: setup.respond_in_dm,
            require_mention: setup.require_mention,
            delivery_mode: setup.delivery_mode.clone(),
            reason_code: setup.reason_code.clone(),
            redaction_status: setup.redaction_status.as_str().to_string(),
            created_at: setup.created_at,
            updated_at: setup.updated_at,
            validated_at: Some(setup.validated_at),
            retention_expires_at: setup.retention_expires_at,
            destinations: Vec::new(),
        };
        store
            .save_discord_hosted_setup(&record)
            .map_err(|err| crate::DiscordError::Other(err))?;
        for destination in &setup.destinations {
            store
                .save_discord_destination_validation(&DiscordDestinationValidationRecord {
                    tenant_id: destination.tenant_id.clone(),
                    connector_id: destination.connector_id.clone(),
                    destination_id: destination.destination_id.clone(),
                    destination_type: destination.destination_type.as_str().to_string(),
                    provider_label: destination.provider_label.clone(),
                    selected: destination.selected,
                    validation_state: destination.validation_state.as_str().to_string(),
                    reason_code: destination.reason_code.clone(),
                    validated_at: destination.validated_at,
                    redaction_status: destination.redaction_status.as_str().to_string(),
                    safe_evidence: destination.safe_evidence.clone(),
                })
                .map_err(|err| crate::DiscordError::Other(err))?;
        }
        self.persist_conformance_evidence(&setup, now)?;

        let mut payload = Map::new();
        payload.insert(
            "tenantId".to_string(),
            Value::String(setup.tenant_id.clone()),
        );
        payload.insert(
            "connectorId".to_string(),
            Value::String(setup.connector_id.clone()),
        );
        payload.insert(
            "readinessState".to_string(),
            Value::String(setup.readiness_state.as_str().to_string()),
        );
        payload.insert("hostedReady".to_string(), Value::Bool(setup.hosted_ready));
        payload.insert(
            "credentialState".to_string(),
            Value::String(setup.credential_state.as_str().to_string()),
        );
        payload.insert(
            "reasonCode".to_string(),
            Value::String(setup.reason_code.clone()),
        );
        payload.insert(
            "redactionStatus".to_string(),
            Value::String(setup.redaction_status.as_str().to_string()),
        );
        payload.insert(
            "validatedAt".to_string(),
            Value::String(setup.validated_at.to_rfc3339()),
        );

        let event = Event {
            category: "connector".to_string(),
            name: "connector.discord_setup_validated".to_string(),
            scope: Scope {
                connector_id: setup.connector_id.clone(),
                ..Scope::default()
            },
            resource: Resource {
                kind: "discord_hosted_setup".to_string(),
                id: setup.connector_id.clone(),
            },
            payload,
            ..Event::default()
        };
        let persisted = store
            .append_event(&event)
            .map_err(|err| crate::DiscordError::Other(err))?;
        if let Some(bus) = &self.inner.event_bus {
            bus.publish(persisted);
        }
        Ok(())
    }

    fn persist_conformance_evidence(
        &self,
        setup: &HostedSetup,
        now: DateTime<Utc>,
    ) -> Result<(), crate::DiscordError> {
        let Some(store) = &self.inner.store else {
            return Ok(());
        };
        let profile = conformance_profile_for_setup(&self.inner.cfg, setup, now);
        let (results, _) = kura_connectors::run_matrix_case(kura_connectors::MatrixCase {
            scenario_id: format!("discord_hosted_setup_{}", self.inner.cfg.connector_id),
            tenant_id: setup.tenant_id.clone(),
            connector_kind: "discord".to_string(),
            connector_id: self.inner.cfg.connector_id.clone(),
            core_invariant_results: profile.core_invariant_results.clone(),
            provider_surface_results: profile.provider_surface_results.clone(),
            equivalent_durable_identity_rule_id: profile
                .equivalent_durable_identity_rule_id
                .clone(),
            equivalent_durable_identity_rule: profile.equivalent_durable_identity_rule.clone(),
            redaction_status: kura_connectors::RedactionStatus::Redacted,
            now,
            ..kura_connectors::MatrixCase::default()
        })
        .map_err(|err| crate::DiscordError::Other(err.to_string()))?;
        for result in results {
            store
                .save_connector_conformance_result(&result)
                .map_err(|err| crate::DiscordError::Other(err))?;
        }
        Ok(())
    }

    fn destination_validation_evidence(&self, now: DateTime<Utc>) -> Vec<DestinationValidation> {
        let mut destinations = destination_evidence_for_config(&self.inner.cfg, now);
        let Some(validator) = self.inner.transport.destination_validator() else {
            return destinations;
        };
        if destinations.is_empty() {
            return destinations;
        }
        match validator.validate_destinations(destinations.clone()) {
            Err(err) => {
                let _ = self.persist_diagnostic(
                    diagnostic_reason_for_error(&err),
                    HashMap::from([("stage".to_string(), "destination_validation".to_string())]),
                );
                for destination in &mut destinations {
                    destination.validation_state = DestinationValidationState::Invalid;
                    destination.reason_code = DiagnosticReasonCode::UnknownConnectorFailure
                        .as_str()
                        .to_string();
                    destination.safe_evidence = HashMap::from([
                        ("source".to_string(), "transport_validation".to_string()),
                        ("errorClass".to_string(), classify_discord_error(&err)),
                    ]);
                }
                destinations
            }
            Ok(validated) => {
                if validated.is_empty() {
                    destinations
                } else {
                    validated
                }
            }
        }
    }

    /// Go `runtimeTenantID`: the explicit config tenant binding first, then
    /// the store's default personal tenant.
    fn runtime_tenant_id(&self) -> String {
        let cfg_tenant = self.inner.cfg.tenant_id.trim().to_string();
        if !cfg_tenant.is_empty() {
            return cfg_tenant;
        }
        if let Some(store) = &self.inner.store {
            if let Ok(tenant_id) = store.resolve_default_personal_tenant_id() {
                let trimmed = tenant_id.trim().to_string();
                if !trimmed.is_empty() {
                    return trimmed;
                }
            }
        }
        String::new()
    }
}

/// Go `discordRouteOutcome`.
#[must_use]
fn discord_route_outcome(cfg: &Config, inbound: &InboundMessage) -> &'static str {
    if !inbound.direct && cfg.require_mention && !inbound.mentioned {
        "ignored"
    } else {
        "blocked"
    }
}

/// Go `discordRouteReason`. Switch order matches Go exactly.
#[must_use]
fn discord_route_reason(cfg: &Config, inbound: &InboundMessage) -> String {
    if inbound.direct && !cfg.respond_in_dm {
        "direct_message_disabled".to_string()
    } else if !cfg.allowed_guild_ids.is_empty()
        && !contains(&cfg.allowed_guild_ids, &inbound.guild_id)
    {
        "blocked_guild".to_string()
    } else if !cfg.allowed_channel_ids.is_empty()
        && !contains(&cfg.allowed_channel_ids, &inbound.channel_id)
    {
        "blocked_channel".to_string()
    } else if !inbound.direct && cfg.require_mention && !inbound.mentioned {
        "mention_required".to_string()
    } else {
        "blocked_route".to_string()
    }
}

/// Go `discordRouteSurface`.
#[must_use]
fn discord_route_surface(inbound: &InboundMessage) -> &'static str {
    if inbound.direct {
        "direct_message"
    } else if !inbound.thread_id.trim().is_empty() && inbound.thread_id != inbound.channel_id {
        "thread_reply"
    } else {
        "group_channel"
    }
}

/// Go `inboundConnectorAccountID`.
#[must_use]
fn inbound_connector_account_id(inbound: &InboundMessage) -> String {
    first_non_empty(&[
        inbound.connector_account_id.as_str(),
        inbound.account_id.as_str(),
    ])
}

/// Go `inboundChannelOrConversationID`.
#[must_use]
fn inbound_channel_or_conversation_id(inbound: &InboundMessage) -> String {
    first_non_empty(&[
        inbound.channel_or_conversation_id.as_str(),
        inbound.channel_id.as_str(),
        inbound.peer_id.as_str(),
    ])
}

/// Go `inboundProviderMessageID`.
#[must_use]
fn inbound_provider_message_id(inbound: &InboundMessage) -> String {
    first_non_empty(&[
        inbound.provider_message_id.as_str(),
        inbound.external_message_id.as_str(),
    ])
}

/// Go `firstNonEmpty`.
#[must_use]
fn first_non_empty(values: &[&str]) -> String {
    for value in values {
        let trimmed = value.trim();
        if !trimmed.is_empty() {
            return trimmed.to_string();
        }
    }
    String::new()
}

/// Go `contains`: case-preserving trimmed membership.
#[must_use]
fn contains(items: &[String], target: &str) -> bool {
    items.iter().any(|item| item.trim() == target.trim())
}
