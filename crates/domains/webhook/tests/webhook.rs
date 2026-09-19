//! Manager-behavior + round-trip tests for `kura-webhook`, mirroring the Go
//! `manager_test.go` / `persistence_test.go` coverage.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use kura_store::SQLiteStore;
use kura_webhook::{
    CreateSecret, Endpoint, Firer, MAX_PAYLOAD_BYTES, Manager, QuotaGate, Status, TargetKind,
    TriggerInput, TriggerRecord, TriggerStatus, WebhookError, sign,
};
use uuid::Uuid;

#[derive(Clone, Default)]
struct RecordingFirer {
    fired: Arc<AtomicUsize>,
}

impl Firer for RecordingFirer {
    fn fire(&self, _endpoint: &Endpoint, _payload: &[u8]) -> Result<String, String> {
        self.fired.fetch_add(1, Ordering::SeqCst);
        Ok("exec_1".to_string())
    }
}

struct DenyQuota;

impl QuotaGate for DenyQuota {
    fn allow(&self, _tenant_id: &str, _webhook_id: &str) -> (bool, String) {
        (false, "monthly webhook quota exhausted".to_string())
    }
}

fn temp_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "kura_webhook_{tag}_{}_{}",
        std::process::id(),
        Uuid::new_v4().simple()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// Mirrors the Go `setup` helper.
fn setup() -> (Manager, Box<RecordingFirer>, Endpoint, String) {
    let firer = Box::new(RecordingFirer::default());
    let m = Manager::new("test", Some(firer.clone() as Box<dyn Firer>), None);
    let created = m
        .create("ten_a", "deploy hook", TargetKind::Routine, "routine_1")
        .unwrap();
    (m, firer, created.endpoint, created.secret)
}

#[test]
fn create_registers_endpoint_and_secret() {
    let m = Manager::new("test", None, None);
    let created: CreateSecret = m
        .create("ten_a", "deploy hook", TargetKind::Routine, "routine_1")
        .unwrap();
    let ep = &created.endpoint;
    assert!(ep.webhook_id.starts_with("webhook_"));
    assert_eq!(ep.tenant_id, "ten_a");
    assert_eq!(ep.environment_scope, "test");
    assert_eq!(ep.status, Status::Active);
    assert_eq!(ep.secret_version, 1);
    assert_eq!(ep.target_kind, TargetKind::Routine);
    assert_eq!(ep.target_ref, "routine_1");
    // The fingerprint is a redacted sha256 prefix, never the secret.
    assert!(ep.secret_fingerprint.starts_with("sha256:"));
    assert_eq!(ep.secret_fingerprint.len(), "sha256:".len() + 12);
    assert!(!created.secret.is_empty());
    assert_eq!(created.secret.len(), 64, "secret is hex of 32 bytes");
}

#[test]
fn create_validates_input() {
    let m = Manager::new("test", None, None);
    assert_eq!(
        m.create("", "name", TargetKind::Routine, "ref")
            .unwrap_err(),
        WebhookError::InvalidEndpoint
    );
    assert_eq!(
        m.create("ten", "", TargetKind::Routine, "ref").unwrap_err(),
        WebhookError::InvalidEndpoint
    );
    assert_eq!(
        m.create("ten", "name", TargetKind::Routine, " ")
            .unwrap_err(),
        WebhookError::InvalidEndpoint
    );
}

#[test]
fn get_and_list_for_tenant() {
    let m = Manager::new("test", None, None);
    let a = m
        .create("ten_a", "a", TargetKind::Run, "run_1")
        .unwrap()
        .endpoint;
    let b = m
        .create("ten_b", "b", TargetKind::Workflow, "wf_1")
        .unwrap()
        .endpoint;
    assert_eq!(
        m.get("ten_a", &a.webhook_id).unwrap().webhook_id,
        a.webhook_id
    );
    // Cross-tenant lookup is denied.
    assert!(m.get("ten_b", &a.webhook_id).is_none());
    assert!(m.get("ten_a", "missing").is_none());
    let listed = m.list_for_tenant("ten_a");
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].webhook_id.as_str(), a.webhook_id.as_str());
    assert_eq!(m.list_for_tenant("ten_b").len(), 1);
    assert_eq!(m.list_for_tenant("ten_c").len(), 0);
    let _ = b;
}

#[test]
fn trigger_fires_cleanly() {
    let (m, firer, ep, secret) = setup();
    let payload = br#"{"event":"push"}"#.to_vec();
    let (rec, result) = m.trigger(TriggerInput {
        webhook_id: ep.webhook_id.clone(),
        tenant_id: "ten_a".to_string(),
        signature: sign(&secret, &payload),
        idempotency_key: "evt-1".to_string(),
        payload: payload.clone(),
    });
    assert!(result.is_ok());
    assert_eq!(rec.status, TriggerStatus::Fired);
    assert_eq!(rec.execution_ref, "exec_1");
    assert_eq!(rec.payload_bytes, payload.len() as i64);
    assert_eq!(rec.tenant_id, "ten_a");
    assert!(rec.trigger_id.starts_with("webhook_trigger_"));
    assert_eq!(firer.fired.load(Ordering::SeqCst), 1);
}

#[test]
fn security_matrix_rejects_without_firing() {
    let (m, firer, ep, secret) = setup();
    let payload = br#"{"event":"push"}"#.to_vec();
    let good = sign(&secret, &payload);

    // Missing auth.
    let (rec, result) = m.trigger(TriggerInput {
        webhook_id: ep.webhook_id.clone(),
        tenant_id: "ten_a".to_string(),
        signature: String::new(),
        idempotency_key: String::new(),
        payload: payload.clone(),
    });
    assert_eq!(result.unwrap_err(), WebhookError::MissingAuth);
    assert_eq!(rec.status, TriggerStatus::AuthFailed);
    assert_eq!(rec.failure_reason, "webhook request is missing a signature");

    // Bad signature.
    let (rec, result) = m.trigger(TriggerInput {
        webhook_id: ep.webhook_id.clone(),
        tenant_id: "ten_a".to_string(),
        signature: "deadbeef".to_string(),
        idempotency_key: String::new(),
        payload: payload.clone(),
    });
    assert_eq!(result.unwrap_err(), WebhookError::BadSignature);
    assert_eq!(rec.status, TriggerStatus::AuthFailed);

    // Cross-tenant.
    let (rec, result) = m.trigger(TriggerInput {
        webhook_id: ep.webhook_id.clone(),
        tenant_id: "ten_b".to_string(),
        signature: good.clone(),
        idempotency_key: String::new(),
        payload: payload.clone(),
    });
    assert_eq!(result.unwrap_err(), WebhookError::CrossTenant);
    assert_eq!(rec.status, TriggerStatus::AuthFailed);

    // Unknown webhook.
    let (rec, result) = m.trigger(TriggerInput {
        webhook_id: "missing".to_string(),
        tenant_id: "ten_a".to_string(),
        signature: good.clone(),
        idempotency_key: String::new(),
        payload: payload.clone(),
    });
    assert_eq!(result.unwrap_err(), WebhookError::EndpointNotFound);
    assert_eq!(rec.status, TriggerStatus::AuthFailed);

    // Oversized payload.
    let big = vec![b'x'; MAX_PAYLOAD_BYTES + 1];
    let (rec, result) = m.trigger(TriggerInput {
        webhook_id: ep.webhook_id.clone(),
        tenant_id: "ten_a".to_string(),
        signature: sign(&secret, &big),
        idempotency_key: String::new(),
        payload: big,
    });
    assert_eq!(result.unwrap_err(), WebhookError::PayloadTooLarge);
    assert_eq!(rec.status, TriggerStatus::PayloadTooLarge);

    assert_eq!(
        firer.fired.load(Ordering::SeqCst),
        0,
        "no failed trigger should fire"
    );

    // Replay: first keyed trigger fires, second is suppressed without error.
    let (rec, result) = m.trigger(TriggerInput {
        webhook_id: ep.webhook_id.clone(),
        tenant_id: "ten_a".to_string(),
        signature: good.clone(),
        idempotency_key: "k1".to_string(),
        payload: payload.clone(),
    });
    assert!(result.is_ok());
    assert_eq!(rec.status, TriggerStatus::Fired);
    let (rec, result) = m.trigger(TriggerInput {
        webhook_id: ep.webhook_id.clone(),
        tenant_id: "ten_a".to_string(),
        signature: good,
        idempotency_key: "k1".to_string(),
        payload: payload.clone(),
    });
    assert!(result.is_ok());
    assert_eq!(rec.status, TriggerStatus::ReplaySuppressed);
    assert_eq!(
        firer.fired.load(Ordering::SeqCst),
        1,
        "replay must not re-fire"
    );

    // Disabled endpoint rejects.
    m.disable("ten_a", &ep.webhook_id).unwrap();
    let (rec, result) = m.trigger(TriggerInput {
        webhook_id: ep.webhook_id.clone(),
        tenant_id: "ten_a".to_string(),
        signature: sign(&secret, &payload),
        idempotency_key: "k2".to_string(),
        payload: payload.clone(),
    });
    assert_eq!(result.unwrap_err(), WebhookError::Disabled);
    assert_eq!(rec.status, TriggerStatus::Disabled);
    assert_eq!(firer.fired.load(Ordering::SeqCst), 1);
}

#[test]
fn quota_denied_before_fire() {
    let firer = Box::new(RecordingFirer::default());
    let m = Manager::new(
        "test",
        Some(firer.clone() as Box<dyn Firer>),
        Some(Box::new(DenyQuota)),
    );
    let created = m
        .create("ten_a", "hook", TargetKind::Workflow, "summarize")
        .unwrap();
    let payload = br#"{}"#.to_vec();
    let (rec, result) = m.trigger(TriggerInput {
        webhook_id: created.endpoint.webhook_id.clone(),
        tenant_id: "ten_a".to_string(),
        signature: sign(&created.secret, &payload),
        idempotency_key: String::new(),
        payload,
    });
    assert_eq!(result.unwrap_err(), WebhookError::QuotaDenied);
    assert_eq!(rec.status, TriggerStatus::QuotaDenied);
    assert_eq!(rec.failure_reason, "monthly webhook quota exhausted");
    assert_eq!(firer.fired.load(Ordering::SeqCst), 0);
}

#[test]
fn rotate_invalidates_old_secret() {
    let (m, _, ep, old_secret) = setup();
    let payload = br#"{"a":1}"#.to_vec();
    let rotated = m.rotate("ten_a", &ep.webhook_id).unwrap();
    assert_eq!(rotated.endpoint.secret_version, 2);
    assert_ne!(rotated.endpoint.secret_fingerprint, ep.secret_fingerprint);
    assert_ne!(rotated.secret, old_secret);

    let (_, result) = m.trigger(TriggerInput {
        webhook_id: ep.webhook_id.clone(),
        tenant_id: "ten_a".to_string(),
        signature: sign(&old_secret, &payload),
        idempotency_key: String::new(),
        payload: payload.clone(),
    });
    assert_eq!(result.unwrap_err(), WebhookError::BadSignature);

    let (rec, result) = m.trigger(TriggerInput {
        webhook_id: ep.webhook_id.clone(),
        tenant_id: "ten_a".to_string(),
        signature: sign(&rotated.secret, &payload),
        idempotency_key: String::new(),
        payload: payload.clone(),
    });
    assert!(result.is_ok());
    assert_eq!(rec.status, TriggerStatus::Fired);

    // Cross-tenant rotate is rejected.
    assert_eq!(
        m.rotate("ten_b", &ep.webhook_id).unwrap_err(),
        WebhookError::CrossTenant
    );
}

#[test]
fn disable_is_tenant_scoped() {
    let m = Manager::new("test", None, None);
    let ep = m
        .create("ten_a", "hook", TargetKind::Run, "run_1")
        .unwrap()
        .endpoint;
    assert_eq!(
        m.disable("ten_b", &ep.webhook_id).unwrap_err(),
        WebhookError::CrossTenant
    );
    assert_eq!(
        m.disable("missing", "missing_hook").unwrap_err(),
        WebhookError::EndpointNotFound
    );
    let disabled = m.disable("ten_a", &ep.webhook_id).unwrap();
    assert_eq!(disabled.status, Status::Disabled);
    assert_eq!(
        m.get("ten_a", &ep.webhook_id).unwrap().status,
        Status::Disabled
    );
}

#[test]
fn trigger_signed_resolves_tenant() {
    let (m, firer, ep, secret) = setup();
    let payload = br#"{"e":1}"#.to_vec();
    let (rec, result) = m.trigger_signed(
        &ep.webhook_id,
        &sign(&secret, &payload),
        "k",
        payload.clone(),
    );
    assert!(result.is_ok());
    assert_eq!(rec.status, TriggerStatus::Fired);
    assert_eq!(rec.tenant_id, "ten_a");
    assert_eq!(firer.fired.load(Ordering::SeqCst), 1);
}

#[test]
fn wire_round_trip() {
    let m = Manager::new("test", None, None);
    let created = m
        .create("ten_a", "hook", TargetKind::Routine, "routine_1")
        .unwrap();
    let json = serde_json::to_string(&created.endpoint).unwrap();
    for key in [
        "\"webhookId\"",
        "\"tenantId\"",
        "\"environmentScope\"",
        "\"targetKind\"",
        "\"targetRef\"",
        "\"secretFingerprint\"",
        "\"secretVersion\"",
        "\"createdAt\"",
    ] {
        assert!(json.contains(key), "missing {key} in {json}");
    }
    assert!(json.contains("\"routine\""));
    let decoded: Endpoint = serde_json::from_str(&json).unwrap();
    assert_eq!(decoded, created.endpoint);

    let (rec, _) = m.trigger(TriggerInput {
        webhook_id: created.endpoint.webhook_id.clone(),
        tenant_id: "ten_a".to_string(),
        signature: sign(&created.secret, b"{}"),
        idempotency_key: "k".to_string(),
        payload: b"{}".to_vec(),
    });
    let json = serde_json::to_string(&rec).unwrap();
    assert!(json.contains("\"payloadBytes\":2"));
    assert!(json.contains("\"idempotencyKey\":\"k\""));
    let decoded: TriggerRecord = serde_json::from_str(&json).unwrap();
    assert_eq!(decoded, rec);
}

#[test]
fn sign_is_deterministic() {
    let secret = "aabbccddeeff00112233445566778899aabbccddeeff00112233445566778899";
    let payload = b"hello";
    assert_eq!(sign(secret, payload), sign(secret, payload));
    assert_ne!(sign(secret, payload), sign(secret, b"hello!"));
    assert_eq!(sign("not-hex!", payload), String::new());
}

#[test]
fn persistence_round_trip() {
    let dir = temp_dir("persist");
    let webhook_id;
    let secret;
    {
        let store = Arc::new(parking_lot::Mutex::new(
            SQLiteStore::new(&dir.to_string_lossy()).unwrap(),
        ));
        let mut m = Manager::new("test", None, None);
        m.with_store(Arc::clone(&store));
        let created = m
            .create("ten_a", "hook", TargetKind::Routine, "routine_1")
            .unwrap();
        webhook_id = created.endpoint.webhook_id.clone();
        secret = created.secret;
    }
    {
        let store = Arc::new(parking_lot::Mutex::new(
            SQLiteStore::new(&dir.to_string_lossy()).unwrap(),
        ));
        let mut m = Manager::new("test", None, None);
        m.with_store(Arc::clone(&store));
        m.load_from_store().unwrap();
        let ep = m
            .get("ten_a", &webhook_id)
            .expect("endpoint survived restart");
        assert_eq!(ep.name, "hook");
        assert_eq!(ep.status, Status::Active);
        // The signing secret must survive so a signature still verifies after restart.
        let payload = br#"{"e":1}"#.to_vec();
        let (rec, result) = m.trigger(TriggerInput {
            webhook_id: webhook_id.clone(),
            tenant_id: "ten_a".to_string(),
            signature: sign(&secret, &payload),
            idempotency_key: "k".to_string(),
            payload,
        });
        assert!(result.is_ok());
        assert_eq!(rec.status, TriggerStatus::Fired);
    }
}
/// Compile-time guard: this manager must be usable from axum `AppState` (Send + Sync).
#[test]
fn manager_is_send_sync() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<kura_webhook::Manager>();
}
