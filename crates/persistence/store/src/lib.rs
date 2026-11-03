//! Port of the daemon/internal/store core (the SQLite connection + schema migration
//! framework). The 55 schema versions and per-domain CRUD methods are ported incrementally;
//! this crate establishes the connection, pragma configuration, and migration runner.

use std::path::Path;

use chrono::SecondsFormat;
use rusqlite::{Connection, params};

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
pub mod pool;
pub use pool::{PoolStats, ReadGuard, StorePool, WaitStats};
mod devices;
mod policy;
pub mod profiles;
mod providers;
mod records;
mod registry;
pub mod reminders;
pub mod schedule;
pub mod secret_scope;
pub mod secrets;
pub mod setupwizard;
pub mod slack_setup;
pub mod telegram_setup;
mod tenancy;
pub mod thread_continuity;
pub mod thread_handoff;
pub mod thread_persistence;
mod tools;
mod turn_checkpoints;
pub mod work_items;
pub mod workflow;
pub mod workspaces;

pub use billing::BillingRepositoryHandle;
pub use channel_management::{
    BackgroundDeliveryOutcome, ConnectorAuditRecord, EnablementState, ForegroundReplyOutcome,
    ManagementState, RepairAction, RouteDecisionOutcome, RoutePolicy, RoutingDecision,
    SupportEvidenceBundle,
};
pub use computeruse::ComputerUseStoreHandle;
pub use consumer_policy::ConsumerPolicyRecordRecord;
pub use devices::{DevicePairingRecord, DeviceRecord};
pub use discord_setup::{
    DiscordDestinationValidationRecord, DiscordHostedSetupRecord, DiscordSmokeEvidenceRecord,
};
pub use evaluation::EvaluationStoreHandle;
pub use live_validation::LiveValidationStoreHandle;
pub use manager_documents::{ManagerDocument, delete_document, list_documents, put_document};
pub use matrix_setup::{
    MatrixConversationRouteRecord, MatrixEventEvidenceRecord, MatrixHomeserverBindingRecord,
    MatrixHostedSetupRecord, MatrixRoutePolicyRecord, MatrixSmokeEvidenceRecord,
};
pub use records::SandboxExecutionRecord;
pub use secret_scope::SecretScopeBindingRecord;
pub use secrets::SecretStoreHandle;
pub use setupwizard::SetupWizardStoreHandle;
pub use slack_setup::{
    SlackConversationRouteRecord, SlackEventEvidenceRecord, SlackHostedSetupRecord,
    SlackRoutePolicyRecord, SlackSmokeEvidenceRecord, SlackWorkspaceBinding,
};
pub use telegram_setup::{
    ConnectorAccountBindingSummary, TelegramAllowmentRecord, TelegramHostedSetupRecord,
    TelegramSmokeEvidenceRecord, TelegramUpdateEvidenceRecord,
};
pub use thread_persistence::ThreadListQuery;
pub use turn_checkpoints::TurnCheckpointRecord;

/// The production schema head: the first-release baseline. The 55
/// development-era migrations were collapsed into it (see migrations.rs);
/// future migrations append as 2, 3, ...
///
/// Must equal the highest `version` in `schema_migrations()`, or a database
/// written by this build is rejected on reopen as "newer than supported".
pub const CURRENT_SCHEMA_VERSION: i64 = 10;

/// The last development-era schema version before the baseline collapse.
/// Databases stamped exactly at this legacy head hold a schema identical to
/// the baseline and are re-stamped in place; anything older predates the
/// first release and must be re-initialized.
pub const LEGACY_DEV_SCHEMA_HEAD: i64 = 55;

pub const DEFAULT_DATABASE_FILE: &str = "daemon.sqlite";

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

/// How long a connection keeps retrying a lock another connection holds
/// before the statement fails with `SQLITE_BUSY` (as `busy_timeout` was).
const BUSY_LIMIT: std::time::Duration = std::time::Duration::from_secs(5);

/// The busy handler every connection gets: retry after 50 µs, doubling to
/// 1 ms, then every 1 ms, for [`BUSY_LIMIT`] in all.
///
/// SQLite's own (`busy_timeout`) sleeps 1, 2, 5, 10, 15, 20, 25, 25, 25, 50,
/// 50 then 100 ms between tries. A write lock held for a fraction of a
/// millisecond could leave a waiting writer asleep for 20 ms: with tens of
/// connections writing one file, turns waited on sleep rather than on the
/// lock, and the process sat mostly idle (measured: 128 concurrent agent
/// turns used one core of fourteen; agent-runtime-program.md 4.5).
fn retry_busy(attempts: i32) -> bool {
    let step = busy_step(attempts);
    if busy_waited(attempts) + step > BUSY_LIMIT {
        return false;
    }
    std::thread::sleep(step);
    true
}

fn busy_step(attempts: i32) -> std::time::Duration {
    let doubling = u32::try_from(attempts.clamp(0, 5)).unwrap_or(5);
    std::time::Duration::from_micros((50_u64 << doubling).min(1_000))
}

/// Time already spent waiting after `attempts` retries.
fn busy_waited(attempts: i32) -> std::time::Duration {
    (0..attempts.max(0)).map(busy_step).sum()
}

/// SQLite `synchronous` for the connections this process opens. Set once at
/// boot, before the first store opens ([`set_synchronous`]); WAL either way.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Synchronous {
    /// Sync the WAL on every commit (SQLite's default, and this daemon's).
    Full,
    /// Sync the WAL at checkpoints only. Never corrupts the database under
    /// WAL; the last commits before a power loss or OS crash can be lost.
    Normal,
}

impl Synchronous {
    pub fn parse(value: &str) -> Result<Self, String> {
        match value.trim().to_ascii_lowercase().as_str() {
            "" | "full" => Ok(Self::Full),
            "normal" => Ok(Self::Normal),
            other => Err(format!(
                "store synchronous mode {other:?} is not full or normal"
            )),
        }
    }

    #[must_use]
    pub fn pragma(self) -> &'static str {
        match self {
            Self::Full => "FULL",
            Self::Normal => "NORMAL",
        }
    }
}

static SYNCHRONOUS_NORMAL: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

/// Sets the mode every connection opened from now on uses.
pub fn set_synchronous(mode: Synchronous) {
    SYNCHRONOUS_NORMAL.store(
        mode == Synchronous::Normal,
        std::sync::atomic::Ordering::SeqCst,
    );
}

#[must_use]
pub fn synchronous() -> Synchronous {
    if SYNCHRONOUS_NORMAL.load(std::sync::atomic::Ordering::SeqCst) {
        Synchronous::Normal
    } else {
        Synchronous::Full
    }
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
        let store = SQLiteStore {
            data_dir: resolved,
            db_path,
            conn,
        };
        store.configure()?;
        store.migrate()?;
        Ok(store)
    }

    /// Opens an additional **query-only** connection to an existing database
    /// for the reader side of [`StorePool`]. Runs no migrations (the writer
    /// already did) and cannot write: `PRAGMA query_only = ON` makes SQLite
    /// refuse any mutation on this connection.
    pub fn open_reader(data_dir: &str) -> Result<Self, String> {
        let resolved = resolve_data_dir(data_dir)?;
        let db_path = Path::new(&resolved).join(DEFAULT_DATABASE_FILE);
        if !db_path.exists() {
            return Err(format!("no database at {}", db_path.display()));
        }
        let db_path = db_path.to_string_lossy().to_string();
        let conn = Connection::open(&db_path).map_err(|e| format!("open sqlite reader: {e}"))?;
        let store = SQLiteStore {
            data_dir: resolved,
            db_path,
            conn,
        };
        store.configure()?;
        store
            .conn
            .execute_batch("PRAGMA query_only = ON;")
            .map_err(|e| format!("apply reader pragma: {e}"))?;
        Ok(store)
    }

    /// Opens an existing database **without running migrations**.
    ///
    /// The upgrade rehearsal needs to read the source database (its schema
    /// version, and a `VACUUM INTO` snapshot) while leaving it exactly as
    /// the previous binary left it. `new` would migrate it in place — which
    /// is activation, the thing a rehearsal exists to defer.
    pub fn open_unmigrated(data_dir: &str) -> Result<Self, String> {
        let resolved = resolve_data_dir(data_dir)?;
        let db_path = Path::new(&resolved).join(DEFAULT_DATABASE_FILE);
        if !db_path.exists() {
            return Err(format!("no database at {}", db_path.display()));
        }
        let db_path = db_path.to_string_lossy().to_string();
        let conn = Connection::open(&db_path).map_err(|e| format!("open sqlite db: {e}"))?;
        let store = SQLiteStore {
            data_dir: resolved,
            db_path,
            conn,
        };
        store.configure()?;
        Ok(store)
    }

    /// Number of user tables. Zero on a database that lost its data is the
    /// signal `integrity_check` cannot give (D9).
    pub fn table_count(&self) -> Result<i64, String> {
        self.conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite_%'",
                [],
                |row| row.get::<_, i64>(0),
            )
            .map_err(|e| format!("table count: {e}"))
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
        let store = SQLiteStore {
            data_dir: resolved,
            db_path,
            conn,
        };
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
    /// Writes a consistent snapshot of the database to `dest` via
    /// `VACUUM INTO`.
    ///
    /// This is the only correct way to copy a WAL-mode database while it may
    /// be open: `cp daemon.sqlite` captures the main file alone, and in WAL
    /// mode committed data lives in `daemon.sqlite-wal` until a checkpoint.
    /// Defect D9 (2026-09-19): the production backup script did exactly that
    /// and produced backups with zero tables that nonetheless passed
    /// `PRAGMA integrity_check`. `VACUUM INTO` reads through the WAL and
    /// emits a single self-contained file.
    ///
    /// `dest` must not already exist (SQLite refuses to overwrite).
    pub fn snapshot_to(&self, dest: &Path) -> Result<(), String> {
        let dest_str = dest.to_string_lossy().to_string();
        self.conn
            .execute("VACUUM INTO ?1", params![dest_str])
            .map_err(|e| format!("snapshot to {}: {e}", dest.display()))?;
        Ok(())
    }

    /// `PRAGMA integrity_check`. Healthy is exactly `["ok"]`.
    ///
    /// Note what this does **not** prove: an empty, freshly-created database
    /// is perfectly intact. Integrity must be checked alongside the schema
    /// version and table presence, or a lost backup looks healthy.
    pub fn integrity_check(&self) -> Result<Vec<String>, String> {
        let mut stmt = self
            .conn
            .prepare("PRAGMA integrity_check")
            .map_err(|e| format!("integrity check: {e}"))?;
        let rows = stmt
            .query_map([], |row| row.get::<_, String>(0))
            .map_err(|e| e.to_string())?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())
    }

    pub fn schema_version(&self) -> Result<i64, String> {
        current_schema_version(&self.conn)
    }

    /// Applies the SQLite pragmas used by the Go store, the process's
    /// [`Synchronous`] mode, and the commit counter
    /// (`kura_store_commits_total`): every write path in the daemon opens
    /// its connection here, so this is the one place all of them pass.
    fn configure(&self) -> Result<(), String> {
        self.conn
            .execute_batch(&format!(
                "PRAGMA foreign_keys = ON;
                 PRAGMA journal_mode = WAL;
                 PRAGMA busy_timeout = 5000;
                 PRAGMA synchronous = {};",
                synchronous().pragma()
            ))
            .map_err(|e| format!("apply pragmas: {e}"))?;
        // After the pragmas: `busy_timeout` installs SQLite's own handler,
        // and installing this one replaces it.
        self.conn
            .busy_handler(Some(retry_busy))
            .map_err(|e| format!("install busy handler: {e}"))?;
        self.conn.commit_hook(Some(|| {
            kura_telemetry::metrics::registry()
                .inc(kura_telemetry::metrics::STORE_COMMITS_TOTAL, &[]);
            false // never veto the commit
        }));
        Ok(())
    }

    /// Ensures the bookkeeping table and applies any migrations newer than the current version.
    fn migrate(&self) -> Result<(), String> {
        let tx = self
            .conn
            .unchecked_transaction()
            .map_err(|e| format!("begin migration: {e}"))?;
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

        tx.commit()
            .map_err(|e| format!("commit migration transaction: {e}"))
    }

    /// Applies schema migrations only up through `target_version`, stopping before any later
    /// migration. Test-only helper mirroring the Go `MigrateToVersion`.
    pub fn migrate_to_version(&self, target_version: i64) -> Result<(), String> {
        if target_version < 1 {
            return Err(format!(
                "migrate to version: invalid target {target_version}"
            ));
        }
        let tx = self
            .conn
            .unchecked_transaction()
            .map_err(|e| format!("begin migration: {e}"))?;
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
        tx.commit()
            .map_err(|e| format!("commit migration transaction: {e}"))
    }
}

fn current_schema_version(conn: &Connection) -> Result<i64, String> {
    let version: Option<i64> = conn
        .query_row("SELECT MAX(version) FROM schema_migrations", [], |row| {
            row.get(0)
        })
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
        let home =
            std::env::var("HOME").map_err(|_| "resolve user home: HOME is not set".to_string())?;
        if data_dir == "~" {
            return Ok(home);
        }
        return Ok(Path::new(&home)
            .join(&data_dir[2..])
            .to_string_lossy()
            .to_string());
    }
    Ok(data_dir.to_string())
}

#[cfg(test)]
mod busy_and_sync_tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn the_busy_handler_retries_finely_and_gives_up_at_the_limit() {
        assert_eq!(busy_step(0), Duration::from_micros(50));
        assert_eq!(busy_step(1), Duration::from_micros(100));
        assert_eq!(busy_step(4), Duration::from_micros(800));
        assert_eq!(busy_step(5), Duration::from_millis(1));
        assert_eq!(busy_step(5_000), Duration::from_millis(1));
        // The first retries cost microseconds, not SQLite's 1-100 ms steps.
        assert!(busy_waited(5) < Duration::from_millis(2));
        // It stops once the next step would pass the limit: about 5 s, as
        // busy_timeout was.
        let last = (0..10_000).find(|n| busy_waited(*n) + busy_step(*n) > BUSY_LIMIT);
        let last = last.expect("bounded");
        assert!(busy_waited(last) <= BUSY_LIMIT);
        assert!(busy_waited(last) > BUSY_LIMIT - Duration::from_millis(2));
    }

    #[test]
    fn the_synchronous_mode_is_full_unless_normal_is_asked_for() {
        assert_eq!(Synchronous::parse("").unwrap(), Synchronous::Full);
        assert_eq!(Synchronous::parse("FULL").unwrap(), Synchronous::Full);
        assert_eq!(Synchronous::parse(" normal ").unwrap(), Synchronous::Normal);
        assert!(Synchronous::parse("off").is_err(), "OFF is never allowed");
        assert_eq!(Synchronous::Normal.pragma(), "NORMAL");
    }
}
