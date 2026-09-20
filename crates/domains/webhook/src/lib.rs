#![allow(unused_doc_comments)]
//! Port of `daemon/internal/webhook` (Roadmap 67): the webhook + external trigger plane.
//! Tenant-scoped webhook endpoints safely trigger runs/workflows/routines with signature
//! authentication, replay protection, bounded + redacted payloads, quota/permission gating, and
//! audit linkage. Webhooks are trigger resources, NOT channel connectors, and never ingest
//! payloads into memory (only the byte size and a redacted outcome are recorded).
//!
//! Wire compatibility with the Go package: string enums serialize as their exact snake_case values
//! and structs use camelCase JSON keys with the same `omitempty` behavior (`skip_serializing_if`).
//!
//! The Go `managerdoc.Store` persistence maps onto `kura_store::{put_document, list_documents}`
//! against `kura_store::SQLiteStore`; a nil store is `Option<&SQLiteStore>` and persistence is
//! skipped while `None`. The Go policy-hook interfaces (`Firer`, `QuotaGate`) become Rust
//! traits with default all-pass/noop implementations. The Go `context.Context` parameters are
//! dropped (sync port). HMAC-SHA256 (RFC 2104) is implemented on the workspace `sha2` crate.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

/// Bounds an inbound webhook payload (Go `MaxPayloadBytes`, FR payload bounding).
pub const MAX_PAYLOAD_BYTES: usize = 64 * 1024;

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

/// What a webhook fires (Go `TargetKind`).
string_enum!(TargetKind {
    Routine => "routine",
    Workflow => "workflow",
    Run => "run",
});

/// The webhook endpoint lifecycle state (Go `Status`).
string_enum!(Status {
    Active => "active",
    Disabled => "disabled",
});

/// Classifies the outcome of a webhook invocation (Go `TriggerStatus`).
string_enum!(TriggerStatus {
    Fired => "fired",
    ReplaySuppressed => "replay_suppressed",
    AuthFailed => "auth_failed",
    PayloadTooLarge => "payload_too_large",
    QuotaDenied => "quota_denied",
    Disabled => "disabled",
});

/// A tenant-scoped webhook trigger resource. The signing secret is never stored on the
/// projection — only a redacted fingerprint is surfaced (Go `Endpoint`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Endpoint {
    pub webhook_id: String,
    pub tenant_id: String,
    pub environment_scope: String,
    pub name: String,
    pub target_kind: TargetKind,
    pub target_ref: String,
    pub status: Status,
    pub secret_fingerprint: String,
    pub secret_version: i64,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// The audited outcome of one webhook invocation. It never carries payload content — only the
/// byte size and the redacted outcome (FR payload redaction) (Go `TriggerRecord`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TriggerRecord {
    pub trigger_id: String,
    pub webhook_id: String,
    pub tenant_id: String,
    pub environment_scope: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub idempotency_key: String,
    pub status: TriggerStatus,
    pub payload_bytes: i64,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub execution_ref: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub failure_reason: String,
    pub created_at: DateTime<Utc>,
}

/// Returned once when a webhook is created or its secret rotated. The plaintext secret is
/// returned to the caller exactly once and never persisted in cleartext (Go `CreateSecret`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateSecret {
    pub endpoint: Endpoint,
    pub secret: String,
}

/// The durable form of a webhook: its projection plus the signing secret (hex) needed to verify
/// signatures across restarts (Go `persistedEndpoint`, unexported).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PersistedEndpoint {
    pub endpoint: Endpoint,
    pub secret_hex: String,
}

/// One inbound webhook invocation (Go `TriggerInput`; internal input type, no JSON tags).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TriggerInput {
    pub webhook_id: String,
    pub tenant_id: String,
    /// Hex HMAC-SHA256 of the payload using the signing secret.
    pub signature: String,
    pub idempotency_key: String,
    pub payload: Vec<u8>,
}

/// Manager validation/lookup failures (Go sentinel errors `ErrEndpointNotFound`,
/// `ErrInvalidEndpoint`, `ErrMissingAuth`, `ErrBadSignature`, `ErrCrossTenant`,
/// `ErrDisabled`, `ErrPayloadTooLarge`, `ErrQuotaDenied`).
#[derive(Debug, thiserror::Error, Clone, PartialEq, Eq)]
pub enum WebhookError {
    #[error("webhook endpoint not found")]
    EndpointNotFound,
    #[error("webhook endpoint definition is invalid")]
    InvalidEndpoint,
    #[error("webhook request is missing a signature")]
    MissingAuth,
    #[error("webhook signature is invalid")]
    BadSignature,
    #[error("webhook does not belong to the requesting tenant")]
    CrossTenant,
    #[error("webhook endpoint is disabled")]
    Disabled,
    #[error("webhook payload exceeds the size limit")]
    PayloadTooLarge,
    #[error("webhook trigger denied by quota or permission")]
    QuotaDenied,
    #[error("webhook trigger failed: {0}")]
    Fire(String),
}

/// Document kind used for durable webhook endpoints + secrets (Go `docKindWebhook`).
const DOC_KIND_WEBHOOK: &str = "webhook_endpoint";

/// Fires a webhook's target (run/workflow/routine) and returns an execution reference. It runs
/// only after authentication, replay, bounding, and quota checks pass (Go `Firer` interface).
pub trait Firer: Send + Sync {
    fn fire(&self, endpoint: &Endpoint, payload: &[u8]) -> Result<String, String>;
}

/// Default firer: records nothing and returns a synthetic reference (Go `noopFirer`), used when
/// no real firer is wired.
#[derive(Debug, Clone, Copy, Default)]
pub struct NoopFirer;

impl Firer for NoopFirer {
    fn fire(&self, _endpoint: &Endpoint, _payload: &[u8]) -> Result<String, String> {
        Ok("webhook_exec_unwired".to_string())
    }
}

/// Runs the quota/permission check before any execution starts; returns `false` plus a reason to
/// deny (Go `QuotaGate` interface).
pub trait QuotaGate: Send + Sync {
    fn allow(&self, tenant_id: &str, webhook_id: &str) -> (bool, String);
}

/// The default permissive gate (Go `allowAllQuota`).
#[derive(Debug, Clone, Copy, Default)]
pub struct AllowAllQuota;

impl QuotaGate for AllowAllQuota {
    fn allow(&self, _tenant_id: &str, _webhook_id: &str) -> (bool, String) {
        (true, String::new())
    }
}

#[derive(Default)]
struct ManagerInner {
    by_id: HashMap<String, Endpoint>,
    /// webhook id -> signing secret (never projected).
    secrets: HashMap<String, Vec<u8>>,
    /// webhook id -> seen idempotency keys.
    seen_keys: HashMap<String, HashSet<String>>,
    ids: Vec<String>,
}

/// Owns webhook endpoints, verifies inbound triggers, and dispatches to the firer (Go
/// `Manager`). Endpoints + secrets + replay keys are in-memory for this slice; the firer routes
/// execution to the existing runtime/routine planes which own the durable execution evidence.
pub struct Manager {
    inner: parking_lot::RwLock<ManagerInner>,
    env: String,
    firer: Box<dyn Firer>,
    quota: Box<dyn QuotaGate>,
    docs: Option<Arc<parking_lot::Mutex<kura_store::SQLiteStore>>>,
}

impl Manager {
    /// Go `NewManager`: `firer`/`quota` default to `NoopFirer`/`AllowAllQuota` when
    /// `None`.
    #[must_use]
    pub fn new(
        environment_scope: &str,
        firer: Option<Box<dyn Firer>>,
        quota: Option<Box<dyn QuotaGate>>,
    ) -> Self {
        Manager {
            inner: parking_lot::RwLock::new(ManagerInner::default()),
            env: environment_scope.trim().to_string(),
            firer: firer.unwrap_or_else(|| Box::new(NoopFirer)),
            quota: quota.unwrap_or_else(|| Box::new(AllowAllQuota)),
            docs: None,
        }
    }

    /// Go `WithStore`: installs durable persistence for webhook endpoints + secrets and returns
    /// the manager.
    pub fn with_store(
        &mut self,
        store: Arc<parking_lot::Mutex<kura_store::SQLiteStore>>,
    ) -> &mut Self {
        self.docs = Some(store);
        self
    }

    /// Go `LoadFromStore`: reloads persisted webhook endpoints + signing secrets on startup.
    /// A no-op when no store is installed.
    pub fn load_from_store(&self) -> Result<(), String> {
        let Some(docs) = &self.docs else {
            return Ok(());
        };
        let items: Vec<PersistedEndpoint> =
            kura_store::list_documents(&docs.lock(), DOC_KIND_WEBHOOK)?;
        let mut inner = self.inner.write();
        for item in items {
            let endpoint = item.endpoint;
            if !inner.by_id.contains_key(&endpoint.webhook_id) {
                inner.ids.push(endpoint.webhook_id.clone());
            }
            inner
                .by_id
                .insert(endpoint.webhook_id.clone(), endpoint.clone());
            if let Some(secret) = decode_hex(&item.secret_hex) {
                inner.secrets.insert(endpoint.webhook_id, secret);
            }
        }
        Ok(())
    }

    /// Registers a webhook endpoint and returns the plaintext signing secret exactly once
    /// (Go `Create`).
    pub fn create(
        &self,
        tenant_id: &str,
        name: &str,
        target_kind: TargetKind,
        target_ref: &str,
    ) -> Result<CreateSecret, WebhookError> {
        if tenant_id.trim().is_empty() || name.trim().is_empty() || target_ref.trim().is_empty() {
            return Err(WebhookError::InvalidEndpoint);
        }
        if !valid_target_kind(target_kind) {
            return Err(WebhookError::InvalidEndpoint);
        }
        let secret = random_secret();
        let now = Utc::now();
        let endpoint = Endpoint {
            webhook_id: new_id("webhook"),
            tenant_id: tenant_id.trim().to_string(),
            environment_scope: self.env.clone(),
            name: name.trim().to_string(),
            target_kind,
            target_ref: target_ref.trim().to_string(),
            status: Status::Active,
            secret_fingerprint: fingerprint(&secret),
            secret_version: 1,
            created_at: now,
            updated_at: now,
        };
        {
            let mut inner = self.inner.write();
            inner
                .by_id
                .insert(endpoint.webhook_id.clone(), endpoint.clone());
            inner.ids.push(endpoint.webhook_id.clone());
            inner
                .secrets
                .insert(endpoint.webhook_id.clone(), secret.clone());
        }
        self.persist(&endpoint, &secret);
        Ok(CreateSecret {
            endpoint,
            secret: encode_hex(&secret),
        })
    }

    /// Issues a new signing secret, invalidating the previous one (Go `Rotate`).
    pub fn rotate(&self, tenant_id: &str, webhook_id: &str) -> Result<CreateSecret, WebhookError> {
        let (endpoint, secret) = {
            let mut inner = self.inner.write();
            let existing = inner
                .by_id
                .get(webhook_id.trim())
                .cloned()
                .ok_or(WebhookError::EndpointNotFound)?;
            if existing.tenant_id != tenant_id.trim() {
                return Err(WebhookError::CrossTenant);
            }
            let secret = random_secret();
            let mut endpoint = existing;
            endpoint.secret_fingerprint = fingerprint(&secret);
            endpoint.secret_version += 1;
            endpoint.updated_at = Utc::now();
            inner
                .by_id
                .insert(endpoint.webhook_id.clone(), endpoint.clone());
            inner
                .secrets
                .insert(endpoint.webhook_id.clone(), secret.clone());
            (endpoint, secret)
        };
        self.persist(&endpoint, &secret);
        Ok(CreateSecret {
            endpoint,
            secret: encode_hex(&secret),
        })
    }

    /// Deactivates a webhook so further triggers are rejected (Go `Disable`).
    pub fn disable(&self, tenant_id: &str, webhook_id: &str) -> Result<Endpoint, WebhookError> {
        let (endpoint, secret) = {
            let mut inner = self.inner.write();
            let existing = inner
                .by_id
                .get(webhook_id.trim())
                .cloned()
                .ok_or(WebhookError::EndpointNotFound)?;
            if existing.tenant_id != tenant_id.trim() {
                return Err(WebhookError::CrossTenant);
            }
            let secret = inner
                .secrets
                .get(webhook_id.trim())
                .cloned()
                .unwrap_or_default();
            let mut endpoint = existing;
            endpoint.status = Status::Disabled;
            endpoint.updated_at = Utc::now();
            inner
                .by_id
                .insert(endpoint.webhook_id.clone(), endpoint.clone());
            (endpoint, secret)
        };
        self.persist(&endpoint, &secret);
        Ok(endpoint)
    }

    /// Go `Get`: returns the endpoint only when it exists and belongs to the requesting tenant.
    pub fn get(&self, tenant_id: &str, webhook_id: &str) -> Option<Endpoint> {
        let inner = self.inner.read();
        match inner.by_id.get(webhook_id.trim()) {
            Some(endpoint) if endpoint.tenant_id == tenant_id.trim() => Some(endpoint.clone()),
            _ => None,
        }
    }

    /// Go `ListForTenant` (insertion order, mirroring the `kura-runtime` manager convention).
    pub fn list_for_tenant(&self, tenant_id: &str) -> Vec<Endpoint> {
        let inner = self.inner.read();
        inner
            .ids
            .iter()
            .filter_map(|id| inner.by_id.get(id))
            .filter(|endpoint| endpoint.tenant_id == tenant_id.trim())
            .cloned()
            .collect()
    }

    /// Authenticates and dispatches an inbound webhook (Go `Trigger`). The check order is:
    /// tenant/endpoint resolution, status, payload bounding, signature, replay protection,
    /// quota/permission, then fire. Every outcome is recorded as a redacted `TriggerRecord` (no
    /// payload content). The record is returned on both success and failure, mirroring Go's
    /// `(TriggerRecord, error)` signature.
    pub fn trigger(&self, input: TriggerInput) -> (TriggerRecord, Result<(), WebhookError>) {
        let (endpoint, secret) = {
            let inner = self.inner.read();
            let endpoint = inner.by_id.get(input.webhook_id.trim()).cloned();
            let secret = inner
                .secrets
                .get(input.webhook_id.trim())
                .cloned()
                .unwrap_or_default();
            (endpoint, secret)
        };

        // `status` is overwritten on every return path below.
        let mut record = TriggerRecord {
            trigger_id: new_id("webhook_trigger"),
            webhook_id: input.webhook_id.trim().to_string(),
            tenant_id: input.tenant_id.trim().to_string(),
            environment_scope: self.env.clone(),
            idempotency_key: input.idempotency_key.trim().to_string(),
            status: TriggerStatus::Fired,
            payload_bytes: input.payload.len() as i64,
            execution_ref: String::new(),
            failure_reason: String::new(),
            created_at: Utc::now(),
        };

        let Some(endpoint) = endpoint else {
            return fail(
                record,
                TriggerStatus::AuthFailed,
                WebhookError::EndpointNotFound,
            );
        };
        if endpoint.tenant_id != input.tenant_id.trim() {
            return fail(record, TriggerStatus::AuthFailed, WebhookError::CrossTenant);
        }
        if endpoint.status != Status::Active {
            return fail(record, TriggerStatus::Disabled, WebhookError::Disabled);
        }
        if input.payload.len() > MAX_PAYLOAD_BYTES {
            return fail(
                record,
                TriggerStatus::PayloadTooLarge,
                WebhookError::PayloadTooLarge,
            );
        }
        if input.signature.trim().is_empty() {
            return fail(record, TriggerStatus::AuthFailed, WebhookError::MissingAuth);
        }
        if !verify_signature(&secret, &input.payload, &input.signature) {
            return fail(
                record,
                TriggerStatus::AuthFailed,
                WebhookError::BadSignature,
            );
        }
        let idempotency_key = input.idempotency_key.trim();
        if !idempotency_key.is_empty() && self.mark_seen(&endpoint.webhook_id, idempotency_key) {
            record.status = TriggerStatus::ReplaySuppressed;
            return (record, Ok(()));
        }
        let (allowed, reason) = self.quota.allow(&endpoint.tenant_id, &endpoint.webhook_id);
        if !allowed {
            record.failure_reason = reason;
            return fail(
                record,
                TriggerStatus::QuotaDenied,
                WebhookError::QuotaDenied,
            );
        }
        match self.firer.fire(&endpoint, &input.payload) {
            Err(err) => fail(record, TriggerStatus::AuthFailed, WebhookError::Fire(err)),
            Ok(reference) => {
                record.status = TriggerStatus::Fired;
                record.execution_ref = reference;
                (record, Ok(()))
            }
        }
    }

    /// Resolves the tenant from the (signature-authenticated) endpoint and triggers it. This is
    /// the inbound-ingress entry point where the request is authenticated by the signature rather
    /// than a bearer principal (Go `TriggerSigned`).
    pub fn trigger_signed(
        &self,
        webhook_id: &str,
        signature: &str,
        idempotency_key: &str,
        payload: Vec<u8>,
    ) -> (TriggerRecord, Result<(), WebhookError>) {
        let tenant = {
            let inner = self.inner.read();
            inner
                .by_id
                .get(webhook_id.trim())
                .map(|endpoint| endpoint.tenant_id.clone())
                .unwrap_or_default()
        };
        self.trigger(TriggerInput {
            webhook_id: webhook_id.to_string(),
            tenant_id: tenant,
            signature: signature.to_string(),
            idempotency_key: idempotency_key.to_string(),
            payload,
        })
    }

    /// Go `markSeen`: records an idempotency key and reports whether it was already seen
    /// (replay).
    fn mark_seen(&self, webhook_id: &str, key: &str) -> bool {
        let mut inner = self.inner.write();
        let keys = inner.seen_keys.entry(webhook_id.to_string()).or_default();
        if keys.contains(key) {
            return true;
        }
        keys.insert(key.to_string());
        false
    }

    /// Go `persist`: write-through of an endpoint + its secret (errors ignored, as in Go).
    fn persist(&self, endpoint: &Endpoint, secret: &[u8]) {
        let Some(docs) = &self.docs else {
            return;
        };
        let persisted = PersistedEndpoint {
            endpoint: endpoint.clone(),
            secret_hex: encode_hex(secret),
        };
        let _ = kura_store::put_document(
            &docs.lock(),
            DOC_KIND_WEBHOOK,
            &endpoint.webhook_id,
            &self.env,
            &endpoint.tenant_id,
            &persisted,
        );
    }
}

/// Go `fail`: stamps the redacted status + failure reason and returns the record with the
/// error. A free function (it needs no manager state).
fn fail(
    mut record: TriggerRecord,
    status: TriggerStatus,
    err: WebhookError,
) -> (TriggerRecord, Result<(), WebhookError>) {
    record.status = status;
    if record.failure_reason.is_empty() {
        record.failure_reason = err.to_string();
    }
    (record, Err(err))
}

/// Go `validTargetKind`.
#[must_use]
fn valid_target_kind(kind: TargetKind) -> bool {
    matches!(
        kind,
        TargetKind::Routine | TargetKind::Workflow | TargetKind::Run
    )
}

/// Go `verifySignature` (HMAC-SHA256 over the payload, constant-time compare).
#[must_use]
fn verify_signature(secret: &[u8], payload: &[u8], signature: &str) -> bool {
    if secret.is_empty() {
        return false;
    }
    let mac = hmac_sha256(secret, payload);
    let Some(got) = decode_hex(signature.trim()) else {
        return false;
    };
    constant_time_eq(&mac, &got)
}

/// Go `Sign`: computes the expected signature for a payload (used by clients/tests).
#[must_use]
pub fn sign(secret_hex: &str, payload: &[u8]) -> String {
    let Some(secret) = decode_hex(secret_hex.trim()) else {
        return String::new();
    };
    encode_hex(&hmac_sha256(&secret, payload))
}

/// HMAC-SHA256 per RFC 2104, built on the workspace `sha2` crate (Go `crypto/hmac` +
/// `crypto/sha256`).
fn hmac_sha256(key: &[u8], data: &[u8]) -> [u8; 32] {
    const BLOCK_SIZE: usize = 64;
    let mut k = [0u8; BLOCK_SIZE];
    if key.len() > BLOCK_SIZE {
        let digest = Sha256::digest(key);
        k[..32].copy_from_slice(&digest);
    } else {
        k[..key.len()].copy_from_slice(key);
    }
    let mut ipad = [0x36u8; BLOCK_SIZE];
    let mut opad = [0x5cu8; BLOCK_SIZE];
    for i in 0..BLOCK_SIZE {
        ipad[i] ^= k[i];
        opad[i] ^= k[i];
    }
    let inner = Sha256::new()
        .chain_update(ipad)
        .chain_update(data)
        .finalize();
    Sha256::new()
        .chain_update(opad)
        .chain_update(inner)
        .finalize()
        .into()
}

/// Constant-time equality (Go `hmac.Equal`).
#[must_use]
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

/// Go `randomSecret`: 32 random bytes. Backed by v4 UUID randomness (getrandom/CSPRNG) so no
/// extra crate is needed.
fn random_secret() -> Vec<u8> {
    let mut buf = [0u8; 32];
    buf[..16].copy_from_slice(Uuid::new_v4().as_bytes());
    buf[16..].copy_from_slice(Uuid::new_v4().as_bytes());
    buf.to_vec()
}

/// Go `fingerprint`: `sha256:` + the first 12 hex chars of the sha256 digest (redacted; never
/// the secret).
#[must_use]
fn fingerprint(secret: &[u8]) -> String {
    let digest = Sha256::digest(secret);
    format!("sha256:{}", &encode_hex(&digest)[..12])
}

/// Go `newID`: `prefix` + 16 hex chars of random bytes (reference `kura-runtime` convention).
#[must_use]
fn new_id(prefix: &str) -> String {
    let hex = Uuid::new_v4().simple().to_string();
    format!("{prefix}_{}", &hex[..16])
}

fn encode_hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

fn decode_hex(s: &str) -> Option<Vec<u8>> {
    if s.len() % 2 != 0 {
        return None;
    }
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len() / 2);
    for pair in bytes.chunks_exact(2) {
        out.push((hex_val(pair[0])? << 4) | hex_val(pair[1])?);
    }
    Some(out)
}

fn hex_val(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}
