//! SQLite-backed adapters for the activation dependency seams (wave 8 parity).
//!
//! Port of the Go wiring in `daemon/internal/app/app.go`:
//!
//! - [`SqliteActivationStore`] implements [`crate::StateStore`],
//!   [`crate::IdentityRepository`], and [`crate::AuditSink`] over a
//!   `kura_store::SQLiteStore`, mirroring Go's `*SQLiteStore` satisfying all
//!   three interfaces: the identity/audit methods delegate to the store's
//!   identity DAOs, and the activation-state table (`activation_states`,
//!   created by store migration 45) is written through a dedicated rusqlite
//!   connection (the table has no DAO yet, matching the Go `store/activation.go`
//!   SQL directly).
//! - [`BillingProjectorAdapter`] projects the quota baseline from
//!   `kura_billing::Manager::usage_summary` (Go `billingManager`).
//! - [`ChatRunnerAdapter`] runs the hosted activation test chat through the
//!   `kura_chat::Service` with the builtin `echo` provider (Go
//!   `activationChatRunner`).

use std::sync::Arc;

use chrono::DateTime;
use chrono::SecondsFormat;
use chrono::Utc;
use kura_billing::BillingError;
use kura_billing::UsageSummary;
use kura_identity::Membership;
use kura_identity::MembershipFilter;
use kura_identity::Principal;
use kura_identity::PrincipalFilter;
use kura_identity::Tenant;
use kura_identity::TenantAuditEvent;
use kura_identity::TenantFilter;
use kura_identity::TokenTenantGrant;
use kura_store::SQLiteStore;
use rusqlite::Connection;
use serde::Serialize;
use serde_json::Map;
use serde_json::Value;

use crate::AuditSink;
use crate::BillingProjector;
use crate::ChatRunFailure;
use crate::ChatRunner;
use crate::IdentityRepository;
use crate::StateStore;
use crate::StoreError;
use crate::TestChatInput;
use crate::TestChatResult;
use crate::service::BoxFuture;
use crate::types::FailureReason;
use crate::types::FirstAction;
use crate::types::QuotaBaseline;
use crate::types::ReadinessItem;
use crate::types::State;
use crate::types::Status;
use crate::types::TestChatMetadata;

const DEFAULT_ACTIVATION_TEST_CHAT_MESSAGE: &str = "Run a safe hosted activation test.";

fn store_err(message: String) -> StoreError {
    message.into()
}

// ---------------------------------------------------------------------------
// SQLite activation state table (Go store/activation.go)
// ---------------------------------------------------------------------------

/// Ensures the `activation_states` table (store migration 45 / Go v40) exists,
/// so the adapter is usable even when the schema migration runner has not run
/// yet. Idempotent; matches the migration DDL exactly.
fn ensure_activation_state_table(conn: &Connection) -> Result<(), String> {
    conn.execute_batch(
        r#"CREATE TABLE IF NOT EXISTS activation_states (
            activation_id TEXT PRIMARY KEY,
            principal_id TEXT NOT NULL,
            tenant_id TEXT NOT NULL,
            environment_scope TEXT NOT NULL,
            status TEXT NOT NULL,
            current_step_id TEXT NOT NULL,
            completed_step_ids_json TEXT NOT NULL,
            blocking_reason_codes_json TEXT NOT NULL,
            readiness_items_json TEXT NOT NULL,
            quota_baseline_json TEXT,
            first_action_json TEXT NOT NULL,
            test_chat_json TEXT,
            failure_reason_json TEXT,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL,
            first_action_completed_at TEXT,
            last_evaluated_at TEXT NOT NULL,
            last_transition_audit_event_id TEXT,
            metadata_json TEXT,
            UNIQUE(principal_id, tenant_id)
        );
        CREATE INDEX IF NOT EXISTS idx_activation_states_tenant_status
            ON activation_states(tenant_id, status, updated_at DESC, activation_id DESC);
        CREATE INDEX IF NOT EXISTS idx_activation_states_principal_updated
            ON activation_states(principal_id, updated_at DESC, activation_id DESC);"#,
    )
    .map_err(|e| format!("ensure activation_states table: {e}"))
}

/// Go `validateActivationStateForStorage`.
fn validate_activation_state_for_storage(state: &State) -> Result<(), String> {
    if state.activation_id.trim().is_empty() {
        return Err("activation state activation id is required".to_string());
    }
    if state.principal_id.trim().is_empty() {
        return Err(format!(
            "activation state {} principal id is required",
            state.activation_id
        ));
    }
    if state.tenant_id.trim().is_empty() {
        return Err(format!(
            "activation state {} tenant id is required",
            state.activation_id
        ));
    }
    if state.environment_scope.trim().is_empty() {
        return Err(format!(
            "activation state {} environment scope is required",
            state.activation_id
        ));
    }
    if state.status.is_empty() {
        return Err(format!(
            "activation state {} status is required",
            state.activation_id
        ));
    }
    if state.current_step_id.trim().is_empty() {
        return Err(format!(
            "activation state {} current step id is required",
            state.activation_id
        ));
    }
    if state.first_action.action_id.trim().is_empty()
        || state.first_action.action_kind.trim().is_empty()
    {
        return Err(format!(
            "activation state {} first action is required",
            state.activation_id
        ));
    }
    if state.created_at == DateTime::<Utc>::UNIX_EPOCH || is_go_zero_time(state.created_at) {
        return Err(format!(
            "activation state {} created at is required",
            state.activation_id
        ));
    }
    if state.updated_at == DateTime::<Utc>::UNIX_EPOCH || is_go_zero_time(state.updated_at) {
        return Err(format!(
            "activation state {} updated at is required",
            state.activation_id
        ));
    }
    if state.last_evaluated_at == DateTime::<Utc>::UNIX_EPOCH
        || is_go_zero_time(state.last_evaluated_at)
    {
        return Err(format!(
            "activation state {} last evaluated at is required",
            state.activation_id
        ));
    }
    Ok(())
}

/// Go `time.Time.IsZero()`: the Go zero time is 0001-01-01T00:00:00Z.
fn is_go_zero_time(at: DateTime<Utc>) -> bool {
    at == DateTime::<Utc>::MIN_UTC
}

/// Go `time.RFC3339Nano` on a UTC time.
fn rfc3339(at: DateTime<Utc>) -> String {
    at.to_rfc3339_opts(SecondsFormat::Nanos, true)
}

/// Parses a Go `time.RFC3339Nano` timestamp (trailing zeros optional).
fn parse_rfc3339(value: &str) -> Result<DateTime<Utc>, String> {
    DateTime::parse_from_rfc3339(value)
        .map(|at| at.with_timezone(&Utc))
        .map_err(|e| format!("parse rfc3339 timestamp {value:?}: {e}"))
}

/// Go `requiredJSON`.
fn required_json<T: Serialize>(value: &T) -> Result<String, String> {
    serde_json::to_string(value).map_err(|e| format!("encode activation JSON: {e}"))
}

/// Go `marshalJSON` for a nullable value: nil maps to NULL.
fn opt_json<T: Serialize>(value: &Option<T>) -> Result<Option<String>, String> {
    match value {
        Some(value) => required_json(value).map(Some),
        None => Ok(None),
    }
}

/// Go `nullableTimeString`.
fn opt_time_string(at: Option<DateTime<Utc>>) -> Option<String> {
    at.map(rfc3339)
}

fn parse_opt_time(value: Option<String>) -> Result<Option<DateTime<Utc>>, String> {
    match value {
        None => Ok(None),
        Some(value) => parse_rfc3339(&value).map(Some),
    }
}

/// Go `unmarshalNullableJSON` for a nullable JSON column.
fn decode_opt_json<T: serde::de::DeserializeOwned>(
    value: Option<String>,
) -> Result<Option<T>, String> {
    match value {
        None => Ok(None),
        Some(value) => serde_json::from_str::<Option<T>>(&value)
            .map_err(|e| format!("decode activation JSON: {e}")),
    }
}

fn upsert_activation_state(conn: &Connection, state: &State) -> Result<(), String> {
    validate_activation_state_for_storage(state)?;
    let completed_steps_json = required_json(&state.completed_step_ids)?;
    let blocking_reasons_json = required_json(&state.blocking_reason_codes)?;
    let readiness_items_json = required_json(&state.readiness_items)?;
    let quota_baseline_json = opt_json(&state.quota_baseline)?;
    let first_action_json = required_json(&state.first_action)?;
    let test_chat_json = opt_json(&state.test_chat)?;
    let failure_reason_json = opt_json(&state.failure_reason)?;
    let metadata_json = opt_json(&state.metadata)?;

    conn.execute(
        r#"INSERT INTO activation_states (
            activation_id, principal_id, tenant_id, environment_scope, status, current_step_id,
            completed_step_ids_json, blocking_reason_codes_json, readiness_items_json,
            quota_baseline_json, first_action_json, test_chat_json, failure_reason_json,
            created_at, updated_at, first_action_completed_at, last_evaluated_at,
            last_transition_audit_event_id, metadata_json
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19)
        ON CONFLICT(principal_id, tenant_id) DO UPDATE SET
            activation_id = excluded.activation_id,
            environment_scope = excluded.environment_scope,
            status = excluded.status,
            current_step_id = excluded.current_step_id,
            completed_step_ids_json = excluded.completed_step_ids_json,
            blocking_reason_codes_json = excluded.blocking_reason_codes_json,
            readiness_items_json = excluded.readiness_items_json,
            quota_baseline_json = excluded.quota_baseline_json,
            first_action_json = excluded.first_action_json,
            test_chat_json = excluded.test_chat_json,
            failure_reason_json = excluded.failure_reason_json,
            created_at = excluded.created_at,
            updated_at = excluded.updated_at,
            first_action_completed_at = excluded.first_action_completed_at,
            last_evaluated_at = excluded.last_evaluated_at,
            last_transition_audit_event_id = excluded.last_transition_audit_event_id,
            metadata_json = excluded.metadata_json"#,
        rusqlite::params![
            state.activation_id,
            state.principal_id,
            state.tenant_id,
            state.environment_scope,
            state.status.to_string(),
            state.current_step_id,
            completed_steps_json,
            blocking_reasons_json,
            readiness_items_json,
            quota_baseline_json,
            first_action_json,
            test_chat_json,
            failure_reason_json,
            rfc3339(state.created_at),
            rfc3339(state.updated_at),
            opt_time_string(state.first_action_completed_at),
            rfc3339(state.last_evaluated_at),
            state.last_transition_audit_event,
            metadata_json,
        ],
    )
    .map_err(|e| {
        format!(
            "upsert activation state {} for principal {} tenant {}: {e}",
            state.activation_id, state.principal_id, state.tenant_id
        )
    })?;
    Ok(())
}

/// Column order mirrors Go `activationStateSelectSQL()`.
const ACTIVATION_STATE_SELECT: &str = r#"SELECT activation_id, principal_id, tenant_id,
    environment_scope, status, current_step_id, completed_step_ids_json,
    blocking_reason_codes_json, readiness_items_json, quota_baseline_json,
    first_action_json, test_chat_json, failure_reason_json, created_at, updated_at,
    first_action_completed_at, last_evaluated_at, last_transition_audit_event_id,
    metadata_json
FROM activation_states"#;

fn scan_activation_state(row: &rusqlite::Row<'_>) -> Result<State, String> {
    let activation_id: String = row.get(0).map_err(|e| e.to_string())?;
    let principal_id: String = row.get(1).map_err(|e| e.to_string())?;
    let tenant_id: String = row.get(2).map_err(|e| e.to_string())?;
    let environment_scope: String = row.get(3).map_err(|e| e.to_string())?;
    let status: String = row.get(4).map_err(|e| e.to_string())?;
    let current_step_id: String = row.get(5).map_err(|e| e.to_string())?;
    let completed_steps_json: String = row.get(6).map_err(|e| e.to_string())?;
    let blocking_reasons_json: String = row.get(7).map_err(|e| e.to_string())?;
    let readiness_items_json: String = row.get(8).map_err(|e| e.to_string())?;
    let quota_baseline_json: Option<String> = row.get(9).map_err(|e| e.to_string())?;
    let first_action_json: String = row.get(10).map_err(|e| e.to_string())?;
    let test_chat_json: Option<String> = row.get(11).map_err(|e| e.to_string())?;
    let failure_reason_json: Option<String> = row.get(12).map_err(|e| e.to_string())?;
    let created_at: String = row.get(13).map_err(|e| e.to_string())?;
    let updated_at: String = row.get(14).map_err(|e| e.to_string())?;
    let first_action_completed_at: Option<String> = row.get(15).map_err(|e| e.to_string())?;
    let last_evaluated_at: String = row.get(16).map_err(|e| e.to_string())?;
    let last_transition_audit_event_id: Option<String> = row.get(17).map_err(|e| e.to_string())?;
    let metadata_json: Option<String> = row.get(18).map_err(|e| e.to_string())?;

    Ok(State {
        activation_id: activation_id.clone(),
        principal_id,
        tenant_id,
        environment_scope,
        status: Status::from(status),
        current_step_id,
        completed_step_ids: serde_json::from_str(&completed_steps_json)
            .map_err(|e| format!("decode activation {activation_id} completed steps: {e}"))?,
        blocking_reason_codes: serde_json::from_str(&blocking_reasons_json)
            .map_err(|e| format!("decode activation {activation_id} blocking reasons: {e}"))?,
        readiness_items: serde_json::from_str::<Vec<ReadinessItem>>(&readiness_items_json)
            .map_err(|e| format!("decode activation {activation_id} readiness items: {e}"))?,
        quota_baseline: decode_opt_json::<QuotaBaseline>(quota_baseline_json)?,
        first_action: serde_json::from_str::<FirstAction>(&first_action_json)
            .map_err(|e| format!("decode activation {activation_id} first action: {e}"))?,
        test_chat: decode_opt_json::<TestChatMetadata>(test_chat_json)?,
        failure_reason: decode_opt_json::<FailureReason>(failure_reason_json)?,
        created_at: parse_rfc3339(&created_at)?,
        updated_at: parse_rfc3339(&updated_at)?,
        first_action_completed_at: parse_opt_time(first_action_completed_at)?,
        last_evaluated_at: parse_rfc3339(&last_evaluated_at)?,
        last_transition_audit_event: last_transition_audit_event_id.unwrap_or_default(),
        metadata: decode_opt_json::<Map<String, Value>>(metadata_json)?,
    })
}

fn get_activation_state(conn: &Connection, activation_id: &str) -> Result<Option<State>, String> {
    let mut stmt = conn
        .prepare(&format!(
            "{ACTIVATION_STATE_SELECT} WHERE activation_id = ?1"
        ))
        .map_err(|e| format!("prepare get activation state: {e}"))?;
    let mut rows = stmt
        .query(rusqlite::params![activation_id])
        .map_err(|e| e.to_string())?;
    match rows.next().map_err(|e| e.to_string())? {
        Some(row) => scan_activation_state(&row).map(Some),
        None => Ok(None),
    }
}

fn get_activation_state_for_principal_tenant(
    conn: &Connection,
    principal_id: &str,
    tenant_id: &str,
) -> Result<Option<State>, String> {
    let mut stmt = conn
        .prepare(&format!(
            "{ACTIVATION_STATE_SELECT} WHERE principal_id = ?1 AND tenant_id = ?2"
        ))
        .map_err(|e| format!("prepare get activation state for principal tenant: {e}"))?;
    let mut rows = stmt
        .query(rusqlite::params![principal_id, tenant_id])
        .map_err(|e| e.to_string())?;
    match rows.next().map_err(|e| e.to_string())? {
        Some(row) => scan_activation_state(&row).map(Some),
        None => Ok(None),
    }
}

// ---------------------------------------------------------------------------
// SqliteActivationStore
// ---------------------------------------------------------------------------

/// One SQLite-backed adapter satisfying the activation [`crate::StateStore`],
/// [`crate::IdentityRepository`], and [`crate::AuditSink`] seams (Go's
/// `*SQLiteStore`).
///
/// Identity and audit methods delegate to the `kura-store` identity DAOs. The
/// activation state table is written through a dedicated rusqlite connection
/// to the same database file (the table is created by store migration 45 but
/// has no DAO yet; the SQL mirrors `daemon/internal/store/activation.go`).
pub struct SqliteActivationStore {
    store: Arc<parking_lot::Mutex<SQLiteStore>>,
    conn: Arc<parking_lot::Mutex<Connection>>,
}

impl SqliteActivationStore {
    /// Opens the adapter over a fresh `kura-store` handle. The store applies
    /// the schema migrations (which create `activation_states`); the adapter
    /// additionally ensures the table exists so it can open the raw
    /// connection before the store migration runner runs.
    pub fn new(store: SQLiteStore) -> Result<Self, String> {
        let conn = Connection::open(store.db_path())
            .map_err(|e| format!("open activation state connection: {e}"))?;
        ensure_activation_state_table(&conn)?;
        Ok(Self {
            store: Arc::new(parking_lot::Mutex::new(store)),
            conn: Arc::new(parking_lot::Mutex::new(conn)),
        })
    }

    /// The shared store handle, for read-side verification (tests / diagnostics).
    #[must_use]
    pub fn store_handle(&self) -> Arc<parking_lot::Mutex<SQLiteStore>> {
        Arc::clone(&self.store)
    }
}

impl StateStore for SqliteActivationStore {
    fn upsert_activation_state(&self, state: State) -> BoxFuture<'_, Result<(), StoreError>> {
        let conn = Arc::clone(&self.conn);
        Box::pin(async move { upsert_activation_state(&conn.lock(), &state).map_err(store_err) })
    }

    fn get_activation_state(
        &self,
        activation_id: &str,
    ) -> BoxFuture<'_, Result<Option<State>, StoreError>> {
        let conn = Arc::clone(&self.conn);
        let activation_id = activation_id.to_string();
        Box::pin(
            async move { get_activation_state(&conn.lock(), &activation_id).map_err(store_err) },
        )
    }

    fn get_activation_state_for_principal_tenant(
        &self,
        principal_id: &str,
        tenant_id: &str,
    ) -> BoxFuture<'_, Result<Option<State>, StoreError>> {
        let conn = Arc::clone(&self.conn);
        let principal_id = principal_id.to_string();
        let tenant_id = tenant_id.to_string();
        Box::pin(async move {
            get_activation_state_for_principal_tenant(&conn.lock(), &principal_id, &tenant_id)
                .map_err(store_err)
        })
    }
}

impl IdentityRepository for SqliteActivationStore {
    fn get_principal(
        &self,
        principal_id: &str,
    ) -> BoxFuture<'_, Result<Option<Principal>, StoreError>> {
        let store = Arc::clone(&self.store);
        let principal_id = principal_id.to_string();
        Box::pin(async move { store.lock().get_principal(&principal_id).map_err(store_err) })
    }

    fn list_principals(
        &self,
        filter: &PrincipalFilter,
    ) -> BoxFuture<'_, Result<Vec<Principal>, StoreError>> {
        let store = Arc::clone(&self.store);
        let filter = filter.clone();
        Box::pin(async move { store.lock().list_principals(&filter).map_err(store_err) })
    }

    fn upsert_principal(&self, principal: Principal) -> BoxFuture<'_, Result<(), StoreError>> {
        let store = Arc::clone(&self.store);
        Box::pin(async move { store.lock().upsert_principal(&principal).map_err(store_err) })
    }

    fn get_tenant(&self, tenant_id: &str) -> BoxFuture<'_, Result<Option<Tenant>, StoreError>> {
        let store = Arc::clone(&self.store);
        let tenant_id = tenant_id.to_string();
        Box::pin(async move { store.lock().get_tenant(&tenant_id).map_err(store_err) })
    }

    fn list_tenants(
        &self,
        filter: &TenantFilter,
    ) -> BoxFuture<'_, Result<Vec<Tenant>, StoreError>> {
        let store = Arc::clone(&self.store);
        let filter = filter.clone();
        Box::pin(async move { store.lock().list_tenants(&filter).map_err(store_err) })
    }

    fn upsert_tenant(&self, tenant: Tenant) -> BoxFuture<'_, Result<(), StoreError>> {
        let store = Arc::clone(&self.store);
        Box::pin(async move { store.lock().upsert_tenant(&tenant).map_err(store_err) })
    }

    fn list_memberships(
        &self,
        filter: &MembershipFilter,
    ) -> BoxFuture<'_, Result<Vec<Membership>, StoreError>> {
        let store = Arc::clone(&self.store);
        let filter = filter.clone();
        Box::pin(async move { store.lock().list_memberships(&filter).map_err(store_err) })
    }

    fn upsert_membership(&self, membership: Membership) -> BoxFuture<'_, Result<(), StoreError>> {
        let store = Arc::clone(&self.store);
        Box::pin(async move {
            store
                .lock()
                .upsert_membership(&membership)
                .map_err(store_err)
        })
    }

    fn list_token_tenant_grants(
        &self,
        token_id: &str,
    ) -> BoxFuture<'_, Result<Vec<TokenTenantGrant>, StoreError>> {
        let store = Arc::clone(&self.store);
        let token_id = token_id.to_string();
        Box::pin(async move {
            store
                .lock()
                .list_token_tenant_grants(&token_id)
                .map_err(store_err)
        })
    }

    fn upsert_token_tenant_grant(
        &self,
        grant: TokenTenantGrant,
    ) -> BoxFuture<'_, Result<(), StoreError>> {
        let store = Arc::clone(&self.store);
        Box::pin(async move {
            store
                .lock()
                .upsert_token_tenant_grant(&grant)
                .map_err(store_err)
        })
    }
}

impl AuditSink for SqliteActivationStore {
    fn append_tenant_audit_event(
        &self,
        event: TenantAuditEvent,
    ) -> BoxFuture<'_, Result<TenantAuditEvent, StoreError>> {
        let store = Arc::clone(&self.store);
        Box::pin(async move {
            store
                .lock()
                .append_tenant_audit_event(&event)
                .map_err(store_err)
        })
    }
}

// ---------------------------------------------------------------------------
// BillingProjectorAdapter
// ---------------------------------------------------------------------------

/// Quota baseline projector backed by `kura_billing::Manager::usage_summary`
/// (Go `billingManager`).
pub struct BillingProjectorAdapter {
    billing: Arc<kura_billing::Manager>,
}

impl BillingProjectorAdapter {
    #[must_use]
    pub fn new(billing: Arc<kura_billing::Manager>) -> Self {
        Self { billing }
    }
}

impl BillingProjector for BillingProjectorAdapter {
    fn usage_summary(
        &self,
        tenant_id: &str,
        hosted: bool,
    ) -> BoxFuture<'_, Result<UsageSummary, BillingError>> {
        let billing = Arc::clone(&self.billing);
        let tenant_id = tenant_id.to_string();
        Box::pin(async move { billing.usage_summary(&tenant_id, hosted).await })
    }
}

// ---------------------------------------------------------------------------
// ChatRunnerAdapter
// ---------------------------------------------------------------------------

/// Runs the hosted activation test chat through the `kura-chat` service with
/// the builtin `echo` provider (Go `activationChatRunner` in
/// `daemon/internal/app/activation_chat.go`).
pub struct ChatRunnerAdapter {
    service: Option<Arc<kura_chat::Service>>,
}

impl ChatRunnerAdapter {
    #[must_use]
    pub fn new(service: Option<Arc<kura_chat::Service>>) -> Self {
        Self { service }
    }
}

impl ChatRunner for ChatRunnerAdapter {
    fn run_activation_test_chat(
        &self,
        input: TestChatInput,
    ) -> BoxFuture<'_, Result<TestChatResult, ChatRunFailure>> {
        let service = self.service.clone();
        Box::pin(async move {
            let Some(service) = service else {
                return Err(ChatRunFailure {
                    result: TestChatResult::default(),
                    message: "chat service is not configured".to_string(),
                });
            };
            let mut message = input.message.trim().to_string();
            if message.is_empty() {
                message = DEFAULT_ACTIVATION_TEST_CHAT_MESSAGE.to_string();
            }
            let (exec, query_err) = match service.query(
                kura_chat::QueryInput {
                    query: message,
                    provider: "echo".to_string(),
                    model: "echo-v1".to_string(),
                    tenant_id: input.tenant_id,
                    ..kura_chat::QueryInput::default()
                },
                &kura_chat::CancellationToken::new(),
            ) {
                Ok(exec) => (exec, None),
                Err(err) => (kura_chat::QueryExecution::default(), Some(err)),
            };
            let dispatch = &exec.result.dispatch;
            let status = if dispatch.status == kura_llm::DispatchStatus::Cancelled {
                crate::TestChatStatus::CANCELLED.into()
            } else if query_err.is_some()
                || matches!(
                    dispatch.status,
                    kura_llm::DispatchStatus::Failed | kura_llm::DispatchStatus::PartialFailed
                )
            {
                crate::TestChatStatus::FAILED.into()
            } else {
                crate::TestChatStatus::COMPLETED.into()
            };
            let completed_at = dispatch.completed_at.unwrap_or_else(Utc::now);
            let result = TestChatResult {
                dispatch_id: dispatch.dispatch_id.clone(),
                status,
                provider: dispatch.provider.clone(),
                model: dispatch.model.clone(),
                usage: Map::from_iter([
                    (
                        "inputTokens".to_string(),
                        Value::from(dispatch.usage.input_tokens),
                    ),
                    (
                        "outputTokens".to_string(),
                        Value::from(dispatch.usage.output_tokens),
                    ),
                    (
                        "totalTokens".to_string(),
                        Value::from(dispatch.usage.total_tokens),
                    ),
                ]),
                finish_reason: dispatch.finish_reason.clone(),
                completed_at: Some(completed_at),
            };
            match query_err {
                Some(err) => Err(ChatRunFailure {
                    result,
                    message: err.to_string(),
                }),
                None => Ok(result),
            }
        })
    }
}
