//! Port of the daemon/internal/store core (the SQLite connection + schema migration
//! framework). The 55 schema versions and per-domain CRUD methods are ported incrementally;
//! this crate establishes the connection, pragma configuration, and migration runner.

use std::path::Path;

use chrono::SecondsFormat;
use rusqlite::{params, Connection};

pub mod billing;
pub mod bindings;
pub mod calendar;
pub mod channel_management;
mod computeruse;
pub mod connectors;
pub mod consumer_policy;
mod crud;
pub mod delivery;
pub mod discord_setup;
pub mod evaluation;
mod evaluation_product;
mod events;
mod identity;
mod integration_diagnostics;
mod integrations;
pub mod live_validation;
pub mod mail;
mod manager_documents;
pub mod matrix_setup;
pub mod mcp;
pub mod memory;
mod migrations;
mod policy;
pub mod profiles;
mod providers;
mod records;
mod registry;
pub mod reminders;
pub mod schedule;
pub mod secret_scope;
pub mod secrets;
pub mod slack_setup;
mod tenancy;
pub mod setupwizard;
pub mod telegram_setup;
pub mod thread_continuity;
pub mod thread_handoff;
pub mod thread_persistence;
pub mod workspaces;
pub mod workflow;

pub use billing::BillingRepositoryHandle;
pub use computeruse::ComputerUseStoreHandle;
pub use channel_management::{
    BackgroundDeliveryOutcome, ConnectorAuditRecord, EnablementState, ForegroundReplyOutcome,
    ManagementState, RepairAction, RouteDecisionOutcome, RoutePolicy, RoutingDecision,
    SupportEvidenceBundle,
};
pub use consumer_policy::ConsumerPolicyRecordRecord;
pub use discord_setup::{
    DiscordDestinationValidationRecord, DiscordHostedSetupRecord, DiscordSmokeEvidenceRecord,
};
pub use evaluation::EvaluationStoreHandle;
pub use live_validation::LiveValidationStoreHandle;
pub use setupwizard::SetupWizardStoreHandle;
pub use manager_documents::{delete_document, list_documents, put_document, ManagerDocument};
pub use matrix_setup::{
    MatrixConversationRouteRecord, MatrixEventEvidenceRecord, MatrixHomeserverBindingRecord,
    MatrixHostedSetupRecord, MatrixRoutePolicyRecord, MatrixSmokeEvidenceRecord,
};
pub use records::SandboxExecutionRecord;
pub use secrets::SecretStoreHandle;
pub use secret_scope::SecretScopeBindingRecord;
pub use slack_setup::{
    SlackConversationRouteRecord, SlackEventEvidenceRecord, SlackHostedSetupRecord,
    SlackRoutePolicyRecord, SlackSmokeEvidenceRecord, SlackWorkspaceBinding,
};
pub use telegram_setup::{
    ConnectorAccountBindingSummary, TelegramAllowmentRecord, TelegramHostedSetupRecord,
    TelegramSmokeEvidenceRecord, TelegramUpdateEvidenceRecord,
};
pub use thread_persistence::ThreadListQuery;

/// The production schema head: the first-release baseline. The 55
/// development-era migrations were collapsed into it (see migrations.rs);
/// future migrations append as 2, 3, ...
///
/// Must equal the highest `version` in `schema_migrations()`, or a database
/// written by this build is rejected on reopen as "newer than supported".
pub const CURRENT_SCHEMA_VERSION: i64 = 4;

/// The last development-era schema version before the baseline collapse.
/// Databases stamped exactly at this legacy head hold a schema identical to
/// the baseline and are re-stamped in place; anything older predates the
/// first release and must be re-initialized.
pub const LEGACY_DEV_SCHEMA_HEAD: i64 = 55;

const DEFAULT_DATABASE_FILE: &str = "daemon.sqlite";

/// One schema migration: a monotonically increasing version plus the SQL statements applied in
/// order within a single transaction.
#[derive(Debug, Clone, Default)]
pub struct SchemaMigration {
    pub version: i64,
    pub name: String,
    pub statements: Vec<String>,
}

/// The ordered schema migration list (see migrations.rs).
#[must_use]
pub fn schema_migrations() -> Vec<SchemaMigration> {
    migrations::schema_migrations()
}

pub struct SQLiteStore {
    data_dir: String,
    db_path: String,
    conn: Connection,
}

/// Apply one migration statement, tolerating a column that is already there.
///
/// Every statement in this chain has to be safe to run twice: a pre-release
/// database stamped at the legacy head is re-stamped as the baseline and then
/// has every post-baseline migration replayed against a schema that already
/// contains them. `CREATE TABLE`/`CREATE INDEX` say `IF NOT EXISTS` and so
/// survive that; SQLite has no `ADD COLUMN IF NOT EXISTS`, and without this a
/// migration that adds one fails the replay and the database will not open.
///
/// Narrow on purpose: only this error, and only for `ADD COLUMN`. Anything
/// else is a real migration failure and still stops the boot.
fn apply_migration_statement(
    tx: &rusqlite::Connection,
    migration: &SchemaMigration,
    statement: &str,
) -> Result<(), String> {
    match tx.execute_batch(statement) {
        Ok(()) => Ok(()),
        Err(error) => {
            let message = error.to_string();
            let adds_column = statement.to_ascii_uppercase().contains("ADD COLUMN");
            if adds_column && message.contains("duplicate column name") {
                return Ok(());
            }
            Err(format!(
                "apply schema migration {} ({}): {message}",
                migration.version, migration.name
            ))
        }
    }
}

impl SQLiteStore {
    pub fn new(data_dir: &str) -> Result<Self, String> {
        let resolved = resolve_data_dir(data_dir)?;
        std::fs::create_dir_all(&resolved).map_err(|e| format!("create data dir: {e}"))?;
        let db_path = Path::new(&resolved).join(DEFAULT_DATABASE_FILE);
        let db_path = db_path.to_string_lossy().to_string();
        let conn = Connection::open(&db_path).map_err(|e| format!("open sqlite db: {e}"))?;
        let store = SQLiteStore { data_dir: resolved, db_path, conn };
        store.configure()?;
        store.migrate()?;
        Ok(store)
    }

    /// Opens a store and migrates only up through `target_version`. Test-only helper that
    /// mirrors the Go `NewSQLiteStoreAtVersion` (used by the migration fixture builder to
    /// produce a pre-tenant database before applying the head migrations).
    pub fn new_at_version(data_dir: &str, target_version: i64) -> Result<Self, String> {
        let resolved = resolve_data_dir(data_dir)?;
        std::fs::create_dir_all(&resolved).map_err(|e| format!("create data dir: {e}"))?;
        let db_path = Path::new(&resolved).join(DEFAULT_DATABASE_FILE);
        let db_path = db_path.to_string_lossy().to_string();
        let conn = Connection::open(&db_path).map_err(|e| format!("open sqlite db: {e}"))?;
        let store = SQLiteStore { data_dir: resolved, db_path, conn };
        store.configure()?;
        store.migrate_to_version(target_version)?;
        Ok(store)
    }

    #[must_use]
    pub fn data_dir(&self) -> &str {
        &self.data_dir
    }

    #[must_use]
    pub fn db_path(&self) -> &str {
        &self.db_path
    }

    /// The schema version currently applied to this database.
    pub fn schema_version(&self) -> Result<i64, String> {
        current_schema_version(&self.conn)
    }

    /// Applies the SQLite pragmas used by the Go store.
    fn configure(&self) -> Result<(), String> {
        self.conn
            .execute_batch(
                "PRAGMA foreign_keys = ON;
                 PRAGMA journal_mode = WAL;
                 PRAGMA busy_timeout = 5000;",
            )
            .map_err(|e| format!("apply pragmas: {e}"))
    }

    /// Ensures the bookkeeping table and applies any migrations newer than the current version.
    fn migrate(&self) -> Result<(), String> {
        let tx = self.conn.unchecked_transaction().map_err(|e| format!("begin migration: {e}"))?;
        tx.execute_batch(
            "CREATE TABLE IF NOT EXISTS schema_migrations (
                version INTEGER PRIMARY KEY,
                name TEXT NOT NULL,
                applied_at TEXT NOT NULL
            );",
        )
        .map_err(|e| format!("ensure schema_migrations table: {e}"))?;

        let mut current = current_schema_version(&tx)?;
        if current == LEGACY_DEV_SCHEMA_HEAD {
            // Pre-release development database at the legacy head: its schema
            // is byte-identical to the baseline (the baseline is the legacy
            // chain's final product), so re-stamp it in place.
            tx.execute("DELETE FROM schema_migrations", [])
                .map_err(|e| format!("clear legacy schema migrations: {e}"))?;
            record_schema_migration(&tx, 1, "baseline_v1_first_release")?;
            current = 1;
        }
        if current > CURRENT_SCHEMA_VERSION {
            return Err(format!(
                "database schema version {current} is newer than supported version {CURRENT_SCHEMA_VERSION}; \
                 pre-release development databases older than the legacy head ({LEGACY_DEV_SCHEMA_HEAD}) are \
                 not upgradable — re-initialize the data directory"
            ));
        }

        for migration in schema_migrations() {
            if migration.version <= current {
                continue;
            }
            for statement in &migration.statements {
                apply_migration_statement(&tx, &migration, statement)?;
            }
            record_schema_migration(&tx, migration.version, &migration.name)?;
            current = migration.version;
        }

        tx.commit().map_err(|e| format!("commit migration transaction: {e}"))
    }

    /// Applies schema migrations only up through `target_version`, stopping before any later
    /// migration. Test-only helper mirroring the Go `MigrateToVersion`.
    pub fn migrate_to_version(&self, target_version: i64) -> Result<(), String> {
        if target_version < 1 {
            return Err(format!("migrate to version: invalid target {target_version}"));
        }
        let tx = self.conn.unchecked_transaction().map_err(|e| format!("begin migration: {e}"))?;
        tx.execute_batch(
            "CREATE TABLE IF NOT EXISTS schema_migrations (
                version INTEGER PRIMARY KEY,
                name TEXT NOT NULL,
                applied_at TEXT NOT NULL
            );",
        )
        .map_err(|e| format!("ensure schema_migrations table: {e}"))?;

        let current = current_schema_version(&tx)?;
        for migration in schema_migrations() {
            if migration.version <= current {
                continue;
            }
            if migration.version > target_version {
                break;
            }
            for statement in &migration.statements {
                apply_migration_statement(&tx, &migration, statement)?;
            }
            record_schema_migration(&tx, migration.version, &migration.name)?;
        }
        tx.commit().map_err(|e| format!("commit migration transaction: {e}"))
    }
}

fn current_schema_version(conn: &Connection) -> Result<i64, String> {
    let version: Option<i64> = conn
        .query_row("SELECT MAX(version) FROM schema_migrations", [], |row| row.get(0))
        .map_err(|e| format!("load current schema version: {e}"))?;
    Ok(version.unwrap_or(0))
}

fn record_schema_migration(conn: &Connection, version: i64, name: &str) -> Result<(), String> {
    let applied_at = chrono::Utc::now().to_rfc3339_opts(SecondsFormat::Nanos, true);
    conn.execute(
        "INSERT INTO schema_migrations (version, name, applied_at)
         VALUES (?1, ?2, ?3)
         ON CONFLICT(version) DO UPDATE SET name = excluded.name, applied_at = excluded.applied_at",
        params![version, name, applied_at],
    )
    .map_err(|e| format!("record schema migration {version}: {e}"))?;
    Ok(())
}

fn resolve_data_dir(data_dir: &str) -> Result<String, String> {
    if data_dir.is_empty() {
        return Err("data dir is required".to_string());
    }
    if data_dir == "~" || data_dir.starts_with("~/") {
        let home = std::env::var("HOME").map_err(|_| "resolve user home: HOME is not set".to_string())?;
        if data_dir == "~" {
            return Ok(home);
        }
        return Ok(Path::new(&home).join(&data_dir[2..]).to_string_lossy().to_string());
    }
    Ok(data_dir.to_string())
}