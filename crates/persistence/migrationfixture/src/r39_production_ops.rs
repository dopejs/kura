//! Roadmap 39 production-ops fixture (port of
//! daemon/internal/store/migrationfixture/r39_production_ops.go): a standalone
//! SQLite file (NOT the store schema) holding tenants, secret refs, and work
//! items, used to exercise restore-path integrity: tenant state survives, raw
//! credential material never lands in restored rows.

use std::io::{Read, Write};
use std::path::Path;

use rusqlite::{Connection, params};

/// Roadmap 39 fixture expectations (Go R39ProductionOpsFixture).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct R39ProductionOpsFixture {
    pub tenants: Vec<R39TenantState>,
    pub raw_credential_values: Vec<String>,
    pub expected_record_checks: usize,
}

/// One tenant's production-ops state snapshot.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct R39TenantState {
    pub tenant_id: String,
    pub credential_refs: Vec<String>,
    pub quota_state: String,
    pub work_state: String,
    pub reconnect_required: bool,
    pub operator_action_needed: bool,
}

/// Static fixture expectations (Go BuildR39ProductionOpsFixture).
#[must_use]
pub fn build_r39_production_ops_fixture() -> R39ProductionOpsFixture {
    R39ProductionOpsFixture {
        tenants: vec![
            R39TenantState {
                tenant_id: "ten_ops_alpha".to_string(),
                credential_refs: vec![
                    "secretref_calendar_alpha".to_string(),
                    "secretref_provider_alpha".to_string(),
                ],
                quota_state: "usage_10_of_100".to_string(),
                work_state: "runtime_delivery_completed".to_string(),
                reconnect_required: false,
                operator_action_needed: false,
            },
            R39TenantState {
                tenant_id: "ten_ops_beta".to_string(),
                credential_refs: vec!["secretref_mail_beta".to_string()],
                quota_state: "usage_40_of_100".to_string(),
                work_state: "scheduled_work_pending".to_string(),
                reconnect_required: false,
                operator_action_needed: false,
            },
            R39TenantState {
                tenant_id: "ten_ops_gamma".to_string(),
                credential_refs: vec!["secretref_gamma_reconnect".to_string()],
                quota_state: "usage_95_of_100".to_string(),
                work_state: "retry_exhausted_operator_action_needed".to_string(),
                reconnect_required: true,
                operator_action_needed: true,
            },
        ],
        raw_credential_values: Vec::new(),
        expected_record_checks: 12,
    }
}

/// Builds the standalone r39 SQLite fixture file (Go BuildR39ProductionOpsSQLiteFixture).
pub fn build_r39_production_ops_sqlite_fixture(
    db_path: &str,
) -> Result<R39ProductionOpsFixture, String> {
    let fixture = build_r39_production_ops_fixture();
    create_parent_dir(db_path)?;
    let conn = Connection::open(db_path).map_err(|e| format!("open sqlite fixture: {e}"))?;

    let statements = [
        "CREATE TABLE r39_tenants (tenant_id TEXT PRIMARY KEY, quota_state TEXT NOT NULL, work_state TEXT NOT NULL, reconnect_required INTEGER NOT NULL, operator_action_needed INTEGER NOT NULL)",
        "CREATE TABLE r39_secret_refs (tenant_id TEXT NOT NULL, secret_ref TEXT NOT NULL, reconnect_required INTEGER NOT NULL, PRIMARY KEY (tenant_id, secret_ref))",
        "CREATE TABLE r39_work_items (work_id TEXT PRIMARY KEY, tenant_id TEXT NOT NULL, state TEXT NOT NULL)",
    ];
    for statement in statements {
        conn.execute(statement, [])
            .map_err(|e| format!("create r39 fixture schema: {e}"))?;
    }

    for tenant in &fixture.tenants {
        conn.execute(
            "INSERT INTO r39_tenants (tenant_id, quota_state, work_state, reconnect_required, operator_action_needed) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![tenant.tenant_id, tenant.quota_state, tenant.work_state, bool_int(tenant.reconnect_required), bool_int(tenant.operator_action_needed)],
        )
        .map_err(|e| format!("insert tenant {}: {e}", tenant.tenant_id))?;
        for secret_ref in &tenant.credential_refs {
            if contains_r39_raw_credential(secret_ref) {
                return Err(format!(
                    "credential ref for {} contains raw material",
                    tenant.tenant_id
                ));
            }
            conn.execute(
                "INSERT INTO r39_secret_refs (tenant_id, secret_ref, reconnect_required) VALUES (?1, ?2, ?3)",
                params![tenant.tenant_id, secret_ref, bool_int(tenant.reconnect_required)],
            )
            .map_err(|e| format!("insert secret ref for {}: {e}", tenant.tenant_id))?;
        }
        conn.execute(
            "INSERT INTO r39_work_items (work_id, tenant_id, state) VALUES (?1, ?2, ?3)",
            params![
                format!("work_{}", tenant.tenant_id),
                tenant.tenant_id,
                tenant.work_state
            ],
        )
        .map_err(|e| format!("insert work for {}: {e}", tenant.tenant_id))?;
    }
    Ok(fixture)
}

/// Copies the built fixture to a restore destination with 0o600 permissions
/// (Go CopyR39ProductionOpsSQLiteFixture).
pub fn copy_r39_production_ops_sqlite_fixture(
    source_path: &str,
    dest_path: &str,
) -> Result<(), String> {
    create_parent_dir(dest_path)?;
    let mut source =
        std::fs::File::open(source_path).map_err(|e| format!("open source sqlite fixture: {e}"))?;
    let mut bytes = Vec::new();
    source
        .read_to_end(&mut bytes)
        .map_err(|e| format!("read source sqlite fixture: {e}"))?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create(true).truncate(true).mode(0o600);
        let mut dest = options
            .open(dest_path)
            .map_err(|e| format!("create restored sqlite fixture: {e}"))?;
        dest.write_all(&bytes)
            .map_err(|e| format!("copy sqlite fixture: {e}"))?;
    }
    #[cfg(not(unix))]
    {
        std::fs::write(dest_path, &bytes).map_err(|e| format!("copy sqlite fixture: {e}"))?;
    }
    Ok(())
}

/// Validates a restored r39 fixture (Go ValidateR39ProductionOpsSQLiteRestore).
pub fn validate_r39_production_ops_sqlite_restore(
    db_path: &str,
    expected: &R39ProductionOpsFixture,
) -> Result<(), String> {
    let conn =
        Connection::open(db_path).map_err(|e| format!("open restored sqlite fixture: {e}"))?;

    let integrity: String = conn
        .query_row("PRAGMA integrity_check", [], |row| row.get(0))
        .map_err(|e| format!("run sqlite integrity_check: {e}"))?;
    if integrity != "ok" {
        return Err(format!("sqlite integrity_check failed: {integrity}"));
    }

    let tenant_count: i64 = conn
        .query_row("SELECT COUNT(*) FROM r39_tenants", [], |row| row.get(0))
        .map_err(|e| format!("count tenants: {e}"))?;
    if tenant_count != expected.tenants.len() as i64 {
        return Err(format!(
            "tenant count mismatch: got {tenant_count} want {}",
            expected.tenants.len()
        ));
    }

    for tenant in &expected.tenants {
        let (quota_state, work_state, reconnect_required): (String, String, i64) = conn
            .query_row(
                "SELECT quota_state, work_state, reconnect_required FROM r39_tenants WHERE tenant_id = ?1",
                params![tenant.tenant_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .map_err(|e| format!("read tenant {}: {e}", tenant.tenant_id))?;
        if quota_state != tenant.quota_state || work_state != tenant.work_state {
            return Err(format!(
                "tenant {} state mismatch after restore",
                tenant.tenant_id
            ));
        }
        if bool_int(tenant.reconnect_required) != reconnect_required {
            return Err(format!(
                "tenant {} reconnect state mismatch after restore",
                tenant.tenant_id
            ));
        }
        for secret_ref in &tenant.credential_refs {
            let exists: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM r39_secret_refs WHERE tenant_id = ?1 AND secret_ref = ?2",
                    params![tenant.tenant_id, secret_ref],
                    |row| row.get(0),
                )
                .map_err(|e| format!("read secret ref for {}: {e}", tenant.tenant_id))?;
            if exists != 1 {
                return Err(format!(
                    "secret ref {secret_ref} for tenant {} missing after restore",
                    tenant.tenant_id
                ));
            }
        }
    }
    validate_r39_no_raw_credential_rows(&conn)
}

fn validate_r39_no_raw_credential_rows(conn: &Connection) -> Result<(), String> {
    let mut stmt = conn
        .prepare("SELECT secret_ref FROM r39_secret_refs")
        .map_err(|e| format!("scan secret refs: {e}"))?;
    let rows = stmt
        .query_map([], |row| row.get::<_, String>(0))
        .map_err(|e| format!("scan secret refs: {e}"))?;
    for row in rows {
        let secret_ref = row.map_err(|e| format!("scan secret ref: {e}"))?;
        if contains_r39_raw_credential(&secret_ref) {
            return Err("restored secret ref contains raw credential material".to_string());
        }
    }
    Ok(())
}

fn bool_int(value: bool) -> i64 {
    if value { 1 } else { 0 }
}

fn contains_r39_raw_credential(value: &str) -> bool {
    let lower = value.to_lowercase();
    [
        "raw_secret",
        "access_token",
        "refresh_token",
        "oauth_code",
        "provider_token",
        "do_not_leak",
    ]
    .iter()
    .any(|marker| lower.contains(marker))
}

fn create_parent_dir(path: &str) -> Result<(), String> {
    let parent = Path::new(path)
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    std::fs::create_dir_all(parent).map_err(|e| format!("create fixture dir: {e}"))
}
