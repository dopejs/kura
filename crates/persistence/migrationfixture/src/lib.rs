//! kura-migrationfixture: Rust port of the Go
//! daemon/internal/store/migrationfixture package - reproducible SQLite
//! fixtures for the store's migration regression suite (Roadmap 35 US2 / T066).
//!
//! The fixture is built programmatically rather than checked in as a binary
//! blob so it stays reproducible: any future schema or seeding change is
//! reviewable as a code diff. The seeding intentionally covers at least one
//! parent + one child per parent-child pair so the head-migration backfill
//! drivers are exercised end-to-end.
//!
//! Typical flow (mirrors the Go tests):
//!
//! 1. build_pre_tenant_v21_fixture opens a fresh store at schema v21
//!    (PRE_TENANT_SCHEMA_VERSION) and seeds the pre-tenant rows.
//! 2. apply_head_migrations runs v22..head on the same store, exercising
//!    the tenant-migration backfills (loss-less: count_seeded_rows before
//!    and after must be equal).
//! 3. The per-roadmap seeders (r37, r41, r42, r48, r49, r50, r51) populate the
//!    head-only tables. FixtureBuilder orchestrates the whole sequence.
//!
//! Raw seed SQL is executed through a second connection to the store's SQLite
//! file (the store keeps its connection private; both connections share the
//! WAL database). r37 uses the store's domain CRUD where it is ported; the
//! r49-r51 channel-connector rows are written as raw SQL because the Go
//! SaveDiscordHostedSetup/telegram/slack accessors are not yet ported to
//! kura-store - the table schemas themselves exist (migrations v45-v47).

pub mod r37_credentials;
pub mod r39_production_ops;
pub mod r41_evaluation_product;
pub mod r42_integration_diagnostics;
pub mod r48_connector_conformance;
pub mod r49_discord_hardening;
pub mod r50_telegram_channel_connector;
pub mod r51_slack_channel_connector;
pub mod seeds;

mod records;

use std::collections::HashMap;
use std::time::Duration;

use rusqlite::Connection;

use kura_store::SQLiteStore;

pub use r37_credentials::{
    R37_FAKE_SECRET_TENANT_A, R37CredentialFixture, seed_r37_local_credential_files,
    seed_r37_local_credential_state,
};
pub use r39_production_ops::{
    R39ProductionOpsFixture, R39TenantState, build_r39_production_ops_fixture,
    build_r39_production_ops_sqlite_fixture, copy_r39_production_ops_sqlite_fixture,
    validate_r39_production_ops_sqlite_restore,
};
pub use r41_evaluation_product::{
    R41EvaluationProductFixture, build_r41_evaluation_product_fixture,
    count_r41_evaluation_product_rows, seed_r41_evaluation_product_rows,
};
pub use r42_integration_diagnostics::{
    R42IntegrationDiagnosticFixture, build_r42_integration_diagnostic_fixture,
    count_r42_integration_diagnostic_rows, seed_r42_integration_diagnostic_rows,
};
pub use r48_connector_conformance::{
    R48ConnectorConformanceFixture, build_r48_connector_conformance_fixture,
    count_r48_connector_conformance_rows, seed_r48_connector_conformance_rows,
};
pub use r49_discord_hardening::{
    R49DiscordHardeningFixture, build_r49_discord_hardening_fixture,
    count_r49_discord_hardening_rows, seed_r49_discord_hardening_rows,
};
pub use r50_telegram_channel_connector::{
    R50TelegramChannelConnectorFixture, build_r50_telegram_channel_connector_fixture,
    count_r50_telegram_channel_connector_rows, seed_r50_telegram_channel_connector_rows,
};
pub use r51_slack_channel_connector::{
    R51SlackChannelConnectorFixture, build_r51_slack_channel_connector_fixture,
    count_r51_slack_channel_connector_rows, seed_r51_slack_channel_connector_rows,
};
pub use seeds::{SeedRowCounts, count_seeded_rows, seed_pre_tenant_v21};

/// The head schema version BEFORE any Roadmap 35 tenant migration applied.
/// v22+ added the tenant_id columns; v21 is the last "pre-tenant" head.
/// Historical marker: the pre-baseline development schema staged fixtures at
/// v21. With the first-release baseline collapse the fixture builds at head.
pub const PRE_TENANT_SCHEMA_VERSION: i64 = 21;

/// Fixed fixture timestamp, mirroring the Go ts constant ("2025-01-01T00:00:00Z").
pub const FIXTURE_TIMESTAMP: &str = "2025-01-01T00:00:00Z";

/// Opens a second connection to the store's SQLite file for raw seed writes.
///
/// The store keeps its connection private, so the fixture (and its tests) use
/// a sibling connection: SQLite in WAL mode supports one writer plus readers,
/// and both connections share the same database file. Foreign keys are enabled
/// to match the store's pragma configuration; seed inserts are ordered
/// parent-first so the constraints hold.
pub fn open_fixture_connection(db_path: &str) -> Result<Connection, String> {
    let conn = Connection::open(db_path).map_err(|e| format!("open fixture connection: {e}"))?;
    conn.busy_timeout(Duration::from_secs(5))
        .map_err(|e| format!("set fixture connection busy timeout: {e}"))?;
    conn.execute_batch("PRAGMA foreign_keys = ON;")
        .map_err(|e| format!("enable fixture connection foreign keys: {e}"))?;
    Ok(conn)
}

/// Go BuildPreTenantV21Fixture: creates a fresh SQLite database in data_dir at
/// schema version 21 and seeds at least one parent + one child row in every
/// in-scope tenant-owned table. The returned store is open at v21 - callers
/// run apply_head_migrations on it to exercise the migrations + backfills.
pub fn build_pre_tenant_v21_fixture(data_dir: &str) -> Result<SQLiteStore, String> {
    let store = SQLiteStore::new(data_dir).map_err(|e| format!("open store: {e}"))?;
    seed_pre_tenant_v21(&store).map_err(|e| format!("seed pre-tenant v21 fixture: {e}"))?;
    Ok(store)
}

/// Go ApplyHeadMigrations: brings a store opened via build_pre_tenant_v21_fixture
/// up to CURRENT_SCHEMA_VERSION.
///
/// Note: the Go implementation additionally registers migration-progress rows
/// (Register*MigrationSteps); that tenancy-package plumbing is not ported to
/// kura-store, so this only applies the schema migrations themselves.
pub fn apply_head_migrations(store: &SQLiteStore) -> Result<(), String> {
    store.migrate_to_version(kura_store::CURRENT_SCHEMA_VERSION)
}

/// Convenience builder that produces the full regression fixture: pre-tenant
/// v21 seed, head migrations, then the per-roadmap seed data (r37-r51).
///
/// Each roadmap seeder can be toggled off (e.g. when a target type is not yet
/// ported), mirroring how the Go suite builds fixtures per test.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FixtureBuilder {
    pub seed_r37: bool,
    pub seed_r41: bool,
    pub seed_r42: bool,
    pub seed_r48: bool,
    pub seed_r49: bool,
    pub seed_r50: bool,
    pub seed_r51: bool,
}

impl Default for FixtureBuilder {
    fn default() -> Self {
        Self::new()
    }
}

impl FixtureBuilder {
    /// All roadmap seeders enabled.
    #[must_use]
    pub fn new() -> Self {
        Self {
            seed_r37: true,
            seed_r41: true,
            seed_r42: true,
            seed_r48: true,
            seed_r49: true,
            seed_r50: true,
            seed_r51: true,
        }
    }

    pub fn without_r37(mut self) -> Self {
        self.seed_r37 = false;
        self
    }

    pub fn without_r41(mut self) -> Self {
        self.seed_r41 = false;
        self
    }

    pub fn without_r42(mut self) -> Self {
        self.seed_r42 = false;
        self
    }

    pub fn without_r48(mut self) -> Self {
        self.seed_r48 = false;
        self
    }

    pub fn without_r49(mut self) -> Self {
        self.seed_r49 = false;
        self
    }

    pub fn without_r50(mut self) -> Self {
        self.seed_r50 = false;
        self
    }

    pub fn without_r51(mut self) -> Self {
        self.seed_r51 = false;
        self
    }

    /// Builds the complete fixture in data_dir: pre-tenant v21 seed -> head
    /// migrations -> r37-r51 roadmap seeds.
    pub fn build(&self, data_dir: &str) -> Result<FixtureOutput, String> {
        let store = build_pre_tenant_v21_fixture(data_dir)?;
        apply_head_migrations(&store)?;

        let r37 = if self.seed_r37 {
            Some(r37_credentials::seed_r37_local_credential_state(
                &store, data_dir,
            )?)
        } else {
            None
        };
        if self.seed_r41 {
            r41_evaluation_product::seed_r41_evaluation_product_rows(&store)?;
        }
        if self.seed_r42 {
            r42_integration_diagnostics::seed_r42_integration_diagnostic_rows(&store)?;
        }
        if self.seed_r48 {
            r48_connector_conformance::seed_r48_connector_conformance_rows(&store)?;
        }
        if self.seed_r49 {
            r49_discord_hardening::seed_r49_discord_hardening_rows(&store)?;
        }
        if self.seed_r50 {
            r50_telegram_channel_connector::seed_r50_telegram_channel_connector_rows(&store)?;
        }
        if self.seed_r51 {
            r51_slack_channel_connector::seed_r51_slack_channel_connector_rows(&store)?;
        }

        let counts = count_seeded_rows(&store)?;
        Ok(FixtureOutput { store, r37, counts })
    }
}

/// Output of FixtureBuilder::build: the open store at head plus the
/// per-roadmap seed summaries.
pub struct FixtureOutput {
    pub store: SQLiteStore,
    pub r37: Option<R37CredentialFixture>,
    pub counts: SeedRowCounts,
}

/// Row counts for the pre-tenant fixture tables (Go SeedRowCounts).
pub type TableCounts = HashMap<String, i64>;
