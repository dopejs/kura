//! Port of `daemon/internal/integrations`: the integration resource registry, backend
//! seam, readiness/auth/health projection, canonical-default selection, and probe
//! dispatch. The diagnostics subsystem and Feishu/Lark diagnostic backend are a follow-up
//! increment.

use std::collections::HashMap;
use std::sync::Arc;

use chrono::{DateTime, Utc};
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

mod classifier;
mod diagnostics;
mod diagnostics_runtime;
pub use classifier::*;
pub use diagnostics::*;
pub use diagnostics_runtime::*;

macro_rules! string_enum {
    ($name:ident { $first:ident => $first_s:literal $(, $v:ident => $s:literal)* $(,)? }) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
        pub enum $name {
            #[default]
            #[serde(rename = $first_s)]
            $first,
            $(#[serde(rename = $s)] $v),*
        }
        impl $name {
            #[must_use]
            pub fn as_str(self) -> &'static str {
                match self {
                    $name::$first => $first_s,
                    $( $name::$v => $s ),*
                }
            }
        }
        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str(self.as_str())
            }
        }
    };
}

string_enum!(ReadinessStatus {
    NotConfigured => "not_configured",
    AuthPending => "auth_pending",
    Healthy => "healthy",
    Degraded => "degraded",
    Unavailable => "unavailable",
});

string_enum!(AuthState {
    NotStarted => "not_started",
    Pending => "pending",
    Authorized => "authorized",
    Expired => "expired",
    Revoked => "revoked",
    NotApplicable => "not_applicable",
});

string_enum!(HealthState {
    Unknown => "unknown",
    Healthy => "healthy",
    Degraded => "degraded",
    Unavailable => "unavailable",
});

string_enum!(BackendKind {
    Mcp => "mcp",
    ManagedProvider => "managed_provider",
    Native => "native",
    FakeLocal => "fake_local",
    FeishuLark => "feishu_lark",
    AdapterRpc => "adapter_rpc",
});

string_enum!(ProbeKind {
    Inspect => "inspect",
    Mutate => "mutate",
});

#[derive(Debug, thiserror::Error, Clone, PartialEq, Eq)]
pub enum IntegrationError {
    #[error("integration id is required")]
    IntegrationIdRequired,
    #[error("domain kind is required")]
    DomainKindRequired,
    #[error("display name is required")]
    DisplayNameRequired,
    #[error("backend kind is required")]
    BackendKindRequired,
    #[error("integration not found")]
    IntegrationNotFound,
    #[error("integration probe unsupported")]
    ProbeUnsupported,
    #[error("integration probe blocked")]
    ProbeBlocked,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountBinding {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub account_key: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub account_label: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub account_type: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub tenant_label: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub external_account_id: String,
    pub known_after_auth: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BackendBinding {
    pub backend_kind: BackendKind,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub backend_ref_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub backend_display_name: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub source_kind: String,
    pub supports_interactive_auth: bool,
    pub supports_probe_read: bool,
    pub supports_probe_mutation: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Provenance {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub secret_resolution: String,
    pub secret_material_present: bool,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub environment_scope: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub backed_by: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Resource {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub tenant_id: String,
    pub integration_id: String,
    pub domain_kind: String,
    pub display_name: String,
    pub environment_scope: String,
    pub readiness_status: ReadinessStatus,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub auth_state: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub health_state: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub readiness_reason: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub required_operator_action: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub disabled_reason: String,
    pub canonical_default: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account_binding: Option<AccountBinding>,
    pub backend_binding: BackendBinding,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provenance: Option<Provenance>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_ready_at: Option<DateTime<Utc>>,
    pub last_transition_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BindingSummary {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub tenant_id: String,
    pub integration_id: String,
    pub domain_kind: String,
    pub display_name: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub account_key: String,
    pub canonical_default: bool,
    pub readiness_at_invocation: ReadinessStatus,
    pub backend_kind: BackendKind,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub secret_resolution: String,
    pub environment_scope: String,
    pub captured_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Default)]
pub struct CreateInput {
    pub tenant_id: String,
    pub integration_id: String,
    pub domain_kind: String,
    pub display_name: String,
    pub account_binding: AccountBinding,
    pub backend_binding: BackendBinding,
    pub canonical_default: bool,
    pub environment_scope: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateReadinessInput {
    pub readiness_status: ReadinessStatus,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub auth_state: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub health_state: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub reason: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub required_operator_action: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account_binding: Option<AccountBinding>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub secret_resolution: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProbeResult {
    pub probe_kind: ProbeKind,
    pub status: String,
    #[serde(default, skip_serializing_if = "Map::is_empty")]
    pub result_summary: Map<String, Value>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub failure_class: String,
}

pub trait Backend: Send + Sync {
    fn run_probe(
        &self,
        resource: &Resource,
        probe_kind: ProbeKind,
        input: &Map<String, Value>,
    ) -> Result<ProbeResult, IntegrationError>;
    fn supported_domain_kinds(&self) -> Vec<String>;
}

#[must_use]
pub fn backend_kind_supports_domain(kind: BackendKind, domain_kind: &str) -> bool {
    let trimmed = domain_kind.trim();
    match kind {
        BackendKind::FakeLocal => FakeBackend.supports_domain_kind(trimmed),
        _ => false,
    }
}

fn normalize_backend_binding(mut binding: BackendBinding) -> BackendBinding {
    binding.backend_ref_id = binding.backend_ref_id.trim().to_string();
    binding.backend_display_name = binding.backend_display_name.trim().to_string();
    binding.source_kind = binding.source_kind.trim().to_string();
    if binding.source_kind.is_empty() {
        binding.source_kind = binding.backend_kind.as_str().to_string();
    }
    binding
}

string_enum!(FakeFaultType {
    Transient5xx => "transient_5xx",
    RateLimit => "rate_limit",
    AuthExpiry => "auth_expiry",
    ProviderUnavailable => "provider_unavailable",
    SlowResponse => "slow_response",
    MalformedResponse => "malformed_response",
});

#[derive(Debug, Clone, Default)]
pub struct FakeFaultDrillResult {
    pub fault_type: FakeFaultType,
    pub domain_kind: String,
    pub observed_classification: String,
    pub operator_action_needed: bool,
}

pub struct FakeBackend;

impl FakeBackend {
    fn supports_domain_kind(&self, domain_kind: &str) -> bool {
        self.supported_domain_kinds()
            .iter()
            .any(|d| d == domain_kind)
    }

    #[must_use]
    pub fn run_fault_drill(
        &self,
        resource: &Resource,
        fault_type: FakeFaultType,
    ) -> FakeFaultDrillResult {
        let (classification, operator_action_needed) = match fault_type {
            FakeFaultType::AuthExpiry | FakeFaultType::MalformedResponse => {
                ("operator_action_needed", true)
            }
            FakeFaultType::ProviderUnavailable => ("retry_exhausted", true),
            _ => ("recovered", false),
        };
        FakeFaultDrillResult {
            fault_type,
            domain_kind: resource.domain_kind.clone(),
            observed_classification: classification.to_string(),
            operator_action_needed,
        }
    }
}

impl Backend for FakeBackend {
    fn supported_domain_kinds(&self) -> Vec<String> {
        vec!["calendar".to_string(), "mail".to_string()]
    }

    fn run_probe(
        &self,
        resource: &Resource,
        probe_kind: ProbeKind,
        input: &Map<String, Value>,
    ) -> Result<ProbeResult, IntegrationError> {
        let mut result_summary = Map::new();
        result_summary.insert(
            "integrationId".to_string(),
            Value::String(resource.integration_id.clone()),
        );
        result_summary.insert(
            "domainKind".to_string(),
            Value::String(resource.domain_kind.clone()),
        );
        result_summary.insert(
            "backendKind".to_string(),
            Value::String(resource.backend_binding.backend_kind.as_str().to_string()),
        );
        result_summary.insert(
            "probeKind".to_string(),
            Value::String(probe_kind.as_str().to_string()),
        );
        if !input.is_empty() {
            result_summary.insert("input".to_string(), Value::Object(input.clone()));
        }
        let message = match probe_kind {
            ProbeKind::Inspect => format!("fake inspect probe for {}", resource.display_name),
            ProbeKind::Mutate => format!("fake mutate probe for {}", resource.display_name),
        };
        result_summary.insert("message".to_string(), Value::String(message));
        Ok(ProbeResult {
            probe_kind,
            status: "completed".to_string(),
            result_summary,
            ..ProbeResult::default()
        })
    }
}

#[must_use]
pub fn live_validation_matrix_rows() -> Vec<kura_livevalidation::MatrixRow> {
    let classes = [
        kura_livevalidation::ToolClass::from(
            kura_livevalidation::ToolClass::INTEGRATION_PROBE_READ,
        ),
        kura_livevalidation::ToolClass::from(
            kura_livevalidation::ToolClass::INTEGRATION_PROBE_MUTATION,
        ),
    ];
    let mut rows = Vec::new();
    for tool_class in classes {
        if let Some(row) = kura_livevalidation::default_matrix_row(&tool_class) {
            rows.push(row);
        }
    }
    rows
}

#[derive(Default)]
struct Inner {
    by_id: HashMap<String, Resource>,
    order: Vec<String>,
}

pub struct Manager {
    inner: RwLock<Inner>,
    backends: HashMap<BackendKind, Arc<dyn Backend>>,
    env: String,
}

impl Manager {
    #[must_use]
    pub fn new(environment_scope: &str) -> Self {
        let mut backends: HashMap<BackendKind, Arc<dyn Backend>> = HashMap::new();
        backends.insert(BackendKind::FakeLocal, Arc::new(FakeBackend));
        Manager {
            inner: RwLock::new(Inner::default()),
            backends,
            env: environment_scope.trim().to_string(),
        }
    }

    pub fn restore(&self, items: Vec<Resource>) {
        let mut inner = self.inner.write();
        inner.by_id.clear();
        inner.order.clear();
        for item in items {
            let id = item.integration_id.clone();
            inner.order.push(id.clone());
            inner.by_id.insert(id, item);
        }
    }

    #[must_use]
    pub fn list(&self) -> Vec<Resource> {
        let inner = self.inner.read();
        inner
            .order
            .iter()
            .map(|id| inner.by_id[id].clone())
            .collect()
    }

    #[must_use]
    pub fn list_for_tenant(&self, tenant_id: &str) -> Vec<Resource> {
        let inner = self.inner.read();
        let tenant_id = tenant_id.trim();
        inner
            .order
            .iter()
            .filter_map(|id| {
                let item = &inner.by_id[id];
                if tenant_id.is_empty() || item.tenant_id.trim() == tenant_id {
                    Some(item.clone())
                } else {
                    None
                }
            })
            .collect()
    }

    #[must_use]
    pub fn get(&self, integration_id: &str) -> Option<Resource> {
        self.inner.read().by_id.get(integration_id.trim()).cloned()
    }

    #[must_use]
    pub fn get_for_tenant(&self, integration_id: &str, tenant_id: &str) -> Option<Resource> {
        let item = self.get(integration_id)?;
        if !tenant_id.trim().is_empty() && item.tenant_id.trim() != tenant_id.trim() {
            return None;
        }
        Some(item)
    }

    pub fn create(&self, input: CreateInput) -> Result<Resource, IntegrationError> {
        if input.integration_id.trim().is_empty() {
            return Err(IntegrationError::IntegrationIdRequired);
        }
        if input.domain_kind.trim().is_empty() {
            return Err(IntegrationError::DomainKindRequired);
        }
        if input.display_name.trim().is_empty() {
            return Err(IntegrationError::DisplayNameRequired);
        }
        if input.backend_binding.backend_kind.as_str().is_empty() {
            return Err(IntegrationError::BackendKindRequired);
        }
        let mut inner = self.inner.write();
        let now = Utc::now();
        let resource = Resource {
            tenant_id: input.tenant_id.trim().to_string(),
            integration_id: input.integration_id.trim().to_string(),
            domain_kind: input.domain_kind.trim().to_string(),
            display_name: input.display_name.trim().to_string(),
            environment_scope: first_non_empty(&[input.environment_scope.trim(), &self.env]),
            readiness_status: ReadinessStatus::NotConfigured,
            auth_state: AuthState::NotStarted.as_str().to_string(),
            health_state: HealthState::Unknown.as_str().to_string(),
            canonical_default: input.canonical_default,
            account_binding: Some(input.account_binding),
            backend_binding: normalize_backend_binding(input.backend_binding.clone()),
            provenance: Some(Provenance {
                environment_scope: first_non_empty(&[input.environment_scope.trim(), &self.env]),
                backed_by: input.backend_binding.backend_kind.as_str().to_string(),
                ..Provenance::default()
            }),
            created_at: now,
            updated_at: now,
            last_transition_at: now,
            ..Resource::default()
        };
        if !inner.by_id.contains_key(&resource.integration_id) {
            inner.order.push(resource.integration_id.clone());
        }
        inner
            .by_id
            .insert(resource.integration_id.clone(), resource.clone());
        if resource.canonical_default {
            demote_sibling_defaults(&mut inner, &resource);
            inner
                .by_id
                .insert(resource.integration_id.clone(), resource.clone());
        }
        Ok(resource)
    }

    pub fn update_readiness(
        &self,
        integration_id: &str,
        input: UpdateReadinessInput,
    ) -> Result<Resource, IntegrationError> {
        let mut inner = self.inner.write();
        let resource = inner
            .by_id
            .get(integration_id.trim())
            .cloned()
            .ok_or(IntegrationError::IntegrationNotFound)?;
        let resource = update_readiness_locked(resource, input);
        inner
            .by_id
            .insert(resource.integration_id.clone(), resource.clone());
        Ok(resource)
    }

    pub fn set_canonical_default(
        &self,
        integration_id: &str,
    ) -> Result<Resource, IntegrationError> {
        let mut inner = self.inner.write();
        let mut resource = inner
            .by_id
            .get(integration_id.trim())
            .cloned()
            .ok_or(IntegrationError::IntegrationNotFound)?;
        resource.canonical_default = true;
        resource.updated_at = Utc::now();
        demote_sibling_defaults(&mut inner, &resource);
        inner
            .by_id
            .insert(resource.integration_id.clone(), resource.clone());
        Ok(resource)
    }

    pub fn disconnect(
        &self,
        integration_id: &str,
        reason: &str,
    ) -> Result<Resource, IntegrationError> {
        let mut inner = self.inner.write();
        let resource = inner
            .by_id
            .get(integration_id.trim())
            .cloned()
            .ok_or(IntegrationError::IntegrationNotFound)?;
        let resource = disconnect_locked(resource, reason);
        inner
            .by_id
            .insert(resource.integration_id.clone(), resource.clone());
        Ok(resource)
    }

    pub fn binding_summary(
        &self,
        integration_id: &str,
        captured_at: DateTime<Utc>,
    ) -> Result<BindingSummary, IntegrationError> {
        let inner = self.inner.read();
        let resource = inner
            .by_id
            .get(integration_id.trim())
            .ok_or(IntegrationError::IntegrationNotFound)?;
        Ok(resource_binding_summary(resource, captured_at))
    }

    pub fn run_probe(
        &self,
        integration_id: &str,
        probe_kind: ProbeKind,
        input: &Map<String, Value>,
    ) -> Result<(Resource, ProbeResult, BindingSummary), IntegrationError> {
        let (resource, backend) = {
            let inner = self.inner.read();
            let resource = inner
                .by_id
                .get(integration_id.trim())
                .cloned()
                .ok_or(IntegrationError::IntegrationNotFound)?;
            let backend = self
                .backends
                .get(&resource.backend_binding.backend_kind)
                .cloned();
            (resource, backend)
        };
        let Some(backend) = backend else {
            return Err(IntegrationError::ProbeUnsupported);
        };
        if resource.readiness_status == ReadinessStatus::Unavailable {
            return Err(IntegrationError::ProbeBlocked);
        }
        if probe_kind == ProbeKind::Inspect && !resource.backend_binding.supports_probe_read {
            return Err(IntegrationError::ProbeUnsupported);
        }
        if probe_kind == ProbeKind::Mutate && !resource.backend_binding.supports_probe_mutation {
            return Err(IntegrationError::ProbeUnsupported);
        }
        let result = backend.run_probe(&resource, probe_kind, input)?;
        let summary = resource_binding_summary(&resource, Utc::now());
        Ok((resource, result, summary))
    }
}

fn update_readiness_locked(mut resource: Resource, input: UpdateReadinessInput) -> Resource {
    let now = Utc::now();
    resource.readiness_status = input.readiness_status;
    if !input.auth_state.is_empty() {
        resource.auth_state = input.auth_state.clone();
    }
    if !input.health_state.is_empty() {
        resource.health_state = input.health_state.clone();
    }
    resource.readiness_reason = input.reason.trim().to_string();
    resource.required_operator_action = input.required_operator_action.trim().to_string();
    if let Some(account_binding) = &input.account_binding {
        if !account_binding.account_key.trim().is_empty()
            || !account_binding.account_label.trim().is_empty()
            || !account_binding.external_account_id.trim().is_empty()
        {
            resource.account_binding = Some(account_binding.clone());
        }
    }
    if resource.readiness_status == ReadinessStatus::Healthy {
        resource.last_ready_at = Some(now);
    }
    resource.updated_at = now;
    resource.last_transition_at = now;
    update_provenance(resource, &input)
}

fn update_provenance(mut resource: Resource, input: &UpdateReadinessInput) -> Resource {
    let provenance = resource.provenance.get_or_insert_with(Provenance::default);
    if !input.secret_resolution.is_empty() {
        provenance.secret_resolution = input.secret_resolution.clone();
        provenance.secret_material_present = input.secret_resolution == "resolved";
    }
    if provenance.environment_scope.is_empty() {
        provenance.environment_scope = resource.environment_scope.clone();
    }
    if provenance.backed_by.is_empty() {
        provenance.backed_by = resource.backend_binding.backend_kind.as_str().to_string();
    }
    resource
}

fn disconnect_locked(mut resource: Resource, reason: &str) -> Resource {
    let now = Utc::now();
    resource.readiness_status = ReadinessStatus::Unavailable;
    resource.auth_state = AuthState::Revoked.as_str().to_string();
    resource.health_state = HealthState::Unavailable.as_str().to_string();
    resource.disabled_reason = reason.trim().to_string();
    resource.required_operator_action = "reconnect integration".to_string();
    resource.canonical_default = false;
    resource.updated_at = now;
    resource.last_transition_at = now;
    resource
}

fn demote_sibling_defaults(inner: &mut Inner, selected: &Resource) {
    let now = Utc::now();
    for (id, item) in inner.by_id.iter_mut() {
        if *id == selected.integration_id {
            continue;
        }
        if same_binding_group(item, selected) && item.canonical_default {
            item.canonical_default = false;
            item.updated_at = now;
        }
    }
}

#[must_use]
fn same_binding_group(left: &Resource, right: &Resource) -> bool {
    left.tenant_id.trim() == right.tenant_id.trim()
        && left.domain_kind.trim() == right.domain_kind.trim()
        && left.environment_scope.trim() == right.environment_scope.trim()
        && left.account_binding.as_ref().map(|a| a.account_key.trim())
            == right.account_binding.as_ref().map(|a| a.account_key.trim())
}

#[must_use]
fn resource_binding_summary(resource: &Resource, captured_at: DateTime<Utc>) -> BindingSummary {
    BindingSummary {
        tenant_id: resource.tenant_id.clone(),
        integration_id: resource.integration_id.clone(),
        domain_kind: resource.domain_kind.clone(),
        display_name: resource.display_name.clone(),
        account_key: resource
            .account_binding
            .as_ref()
            .map(|a| a.account_key.clone())
            .unwrap_or_default(),
        canonical_default: resource.canonical_default,
        readiness_at_invocation: resource.readiness_status,
        backend_kind: resource.backend_binding.backend_kind,
        secret_resolution: resource
            .provenance
            .as_ref()
            .map(|p| p.secret_resolution.clone())
            .unwrap_or_default(),
        environment_scope: resource.environment_scope.clone(),
        captured_at,
    }
}

#[must_use]
pub fn first_non_empty(values: &[&str]) -> String {
    for value in values {
        let trimmed = value.trim();
        if !trimmed.is_empty() {
            return trimmed.to_string();
        }
    }
    String::new()
}

#[must_use]
pub fn is_unavailable_probe_error(err: &IntegrationError) -> bool {
    matches!(err, IntegrationError::ProbeBlocked)
}
