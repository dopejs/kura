//! Setup service (port of `service.go` + `submitted_secret.go` + `oauth.go` +
//! `recovery.go` + `dependent_use.go`): the setup session state machine and its
//! dependencies.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use chrono::{DateTime, Utc};
use kura_identity::TenantContext;
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};

use crate::catalog::{catalog_targets, target_by_id};
use crate::helpers::*;
use crate::permissions::{require_inspection, require_mutation};
use crate::probe::{
    default_diagnostic_result_id, default_diagnostic_run_id, default_diagnostic_source_kind,
    diagnostic_for_session, diagnostic_stage_for_operation,
};
use crate::types::*;

pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

// ---------------------------------------------------------------------------
// Dependency traits
// ---------------------------------------------------------------------------

pub trait Store: Send + Sync {
    fn save_setup_session(&self, session: SetupSession) -> BoxFuture<'_, Result<(), SetupError>>;
    fn get_setup_session(
        &self,
        tenant_id: &str,
        session_id: &str,
    ) -> BoxFuture<'_, Result<Option<SetupSession>, SetupError>>;
    fn list_setup_sessions(
        &self,
        tenant_id: &str,
    ) -> BoxFuture<'_, Result<Vec<SetupSession>, SetupError>>;
    fn append_setup_attempt(&self, attempt: SetupAttempt) -> BoxFuture<'_, Result<(), SetupError>>;
    fn list_setup_attempts(
        &self,
        tenant_id: &str,
        session_id: &str,
    ) -> BoxFuture<'_, Result<Vec<SetupAttempt>, SetupError>>;
}

pub trait SecretManager: Send + Sync {
    fn create(
        &self,
        input: kura_secrets::CreateInput,
    ) -> BoxFuture<'_, Result<kura_secrets::TenantSecret, SetupError>>;
    fn rotate(
        &self,
        input: kura_secrets::RotateInput,
    ) -> BoxFuture<'_, Result<kura_secrets::TenantSecret, SetupError>>;
    fn get(
        &self,
        tenant_id: &str,
        secret_ref: &str,
    ) -> BoxFuture<'_, Result<kura_secrets::TenantSecret, SetupError>>;
    fn disable(
        &self,
        input: kura_secrets::DisableInput,
    ) -> BoxFuture<'_, Result<kura_secrets::TenantSecret, SetupError>>;
}

impl SecretManager for kura_secrets::Manager {
    fn create(
        &self,
        input: kura_secrets::CreateInput,
    ) -> BoxFuture<'_, Result<kura_secrets::TenantSecret, SetupError>> {
        Box::pin(async move { Ok(self.create(input).await?) })
    }
    fn rotate(
        &self,
        input: kura_secrets::RotateInput,
    ) -> BoxFuture<'_, Result<kura_secrets::TenantSecret, SetupError>> {
        Box::pin(async move { Ok(self.rotate(input).await?) })
    }
    fn get(
        &self,
        tenant_id: &str,
        secret_ref: &str,
    ) -> BoxFuture<'_, Result<kura_secrets::TenantSecret, SetupError>> {
        let tenant_id = tenant_id.to_string();
        let secret_ref = secret_ref.to_string();
        Box::pin(async move { Ok(self.get(&tenant_id, &secret_ref).await?) })
    }
    fn disable(
        &self,
        input: kura_secrets::DisableInput,
    ) -> BoxFuture<'_, Result<kura_secrets::TenantSecret, SetupError>> {
        Box::pin(async move { Ok(self.disable(input).await?) })
    }
}

pub trait DiagnosticProbe: Send + Sync {
    fn probe_setup(
        &self,
        session: &SetupSession,
        operation: SetupOperation,
    ) -> BoxFuture<'_, Result<SetupDiagnosticProbeResult, SetupError>>;
}

pub trait AuditRecorder: Send + Sync {
    fn record_setup_audit(
        &self,
        record: SetupAuditRecord,
    ) -> BoxFuture<'_, Result<String, SetupError>>;
}

pub trait SubmittedSecretRecorder: Send + Sync {
    fn record_submitted_secret_setup(
        &self,
        session: SetupSession,
        input: SubmitSecretInput,
    ) -> BoxFuture<'_, Result<(), SetupError>>;
}

pub trait OAuthCallbackRecorder: Send + Sync {
    fn record_oauth_setup(
        &self,
        session: SetupSession,
        input: OAuthCallbackInput,
    ) -> BoxFuture<'_, Result<(), SetupError>>;
}

pub trait OAuthStartURLProvider: Send + Sync {
    fn authorization_url(
        &self,
        session: SetupSession,
        input: OAuthStartInput,
        default_url: String,
    ) -> BoxFuture<'_, Result<String, SetupError>>;
}

// ---------------------------------------------------------------------------
// Input / output types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct StartInput {
    pub tenant_context: TenantContext,
    pub target_id: String,
    pub setup_style: SetupStyle,
    pub source: String,
}

#[derive(Debug, Clone)]
pub struct SubmitSecretInput {
    pub tenant_context: TenantContext,
    pub session_id: String,
    pub secret_ref: String,
    pub value: String,
    pub display_name: String,
    pub resource_refs: Vec<ResourceRef>,
}

#[derive(Debug, Clone)]
pub struct OAuthStartInput {
    pub tenant_context: TenantContext,
    pub session_id: String,
    pub redirect_route: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OAuthStartResult {
    pub session: SetupSession,
    pub authorization_url: String,
    pub state_ref: String,
}

#[derive(Debug, Clone)]
pub struct OAuthCallbackInput {
    pub tenant_context: TenantContext,
    pub session_id: String,
    pub state: String,
    pub result: OAuthResult,
    pub account_label: String,
    pub code: String,
    pub redirect_uri: String,
}

#[derive(Debug, Clone)]
pub struct ReplaceInput {
    pub tenant_context: TenantContext,
    pub session_id: String,
}

#[derive(Debug, Clone)]
pub struct DisableInput {
    pub tenant_context: TenantContext,
    pub session_id: String,
    pub disabled_reason: String,
}

// ---------------------------------------------------------------------------
// Service
// ---------------------------------------------------------------------------

type Clock = Arc<dyn Fn() -> DateTime<Utc> + Send + Sync>;

pub struct ServiceDependencies {
    pub store: Option<Arc<dyn Store>>,
    pub secrets: Option<Arc<dyn SecretManager>>,
    pub diagnostics: Option<Arc<dyn DiagnosticProbe>>,
    pub audit: Option<Arc<dyn AuditRecorder>>,
    pub submitted_secret_recorder: Option<Arc<dyn SubmittedSecretRecorder>>,
    pub oauth_callback_recorder: Option<Arc<dyn OAuthCallbackRecorder>>,
    pub oauth_start_url_provider: Option<Arc<dyn OAuthStartURLProvider>>,
    pub now: Option<Clock>,
}

impl Default for ServiceDependencies {
    fn default() -> Self {
        ServiceDependencies {
            store: None,
            secrets: None,
            diagnostics: None,
            audit: None,
            submitted_secret_recorder: None,
            oauth_callback_recorder: None,
            oauth_start_url_provider: None,
            now: None,
        }
    }
}

pub struct Service {
    store: Arc<dyn Store>,
    secrets: Option<Arc<dyn SecretManager>>,
    diagnostics: Option<Arc<dyn DiagnosticProbe>>,
    audit: Option<Arc<dyn AuditRecorder>>,
    submitted_secret_recorder: Option<Arc<dyn SubmittedSecretRecorder>>,
    oauth_callback_recorder: Option<Arc<dyn OAuthCallbackRecorder>>,
    oauth_start_url_provider: Option<Arc<dyn OAuthStartURLProvider>>,
    now: Clock,
}

#[must_use]
pub fn new_service(deps: ServiceDependencies) -> Service {
    let now = deps.now.unwrap_or_else(|| Arc::new(Utc::now));
    let store = deps
        .store
        .unwrap_or_else(|| Arc::new(MemoryStore::default()));
    let diagnostics = deps.diagnostics.or_else(|| {
        deps.secrets.as_ref().map(|secrets| {
            Arc::new(crate::probe::DefaultDiagnosticProbe::new(Some(
                secrets.clone(),
            ))) as Arc<dyn DiagnosticProbe>
        })
    });
    Service {
        store,
        secrets: deps.secrets,
        diagnostics,
        audit: deps.audit,
        submitted_secret_recorder: deps.submitted_secret_recorder,
        oauth_callback_recorder: deps.oauth_callback_recorder,
        oauth_start_url_provider: deps.oauth_start_url_provider,
        now,
    }
}

impl Service {
    fn now(&self) -> DateTime<Utc> {
        (self.now)()
    }

    pub async fn list_targets(
        &self,
        tenant_context: &TenantContext,
    ) -> Result<Vec<SetupTarget>, SetupError> {
        require_inspection(tenant_context)?;
        let mut targets = catalog_targets(&tenant_context.tenant_id);
        let sessions = self
            .store
            .list_setup_sessions(&tenant_context.tenant_id)
            .await?;
        let by_target: std::collections::HashMap<String, SetupSession> = sessions
            .into_iter()
            .map(|s| (s.target_id.clone(), s))
            .collect();
        for target in &mut targets {
            if let Some(session) = by_target.get(&target.target_id) {
                target.current_session_id = session.setup_session_id.clone();
                target.current_state = session.state;
                target.diagnostic_result_id = session.diagnostic_result_id.clone();
            }
        }
        Ok(targets)
    }

    pub async fn list_sessions(
        &self,
        tenant_context: &TenantContext,
    ) -> Result<Vec<SetupSession>, SetupError> {
        require_inspection(tenant_context)?;
        self.store
            .list_setup_sessions(&tenant_context.tenant_id)
            .await
    }

    pub async fn get(
        &self,
        tenant_context: &TenantContext,
        session_id: &str,
    ) -> Result<SetupSession, SetupError> {
        require_inspection(tenant_context)?;
        let session = self
            .store
            .get_setup_session(&tenant_context.tenant_id, session_id.trim())
            .await?;
        session.ok_or(SetupError::SessionNotFound)
    }

    pub async fn start(&self, input: StartInput) -> Result<SetupSession, SetupError> {
        require_mutation(&input.tenant_context)?;
        let (target, ok) = target_by_id(&input.tenant_context.tenant_id, &input.target_id);
        if !ok || target.support_status != SupportStatus::Supported {
            return Err(SetupError::UnsupportedTarget);
        }
        if input.setup_style != SetupStyle::Unsupported && input.setup_style != target.setup_style {
            return Err(SetupError::StyleMismatch(
                input.setup_style.as_str().to_string(),
                target.setup_style.as_str().to_string(),
            ));
        }
        let now = self.now();
        let mut session = SetupSession {
            setup_session_id: session_id(
                &input.tenant_context.tenant_id,
                &target.target_id,
                target.setup_style,
            ),
            tenant_id: input.tenant_context.tenant_id.clone(),
            actor_principal_id: input.tenant_context.principal_id.clone(),
            target_id: target.target_id.clone(),
            target_kind: target.target_kind,
            setup_style: target.setup_style,
            state: SetupState::InProgress,
            reason_code: String::new(),
            retryable: true,
            remediation_owner: RemediationOwner::ProductUser,
            safe_use_mode: SafeUseMode::Blocked,
            allowed_capabilities: Vec::new(),
            current_attempt_id: String::new(),
            diagnostic_result_id: String::new(),
            diagnostic_run_id: String::new(),
            diagnostic_stage: String::new(),
            diagnostic_source_kind: String::new(),
            diagnostic_source_id: String::new(),
            diagnostic_allowed_use: Vec::new(),
            redaction_status: RedactionStatus::Redacted,
            resource_refs: Vec::new(),
            redacted_evidence: std::collections::HashMap::new(),
            oauth_state_ref: String::new(),
            created_at: now,
            updated_at: now,
            last_transition_at: now,
            last_transition_audit_id: String::new(),
            operator_remediation: String::new(),
            user_remediation: String::new(),
            unsupported_reason_code: String::new(),
        };
        if let Some(existing) = self
            .store
            .get_setup_session(&session.tenant_id, &session.setup_session_id)
            .await?
        {
            session.created_at = existing.created_at;
            session.resource_refs = existing.resource_refs;
        }
        self.transition(
            session,
            SetupOperation::Start,
            SetupState::InProgress,
            "",
            None,
        )
        .await
    }

    #[allow(clippy::too_many_arguments)]
    async fn transition(
        &self,
        mut session: SetupSession,
        op: SetupOperation,
        to: SetupState,
        reason: &str,
        evidence: Option<std::collections::HashMap<String, String>>,
    ) -> Result<SetupSession, SetupError> {
        let from = session.state;
        let now = self.now();
        session.state = to;
        session.reason_code = reason.to_string();
        session.redacted_evidence = evidence.unwrap_or_default();
        session.updated_at = now;
        session.last_transition_at = now;
        session.redaction_status = first_redaction(session.redaction_status);
        session.safe_use_mode = safe_use_for_state(&session);
        session.retryable = retryable_for_state(to);
        session.remediation_owner = remediation_owner_for_state(to, reason);
        if to == SetupState::Ready {
            session.reason_code = REASON_HEALTHY.to_string();
        }
        if to == SetupState::Ready && session.diagnostic_result_id.is_empty() {
            return Err(SetupError::DiagnosticLinkNeeded);
        }
        let mut attempt = SetupAttempt {
            attempt_id: attempt_id(&session.setup_session_id, op, now),
            setup_session_id: session.setup_session_id.clone(),
            tenant_id: session.tenant_id.clone(),
            actor_principal_id: session.actor_principal_id.clone(),
            operation: op,
            from_state: from,
            to_state: to,
            reason_code: session.reason_code.clone(),
            redacted_evidence: session.redacted_evidence.clone(),
            resource_refs: session.resource_refs.clone(),
            redaction_status: session.redaction_status,
            diagnostic_result_id: session.diagnostic_result_id.clone(),
            created_at: now,
        };
        session.current_attempt_id = attempt.attempt_id.clone();
        if contains_forbidden_evidence(&session, &[]) || contains_forbidden_evidence(&attempt, &[])
        {
            session = fail_closed(session, REASON_REDACTION_FAILED_CLOSED);
            attempt.to_state = session.state;
            attempt.reason_code = session.reason_code.clone();
            attempt.redaction_status = session.redaction_status;
        }
        self.store.save_setup_session(session.clone()).await?;
        self.store.append_setup_attempt(attempt.clone()).await?;
        if let Some(audit) = &self.audit {
            let record = crate::audit::audit_record_for_attempt(&session, &attempt);
            let audit_id = audit.record_setup_audit(record).await?;
            if !audit_id.is_empty() {
                session.last_transition_audit_id = audit_id;
                self.store.save_setup_session(session.clone()).await?;
            }
        }
        Ok(session)
    }

    async fn load_for_mutation(
        &self,
        tc: &TenantContext,
        session_id_value: &str,
    ) -> Result<SetupSession, SetupError> {
        require_mutation(tc)?;
        if session_id_value.trim().is_empty() {
            return Err(SetupError::SessionRequired);
        }
        let mut session = self
            .store
            .get_setup_session(&tc.tenant_id, session_id_value.trim())
            .await?
            .ok_or(SetupError::SessionNotFound)?;
        session.actor_principal_id =
            first_non_empty(&[&tc.principal_id, &session.actor_principal_id]);
        Ok(session)
    }

    pub async fn submit_secret(
        &self,
        input: SubmitSecretInput,
    ) -> Result<SetupSession, SetupError> {
        let mut session = self
            .load_for_mutation(&input.tenant_context, &input.session_id)
            .await?;
        if session.setup_style != SetupStyle::SubmittedSecret {
            return Err(SetupError::UnsupportedTarget);
        }
        let secret_ref = input.secret_ref.trim();
        if secret_ref.is_empty() {
            return Err(SetupError::SecretRefRequired);
        }
        if input.value.trim().is_empty() {
            return Err(SetupError::SecretValueRequired);
        }
        let mut check = std::collections::HashMap::new();
        check.insert("displayName".to_string(), input.display_name.clone());
        check.insert("secretRef".to_string(), secret_ref.to_string());
        if contains_forbidden_evidence(&check, &[input.value.clone()]) {
            session = fail_closed(session, REASON_REDACTION_FAILED_CLOSED);
            let state = session.state;
            let reason = session.reason_code.clone();
            let evidence = session.redacted_evidence.clone();
            return self
                .transition(
                    session,
                    SetupOperation::SubmitSecret,
                    state,
                    &reason,
                    Some(evidence),
                )
                .await;
        }
        let mut version_id = "redacted_version".to_string();
        if let Some(secrets) = &self.secrets {
            let secret = match secrets.get(&session.tenant_id, secret_ref).await {
                Ok(_) => {
                    secrets
                        .rotate(kura_secrets::RotateInput {
                            tenant_id: session.tenant_id.clone(),
                            secret_ref: secret_ref.to_string(),
                            value: input.value.clone(),
                        })
                        .await?
                }
                Err(SetupError::Secrets(kura_secrets::SecretsError::SecretNotFound)) => {
                    secrets
                        .create(kura_secrets::CreateInput {
                            tenant_id: session.tenant_id.clone(),
                            secret_ref: secret_ref.to_string(),
                            display_name: input.display_name.clone(),
                            value: input.value.clone(),
                            document: None,
                        })
                        .await?
                }
                Err(e) => return Err(e),
            };
            version_id = secret.active_version_id;
        }
        upsert_resource_ref(
            &mut session.resource_refs,
            ResourceRef {
                kind: "tenant_secret".to_string(),
                id: secret_ref.to_string(),
                route: format!("/v1/tenant-secrets/{secret_ref}"),
            },
        );
        for reference in &input.resource_refs {
            if reference.kind.trim().is_empty() || reference.id.trim().is_empty() {
                continue;
            }
            upsert_resource_ref(&mut session.resource_refs, reference.clone());
        }
        let mut evidence = redacted_secret_evidence(secret_ref, &input.display_name);
        evidence.insert("secretVersionId".to_string(), version_id);
        session.redacted_evidence = evidence;
        let (session, state, reason) = self
            .probe_readiness(session, SetupOperation::SubmitSecret)
            .await?;
        let evidence = session.redacted_evidence.clone();
        let session = self
            .transition(
                session,
                SetupOperation::SubmitSecret,
                state,
                &reason,
                Some(evidence),
            )
            .await?;
        if let Some(recorder) = &self.submitted_secret_recorder {
            recorder
                .record_submitted_secret_setup(session.clone(), input)
                .await?;
        }
        Ok(session)
    }

    pub async fn start_oauth(
        &self,
        input: OAuthStartInput,
    ) -> Result<OAuthStartResult, SetupError> {
        let mut session = self
            .load_for_mutation(&input.tenant_context, &input.session_id)
            .await?;
        if session.setup_style != SetupStyle::OAuth {
            return Err(SetupError::UnsupportedTarget);
        }
        let state_ref = oauth_state_ref(&session);
        session.oauth_state_ref = state_ref.clone();
        let mut authorization_url = format!("https://oauth.test/authorize?state={state_ref}");
        if let Some(provider) = &self.oauth_start_url_provider {
            authorization_url = provider
                .authorization_url(session.clone(), input.clone(), authorization_url)
                .await?;
        }
        let mut evidence = std::collections::HashMap::new();
        evidence.insert(
            "redactionRule".to_string(),
            "oauth_start_metadata_only".to_string(),
        );
        evidence.insert(
            "redirectRoute".to_string(),
            input.redirect_route.trim().to_string(),
        );
        let updated = self
            .transition(
                session,
                SetupOperation::OAuthStart,
                SetupState::InProgress,
                "",
                Some(evidence),
            )
            .await?;
        Ok(OAuthStartResult {
            session: updated,
            authorization_url,
            state_ref,
        })
    }

    pub async fn complete_oauth(
        &self,
        mut input: OAuthCallbackInput,
    ) -> Result<SetupSession, SetupError> {
        let mut session = self
            .load_for_mutation(&input.tenant_context, &input.session_id)
            .await?;
        if session.setup_style != SetupStyle::OAuth {
            return Err(SetupError::UnsupportedTarget);
        }
        if input.state.trim().is_empty() {
            return Err(SetupError::OAuthStateRequired);
        }
        if !session.oauth_state_ref.is_empty() && input.state.trim() != session.oauth_state_ref {
            input.result = OAuthResult::TenantMismatch;
        }
        let (state, reason) = map_oauth_result(input.result);
        let evidence = redacted_oauth_evidence(input.result, &input.account_label);
        session.redacted_evidence = evidence.clone();
        upsert_resource_ref(
            &mut session.resource_refs,
            ResourceRef {
                kind: "provider_auth_state".to_string(),
                id: session.target_id.clone(),
                route: String::new(),
            },
        );
        let (session, state, reason) =
            if state == SetupState::Ready || state == SetupState::Degraded {
                self.probe_readiness(session, SetupOperation::OAuthCallback)
                    .await?
            } else {
                (session, state, reason)
            };
        let updated = self
            .transition(
                session,
                SetupOperation::OAuthCallback,
                state,
                &reason,
                Some(evidence),
            )
            .await?;
        if let Some(recorder) = &self.oauth_callback_recorder {
            recorder.record_oauth_setup(updated.clone(), input).await?;
        }
        Ok(updated)
    }

    pub async fn retry(&self, input: ReplaceInput) -> Result<SetupSession, SetupError> {
        let session = self
            .load_for_mutation(&input.tenant_context, &input.session_id)
            .await?;
        let mut evidence = std::collections::HashMap::new();
        evidence.insert(
            "redactionRule".to_string(),
            "retry_metadata_only".to_string(),
        );
        self.transition(
            session,
            SetupOperation::Retry,
            SetupState::InProgress,
            "",
            Some(evidence),
        )
        .await
    }

    pub async fn replace(&self, input: ReplaceInput) -> Result<SetupSession, SetupError> {
        let session = self
            .load_for_mutation(&input.tenant_context, &input.session_id)
            .await?;
        let mut evidence = std::collections::HashMap::new();
        evidence.insert(
            "redactionRule".to_string(),
            "replace_metadata_only".to_string(),
        );
        self.transition(
            session,
            SetupOperation::Replace,
            SetupState::InProgress,
            "",
            Some(evidence),
        )
        .await
    }

    pub async fn cancel(&self, input: ReplaceInput) -> Result<SetupSession, SetupError> {
        let session = self
            .load_for_mutation(&input.tenant_context, &input.session_id)
            .await?;
        let mut evidence = std::collections::HashMap::new();
        evidence.insert(
            "redactionRule".to_string(),
            "cancel_metadata_only".to_string(),
        );
        self.transition(
            session,
            SetupOperation::Cancel,
            SetupState::Cancelled,
            REASON_USER_CANCELLED,
            Some(evidence),
        )
        .await
    }

    pub async fn disable(&self, input: DisableInput) -> Result<SetupSession, SetupError> {
        let session = self
            .load_for_mutation(&input.tenant_context, &input.session_id)
            .await?;
        if let Some(secrets) = &self.secrets {
            if session.setup_style == SetupStyle::SubmittedSecret {
                for reference in &session.resource_refs {
                    if reference.kind == "tenant_secret" && !reference.id.is_empty() {
                        let _ = secrets
                            .disable(kura_secrets::DisableInput {
                                tenant_id: session.tenant_id.clone(),
                                secret_ref: reference.id.clone(),
                                disabled_reason: input.disabled_reason.clone(),
                            })
                            .await;
                    }
                }
            }
        }
        let mut evidence = std::collections::HashMap::new();
        evidence.insert(
            "redactionRule".to_string(),
            "disable_metadata_only".to_string(),
        );
        evidence.insert("disabledReason".to_string(), input.disabled_reason.clone());
        self.transition(
            session,
            SetupOperation::Disable,
            SetupState::Disabled,
            REASON_DISABLED_BY_USER,
            Some(evidence),
        )
        .await
    }

    #[must_use]
    pub fn dependent_use_decision(
        &self,
        session: &SetupSession,
        capability: &str,
    ) -> DependentUseDecision {
        let mut mode = SafeUseMode::Blocked;
        let mut allowed: Vec<String> = Vec::new();
        let mut reason = session.reason_code.clone();
        match session.state {
            SetupState::Ready => {
                mode = SafeUseMode::Normal;
                reason = String::new();
            }
            SetupState::Degraded => {
                if contains(&session.allowed_capabilities, capability)
                    && contains(&session.diagnostic_allowed_use, capability)
                {
                    mode = SafeUseMode::LimitedSafe;
                    allowed = vec![capability.to_string()];
                } else {
                    reason = first_non_empty(&[&reason, "degraded_capability_not_allowed"]);
                }
            }
            _ => {
                mode = SafeUseMode::Blocked;
            }
        }
        DependentUseDecision {
            tenant_id: session.tenant_id.clone(),
            target_id: session.target_id.clone(),
            setup_state: session.state,
            safe_use_mode: mode,
            allowed_capabilities: allowed,
            reason_code: reason,
            checked_at: self.now(),
        }
    }

    pub async fn diagnostics(
        &self,
        tenant_context: &TenantContext,
        session_id: &str,
    ) -> Result<Vec<SetupDiagnostic>, SetupError> {
        let session = self.get(tenant_context, session_id).await?;
        Ok(vec![diagnostic_for_session(&session, self.now())])
    }

    async fn probe_readiness(
        &self,
        session: SetupSession,
        operation: SetupOperation,
    ) -> Result<(SetupSession, SetupState, String), SetupError> {
        let Some(diagnostics) = &self.diagnostics else {
            return Err(SetupError::DiagnosticLinkNeeded);
        };
        let result = diagnostics.probe_setup(&session, operation).await?;
        Ok(self.apply_probe_result(session, operation, result))
    }

    fn apply_probe_result(
        &self,
        mut session: SetupSession,
        operation: SetupOperation,
        result: SetupDiagnosticProbeResult,
    ) -> (SetupSession, SetupState, String) {
        let result = normalize_probe_result(&session, operation, result);
        session.diagnostic_result_id = result.diagnostic_result_id.clone();
        session.diagnostic_run_id = result.diagnostic_run_id.clone();
        session.diagnostic_stage = result.diagnostic_stage.clone();
        session.diagnostic_source_kind = result.diagnostic_source.kind.clone();
        session.diagnostic_source_id = result.diagnostic_source.id.clone();
        session.diagnostic_allowed_use = result.allowed_capabilities.clone();
        session.allowed_capabilities = result.allowed_capabilities.clone();
        session.remediation_owner = result.remediation_owner;
        (session, result.state, result.reason_code)
    }
}

fn map_oauth_result(result: OAuthResult) -> (SetupState, String) {
    match result {
        OAuthResult::Completed => (SetupState::Ready, REASON_HEALTHY.to_string()),
        OAuthResult::Denied => (SetupState::ActionRequired, REASON_OAUTH_DENIED.to_string()),
        OAuthResult::Abandoned => (SetupState::Cancelled, REASON_OAUTH_ABANDONED.to_string()),
        OAuthResult::Expired => (SetupState::ActionRequired, REASON_OAUTH_EXPIRED.to_string()),
        OAuthResult::Replay => (SetupState::ActionRequired, REASON_OAUTH_REPLAY.to_string()),
        OAuthResult::TenantMismatch => (
            SetupState::ActionRequired,
            REASON_TENANT_MISMATCH.to_string(),
        ),
        OAuthResult::ProviderError => (
            SetupState::Unavailable,
            REASON_PROVIDER_UNAVAILABLE.to_string(),
        ),
    }
}

fn normalize_probe_result(
    session: &SetupSession,
    operation: SetupOperation,
    mut result: SetupDiagnosticProbeResult,
) -> SetupDiagnosticProbeResult {
    if result.reason_code.is_empty() {
        result.reason_code = if result.state == SetupState::Ready {
            REASON_HEALTHY.to_string()
        } else {
            REASON_PROVIDER_UNAVAILABLE.to_string()
        };
    }
    if result.diagnostic_result_id.is_empty() {
        result.diagnostic_result_id = default_diagnostic_result_id(session, operation);
    }
    if result.diagnostic_run_id.is_empty() {
        result.diagnostic_run_id = default_diagnostic_run_id(session, operation);
    }
    if result.diagnostic_stage.is_empty() {
        result.diagnostic_stage = diagnostic_stage_for_operation(operation);
    }
    if result.diagnostic_source.kind.is_empty() {
        result.diagnostic_source.kind = default_diagnostic_source_kind(session);
    }
    if result.diagnostic_source.id.is_empty() {
        result.diagnostic_source.id = session.target_id.clone();
    }
    result
}

// ---------------------------------------------------------------------------
// Memory store
// ---------------------------------------------------------------------------

#[derive(Default)]
struct MemoryInner {
    sessions: std::collections::HashMap<String, SetupSession>,
    attempts: std::collections::HashMap<String, Vec<SetupAttempt>>,
}

/// In-memory [`Store`] for tests and the no-store fallback.
#[derive(Default)]
pub struct MemoryStore {
    inner: RwLock<MemoryInner>,
}

impl Store for MemoryStore {
    fn save_setup_session(&self, session: SetupSession) -> BoxFuture<'_, Result<(), SetupError>> {
        let key = format!("{}::{}", session.tenant_id, session.setup_session_id);
        self.inner.write().sessions.insert(key, session);
        Box::pin(async { Ok(()) })
    }

    fn get_setup_session(
        &self,
        tenant_id: &str,
        session_id: &str,
    ) -> BoxFuture<'_, Result<Option<SetupSession>, SetupError>> {
        let key = format!("{}::{}", tenant_id.trim(), session_id.trim());
        let result = self.inner.read().sessions.get(&key).cloned();
        Box::pin(async move { Ok(result) })
    }

    fn list_setup_sessions(
        &self,
        tenant_id: &str,
    ) -> BoxFuture<'_, Result<Vec<SetupSession>, SetupError>> {
        let tenant_id = tenant_id.trim().to_string();
        let inner = self.inner.read();
        let mut items: Vec<SetupSession> = inner
            .sessions
            .values()
            .filter(|s| s.tenant_id.trim() == tenant_id)
            .cloned()
            .collect();
        items.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
        Box::pin(async move { Ok(items) })
    }

    fn append_setup_attempt(&self, attempt: SetupAttempt) -> BoxFuture<'_, Result<(), SetupError>> {
        let key = format!("{}::{}", attempt.tenant_id, attempt.setup_session_id);
        self.inner
            .write()
            .attempts
            .entry(key)
            .or_default()
            .push(attempt);
        Box::pin(async { Ok(()) })
    }

    fn list_setup_attempts(
        &self,
        tenant_id: &str,
        session_id: &str,
    ) -> BoxFuture<'_, Result<Vec<SetupAttempt>, SetupError>> {
        let key = format!("{}::{}", tenant_id.trim(), session_id.trim());
        let result = self
            .inner
            .read()
            .attempts
            .get(&key)
            .cloned()
            .unwrap_or_default();
        Box::pin(async move { Ok(result) })
    }
}
